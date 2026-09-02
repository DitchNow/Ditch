use super::{
    ClientRequest, DitchStore, RuntimeState, ServerResponse, protocol_error, remote_control,
};
use chrono::Utc;
use ditch_commercial::{
    CommercialCapabilityId, CommercialCapabilityRegistry, CommercialEntitlement,
};
use ditch_commercial_store::CommercialStoreExt;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const RUNTIME_EDITION: &str = "commercial";

/// Statically linked private state. Future proprietary providers are added to
/// this composition without changing the Community runtime or adding daemons.
pub struct State {
    pub commercial_capabilities: CommercialCapabilityRegistry,
    pub commercial_entitlement: CommercialEntitlement,
    pub remote: remote_control::RemoteController,
}

impl State {
    pub fn new() -> Self {
        Self {
            commercial_capabilities: CommercialCapabilityRegistry::standard(),
            commercial_entitlement: CommercialEntitlement::inactive(Utc::now()),
            remote: remote_control::RemoteController::new(),
        }
    }

    pub fn commercial_allows(&self, capability: CommercialCapabilityId) -> bool {
        self.commercial_capabilities
            .allows(&self.commercial_entitlement, capability, Utc::now())
    }
}

pub fn initialize_store(store: &mut DitchStore) -> io::Result<()> {
    store
        .initialize_commercial_schema()
        .map_err(io::Error::other)
}

pub fn extend_runtime_capabilities(state: &State, capabilities: &mut Vec<String>) {
    capabilities.extend(
        state
            .commercial_capabilities
            .enabled_runtime_capabilities(&state.commercial_entitlement, Utc::now()),
    );
}

pub fn after_broadcast(state: &mut RuntimeState) {
    remote_control::publish_projection(state);
}

pub fn start(state: Arc<Mutex<RuntimeState>>) {
    start_entitlement_reconciler(Arc::clone(&state));
    remote_control::start_connection(state);
}

fn start_entitlement_reconciler(state: Arc<Mutex<RuntimeState>>) {
    thread::spawn(move || {
        loop {
            let installation_identity = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .installation_identity
                .clone();
            if let Ok(entitlement) = ditch_commercial::refresh_entitlement(&installation_identity) {
                let active = entitlement.active_at(Utc::now());
                state
                    .lock()
                    .expect("runtime state lock should not be poisoned")
                    .edition
                    .commercial_entitlement = entitlement;
                remote_control::entitlement_changed(Arc::clone(&state), active);
            }
            thread::sleep(Duration::from_secs(300));
        }
    });
}

pub fn handle_request(request: ClientRequest, state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    match request {
        ClientRequest::RemoteControlStatus => remote_control::status(state),
        ClientRequest::EnsureRemoteMachineIdentity => {
            remote_control::ensure_machine_identity(state)
        }
        ClientRequest::RemoteMachineControlStatus { alias } => {
            forward_remote_machine_request(&state, &alias, ClientRequest::RemoteControlStatus)
        }
        ClientRequest::CreateRemoteMachinePairing { alias } => {
            forward_remote_machine_request(&state, &alias, ClientRequest::CreateRemotePairing)
        }
        ClientRequest::GetRemoteMachinePairing { alias, pairing_id } => {
            forward_remote_machine_request(
                &state,
                &alias,
                ClientRequest::GetRemotePairing { pairing_id },
            )
        }
        ClientRequest::ConfirmRemoteMachinePairing { alias, pairing_id } => {
            forward_remote_machine_request(
                &state,
                &alias,
                ClientRequest::ConfirmRemotePairing { pairing_id },
            )
        }
        ClientRequest::CancelRemoteMachinePairing { alias, pairing_id } => {
            forward_remote_machine_request(
                &state,
                &alias,
                ClientRequest::CancelRemotePairing { pairing_id },
            )
        }
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

fn forward_remote_machine_request(
    state: &Arc<Mutex<RuntimeState>>,
    alias: &str,
    request: ClientRequest,
) -> ServerResponse {
    let connections = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .remote_connections
        .clone();
    connections
        .request(alias, request)
        .unwrap_or_else(|error| protocol_error("remote_unavailable", error.to_string()))
}
