use super::{
    ClientRequest, DitchStore, RuntimeState, ServerResponse, protocol_error, remote_control,
};
use chrono::Utc;
use ditch_commercial::{
    CommercialCapabilityId, CommercialCapabilityRegistry, CommercialEntitlementStatus,
    RefreshedCommercialEntitlement,
};
use ditch_commercial_store::CommercialStoreExt;
use std::fs;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const RUNTIME_EDITION: &str = "commercial";
const ENTITLEMENT_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const ENTITLEMENT_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntitlementReadiness {
    Loading,
    Ready,
    RefreshFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefreshOutcome {
    Updated,
    AlreadyInFlight,
    Failed(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapabilityAvailability {
    Allowed,
    Loading,
    Unavailable,
    Required(CommercialEntitlementStatus),
}

/// Statically linked private state. Future proprietary providers are added to
/// this composition without changing the Community runtime or adding daemons.
pub struct State {
    pub commercial_capabilities: CommercialCapabilityRegistry,
    pub remote: remote_control::RemoteController,
    entitlement: Option<RefreshedCommercialEntitlement>,
    entitlement_readiness: EntitlementReadiness,
    entitlement_refresh_in_flight: bool,
    entitlement_refresh_failures: u32,
    entitlement_generation: u64,
}

impl State {
    pub fn new() -> Self {
        Self {
            commercial_capabilities: CommercialCapabilityRegistry::standard(),
            remote: remote_control::RemoteController::new(),
            entitlement: None,
            entitlement_readiness: EntitlementReadiness::Loading,
            entitlement_refresh_in_flight: false,
            entitlement_refresh_failures: 0,
            entitlement_generation: 0,
        }
    }

    fn accept_activation(&mut self, summary: ditch_upgrade::EntitlementSummary) -> bool {
        let entitlement = ditch_commercial::refreshed_entitlement_from_summary(summary, Utc::now());
        let active = entitlement.capability.active_at(Utc::now());
        self.entitlement_generation += 1;
        self.entitlement = Some(entitlement);
        self.entitlement_readiness = EntitlementReadiness::Ready;
        self.entitlement_refresh_failures = 0;
        active
    }

    pub fn commercial_allows(&self, capability: CommercialCapabilityId) -> bool {
        self.entitlement.as_ref().is_some_and(|entitlement| {
            self.commercial_capabilities
                .allows(&entitlement.capability, capability, Utc::now())
        })
    }

    fn remote_control_availability(&self) -> CapabilityAvailability {
        if self.commercial_allows(CommercialCapabilityId::REMOTE_CONTROL) {
            return CapabilityAvailability::Allowed;
        }
        if let Some(entitlement) = self.entitlement.as_ref() {
            return CapabilityAvailability::Required(entitlement.capability.status);
        }
        match self.entitlement_readiness {
            EntitlementReadiness::Loading => CapabilityAvailability::Loading,
            EntitlementReadiness::RefreshFailed => CapabilityAvailability::Unavailable,
            EntitlementReadiness::Ready => CapabilityAvailability::Unavailable,
        }
    }
}

pub fn initialize_store(store: &mut DitchStore) -> io::Result<()> {
    store
        .initialize_commercial_schema()
        .map_err(io::Error::other)
}

pub fn extend_runtime_capabilities(state: &State, capabilities: &mut Vec<String>) {
    if let Some(entitlement) = state.entitlement.as_ref() {
        capabilities.extend(
            state
                .commercial_capabilities
                .enabled_runtime_capabilities(&entitlement.capability, Utc::now()),
        );
    }
}

pub fn after_broadcast(state: &mut RuntimeState) {
    remote_control::publish_projection(state);
}

pub fn start(state: Arc<Mutex<RuntimeState>>) {
    if state.lock().unwrap().remote_runtime {
        return;
    }
    start_entitlement_reconciler(Arc::clone(&state));
    remote_control::start_connection(state);
}

fn start_entitlement_reconciler(state: Arc<Mutex<RuntimeState>>) {
    thread::spawn(move || {
        loop {
            let delay = match refresh_entitlement_now(&state) {
                RefreshOutcome::Updated => ENTITLEMENT_REFRESH_INTERVAL,
                RefreshOutcome::AlreadyInFlight => ENTITLEMENT_RETRY_DELAYS[0],
                RefreshOutcome::Failed(failures) => entitlement_retry_delay(failures),
            };
            thread::sleep(delay);
        }
    });
}

fn entitlement_retry_delay(failures: u32) -> Duration {
    ENTITLEMENT_RETRY_DELAYS[usize::try_from(failures.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(ENTITLEMENT_RETRY_DELAYS.len() - 1)]
}

fn refresh_entitlement_now(state: &Arc<Mutex<RuntimeState>>) -> RefreshOutcome {
    let (installation_identity, generation) = {
        let mut guard = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if guard.edition.entitlement_refresh_in_flight {
            return RefreshOutcome::AlreadyInFlight;
        }
        guard.edition.entitlement_refresh_in_flight = true;
        (
            guard.installation_identity.clone(),
            guard.edition.entitlement_generation,
        )
    };
    let machine_name = remote_control::machine_name();
    match ditch_commercial::refresh_entitlement(&installation_identity, &machine_name) {
        Ok(entitlement) => {
            let mut guard = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            // A successful explicit activation supersedes an older in-flight refresh.
            if guard.edition.entitlement_generation != generation {
                guard.edition.entitlement_refresh_in_flight = false;
                return RefreshOutcome::Updated;
            }
            guard.edition.entitlement = Some(entitlement);
            guard.edition.entitlement_readiness = EntitlementReadiness::Ready;
            guard.edition.entitlement_refresh_in_flight = false;
            guard.edition.entitlement_refresh_failures = 0;
            drop(guard);
            remote_control::entitlement_changed(Arc::clone(state));
            RefreshOutcome::Updated
        }
        Err(error) => {
            let failures = {
                let mut guard = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                if guard.edition.entitlement_generation != generation {
                    guard.edition.entitlement_refresh_in_flight = false;
                    return RefreshOutcome::Updated;
                }
                guard.edition.entitlement_readiness = EntitlementReadiness::RefreshFailed;
                guard.edition.entitlement_refresh_in_flight = false;
                guard.edition.entitlement_refresh_failures =
                    guard.edition.entitlement_refresh_failures.saturating_add(1);
                guard.edition.entitlement_refresh_failures
            };
            log_entitlement_refresh_failure(state, &error.to_string(), failures);
            RefreshOutcome::Failed(failures)
        }
    }
}

fn log_entitlement_refresh_failure(state: &Arc<Mutex<RuntimeState>>, error: &str, failures: u32) {
    let message = format!("Commercial entitlement refresh attempt {failures} failed: {error}");
    eprintln!("The Ditch Runtime {message}");
    let path = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .paths
        .logs_dir
        .join("runtime.log");
    let line = format!("{} The Ditch Runtime {message}\n", Utc::now());
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn commercial_entitlement(state: Arc<Mutex<RuntimeState>>) -> Option<ServerResponse> {
    let outcome = refresh_entitlement_now(&state);
    let guard = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if let Some(entitlement) = guard.edition.entitlement.as_ref() {
        return Some(ServerResponse::CommercialEntitlement(
            entitlement.summary.clone(),
        ));
    }
    Some(match outcome {
        RefreshOutcome::AlreadyInFlight => entitlement_loading(),
        RefreshOutcome::Failed(_) => entitlement_unavailable(),
        RefreshOutcome::Updated => unreachable!("an updated entitlement must be stored"),
    })
}

pub fn accept_commercial_entitlement(
    state: Arc<Mutex<RuntimeState>>,
    summary: ditch_upgrade::EntitlementSummary,
) {
    let mut guard = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    guard.edition.accept_activation(summary);
    drop(guard);
    remote_control::entitlement_changed(state);
}

pub fn require_remote_control_entitlement(
    state: &Arc<Mutex<RuntimeState>>,
) -> Option<ServerResponse> {
    if state.lock().unwrap().remote_runtime {
        return Some(protocol_error(
            "wrong_execution_target",
            "Mobile control is available through the Mac runtime only",
        ));
    }
    let availability = {
        let guard = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        guard.edition.remote_control_availability()
    };

    match availability {
        CapabilityAvailability::Allowed => return None,
        CapabilityAvailability::Required(status) => return Some(entitlement_required(status)),
        CapabilityAvailability::Loading | CapabilityAvailability::Unavailable => {}
    }
    {
        match refresh_entitlement_now(state) {
            RefreshOutcome::Updated => {
                let guard = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                match guard.edition.remote_control_availability() {
                    CapabilityAvailability::Allowed => return None,
                    CapabilityAvailability::Required(status) => {
                        return Some(entitlement_required(status));
                    }
                    CapabilityAvailability::Loading | CapabilityAvailability::Unavailable => {}
                }
            }
            RefreshOutcome::AlreadyInFlight => return Some(entitlement_loading()),
            RefreshOutcome::Failed(_) => return Some(entitlement_unavailable()),
        }
    }
    Some(entitlement_unavailable())
}

fn entitlement_loading() -> ServerResponse {
    protocol_error(
        "commercial_entitlement_loading",
        "Ditch is activating Remote Control. Try again in a moment.",
    )
}

fn entitlement_unavailable() -> ServerResponse {
    protocol_error(
        "commercial_entitlement_unavailable",
        "Ditch could not verify Commercial access. Check the connection and try again.",
    )
}

fn entitlement_required(status: CommercialEntitlementStatus) -> ServerResponse {
    let message = if status == CommercialEntitlementStatus::Expired {
        "Commercial subscription expired. Local and SSH Ditch continue to work."
    } else {
        "Commercial access is required for Remote Control. Local and SSH Ditch continue to work."
    };
    protocol_error("commercial_entitlement_required", message)
}

pub fn handle_request(request: ClientRequest, state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    if state.lock().unwrap().remote_runtime {
        return protocol_error(
            "wrong_execution_target",
            "Mobile control is available through the Mac runtime only",
        );
    }
    match request {
        ClientRequest::RemoteControlStatus => remote_control::status(state),
        ClientRequest::CreateRemotePairing => remote_control::create_pairing(state),
        ClientRequest::GetRemotePairing { pairing_id } => {
            remote_control::get_pairing(state, pairing_id)
        }
        ClientRequest::ConfirmRemotePairing { pairing_id } => {
            remote_control::confirm_pairing(state, pairing_id)
        }
        ClientRequest::CancelRemotePairing { pairing_id } => {
            remote_control::cancel_pairing(state, pairing_id)
        }
        ClientRequest::RevokeRemoteDevice { device_id } => {
            remote_control::revoke_device(state, device_id)
        }
        ClientRequest::DisableRemoteControl => remote_control::disable(state),
        _ => protocol_error(
            "unsupported_request",
            "request is not implemented by this Commercial capability composition",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ditch_commercial::CommercialEntitlement;
    use ditch_upgrade::{CurrentLicenseSummary, EntitlementSummary};
    use std::collections::BTreeSet;

    fn refreshed(status: CommercialEntitlementStatus) -> RefreshedCommercialEntitlement {
        let active = matches!(
            status,
            CommercialEntitlementStatus::Active | CommercialEntitlementStatus::OverLimit
        );
        let status_name = match status {
            CommercialEntitlementStatus::Inactive => "inactive",
            CommercialEntitlementStatus::Active => "active",
            CommercialEntitlementStatus::OverLimit => "over_limit",
            CommercialEntitlementStatus::Expired => "expired",
            CommercialEntitlementStatus::Refunded => "refunded",
        };
        RefreshedCommercialEntitlement {
            capability: CommercialEntitlement {
                plan: "commercial_lifetime".to_owned(),
                status,
                capabilities: if active {
                    BTreeSet::from(["remote_control".to_owned()])
                } else {
                    BTreeSet::new()
                },
                valid_until: None,
                refreshed_at: Utc::now(),
            },
            summary: EntitlementSummary {
                active,
                plan: Some("commercial_lifetime".to_owned()),
                status: status_name.to_owned(),
                expires_at: None,
                mac_slots: 2,
                iphone_slots: 2,
                billing_management_available: true,
                renewal_available: false,
                current_license: CurrentLicenseSummary {
                    edition: "commercial".to_owned(),
                    status: status_name.to_owned(),
                    display_name: "Ditch Commercial".to_owned(),
                    plans: Vec::new(),
                },
            },
        }
    }

    #[test]
    fn explicit_activation_updates_capabilities_without_restarting_the_runtime() {
        let mut state = State::new();
        state.entitlement_refresh_in_flight = true;
        state.entitlement_refresh_failures = 3;
        let old_generation = state.entitlement_generation;
        assert!(state.accept_activation(refreshed(CommercialEntitlementStatus::Active).summary));
        assert_eq!(
            state.remote_control_availability(),
            CapabilityAvailability::Allowed
        );
        assert_ne!(state.entitlement_generation, old_generation);
        assert_eq!(state.entitlement_refresh_failures, 0);
        // Keep the real in-flight operation marked until it completes; its old
        // generation must not overwrite this authoritative activation snapshot.
        assert!(state.entitlement_refresh_in_flight);
    }

    #[test]
    fn active_community_license_does_not_enable_commercial_capabilities() {
        let mut state = State::new();
        let mut summary = refreshed(CommercialEntitlementStatus::Inactive).summary;
        summary.current_license.edition = "community".to_owned();
        summary.current_license.status = "active".to_owned();
        summary.plan = None;
        assert!(!state.accept_activation(summary));
        assert_eq!(
            state.remote_control_availability(),
            CapabilityAvailability::Required(CommercialEntitlementStatus::Inactive)
        );
    }

    #[test]
    fn startup_and_refresh_failure_are_not_reported_as_expired() {
        let mut state = State::new();
        assert_eq!(
            state.remote_control_availability(),
            CapabilityAvailability::Loading
        );

        state.entitlement_readiness = EntitlementReadiness::RefreshFailed;
        assert_eq!(
            state.remote_control_availability(),
            CapabilityAvailability::Unavailable
        );
    }

    #[test]
    fn confirmed_entitlement_is_the_only_source_of_required_or_allowed() {
        let mut state = State::new();
        state.entitlement_readiness = EntitlementReadiness::Ready;
        state.entitlement = Some(refreshed(CommercialEntitlementStatus::Expired));
        assert_eq!(
            state.remote_control_availability(),
            CapabilityAvailability::Required(CommercialEntitlementStatus::Expired)
        );

        state.entitlement = Some(refreshed(CommercialEntitlementStatus::Active));
        assert_eq!(
            state.remote_control_availability(),
            CapabilityAvailability::Allowed
        );
    }

    #[test]
    fn failed_refreshes_back_off_quickly_before_normal_polling() {
        assert_eq!(entitlement_retry_delay(1), Duration::from_secs(2));
        assert_eq!(entitlement_retry_delay(2), Duration::from_secs(5));
        assert_eq!(entitlement_retry_delay(3), Duration::from_secs(15));
        assert_eq!(entitlement_retry_delay(4), Duration::from_secs(60));
        assert_eq!(entitlement_retry_delay(20), Duration::from_secs(60));
        assert_eq!(ENTITLEMENT_REFRESH_INTERVAL, Duration::from_secs(300));
    }
}
