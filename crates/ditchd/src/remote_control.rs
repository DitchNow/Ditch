use super::{RuntimeState, protocol_error};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{TimeZone, Utc};
use ditch_commercial_store::{CommercialStoreExt, RemoteDeviceRecord, RemoteMachineRecord};
use ditch_protocol::{RemoteControlStatus, RemoteDeviceSummary, RemotePairing, ServerResponse};
use ditch_remote::{
    Aad, AttentionProjection, ConfirmationClass, PairwiseContext, ProjectProjection, RemoteCommand,
    RemoteCommandType, RemoteProjector, SessionProjection, SessionPromptPayload,
    SessionStartPayload, SessionTargetPayload, TranscriptQueryPayload, decrypt, encrypt,
    validate_p256_public_key,
};
use ditch_remote::{MachineIdentity, canonical_request};
use rand_core::{OsRng, RngCore};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};
use std::thread;
use std::time::{Duration as StdDuration, Instant};
use tungstenite::{Message, stream::MaybeTlsStream};
use url::Url;
use uuid::Uuid;

const HEARTBEAT_INTERVAL: StdDuration = StdDuration::from_secs(20);
const PROJECTION_ACK_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const MAX_INCREMENTAL_RECORDS: usize = 32;
const MAX_MACHINE_NAME_CHARS: usize = 120;

pub(super) struct RemoteController {
    relay_origin: Option<String>,
    pairings: HashMap<Uuid, RemotePairing>,
    authenticated_socket_live: bool,
    connection_started: bool,
    projection_wakeup: Option<SyncSender<()>>,
}

impl RemoteController {
    pub(super) fn new() -> Self {
        // Official Release builds use the origin compiled into the edition.
        // Debug builds retain a local override for Relay development only.
        #[cfg(debug_assertions)]
        let development_override = std::env::var("DITCH_REMOTE_RELAY_ORIGIN").ok();
        #[cfg(not(debug_assertions))]
        let development_override: Option<String> = None;
        let relay_origin = configured_relay_origin(
            development_override.as_deref(),
            ditch_commercial::CONFIGURED_RELAY_ORIGIN,
        );
        if let Err(error) = &relay_origin {
            eprintln!("ditchd ignored invalid DITCH_REMOTE_RELAY_ORIGIN: {error}");
        }
        Self {
            relay_origin: relay_origin.ok(),
            pairings: HashMap::new(),
            authenticated_socket_live: false,
            connection_started: false,
            projection_wakeup: None,
        }
    }
}

fn configured_relay_origin(
    override_value: Option<&str>,
    compiled_origin: &str,
) -> Result<String, String> {
    let candidate = override_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(compiled_origin);
    let parsed = Url::parse(candidate).map_err(|_| "expected an absolute URL".to_owned())?;
    let local_development = parsed.scheme() == "http"
        && parsed
            .host_str()
            .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"));
    if parsed.scheme() != "https" && !local_development {
        return Err("expected HTTPS (plain HTTP is allowed only for localhost)".to_owned());
    }
    if parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err("expected an origin without credentials, path, query, or fragment".to_owned());
    }
    Ok(parsed.origin().ascii_serialization())
}

fn has_remote_control_entitlement(state: &Arc<Mutex<RuntimeState>>) -> bool {
    state.lock().is_ok_and(|guard| {
        guard
            .edition
            .commercial_allows(ditch_commercial::CommercialCapabilityId::REMOTE_CONTROL)
    })
}

pub(super) fn entitlement_changed(state: Arc<Mutex<RuntimeState>>) {
    // Re-read the current snapshot: an older refresh notification can arrive
    // after an explicit device activation has already updated the entitlement.
    let active = {
        let Ok(mut guard) = state.lock() else {
            return;
        };
        let active = guard
            .edition
            .commercial_allows(ditch_commercial::CommercialCapabilityId::REMOTE_CONTROL);
        if !active {
            guard.edition.remote.authenticated_socket_live = false;
            if let Some(sender) = guard.edition.remote.projection_wakeup.as_ref() {
                let _ = sender.try_send(());
            }
        }
        active
    };
    if active {
        start_connection(state);
    }
}

pub(super) fn status(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    if let Some(response) = super::edition::require_remote_control_entitlement(&state) {
        return response;
    }
    let should_reconcile = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .store
        .remote_machine()
        .ok()
        .flatten()
        .is_some_and(|machine| machine.enabled && machine.owner_id.is_some());
    if should_reconcile {
        if let Err(error) = reconcile_remote_devices(&state) {
            return protocol_error("remote_device_sync_failed", error);
        }
    } else if let Err(error) = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .store
        .replace_remote_devices(&[])
    {
        return protocol_error("remote_store_failed", error.to_string());
    }
    status_from_local_store(state)
}

pub(super) fn ensure_machine_identity(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    if let Some(response) = super::edition::require_remote_control_entitlement(&state) {
        return response;
    }
    let (origin, identity, name) = {
        let mut guard = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(origin) = guard.edition.remote.relay_origin.clone() else {
            return protocol_error(
                "remote_relay_unavailable",
                "The remote relay configuration is invalid.",
            );
        };
        let identity = match ensure_identity(&mut guard) {
            Ok(identity) => identity,
            Err(error) => return protocol_error("remote_identity_failed", error),
        };
        let name = guard
            .store
            .remote_machine()
            .ok()
            .flatten()
            .map(|machine| machine.name)
            .unwrap_or_else(machine_name);
        (origin, identity, name)
    };
    // Registration publishes public keys only. Owner enrollment remains a
    // separate, short-lived authorization operation.
    if let Err(error) = register_machine(&origin, &identity, &name) {
        return protocol_error("remote_registration_failed", error);
    }
    status_from_local_store(state)
}

fn status_from_local_store(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    let state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let machine = match state.store.remote_machine() {
        Ok(value) => value,
        Err(error) => return protocol_error("remote_store_failed", error.to_string()),
    };
    let devices = match state.store.remote_devices() {
        Ok(values) => values,
        Err(error) => return protocol_error("remote_store_failed", error.to_string()),
    };
    ServerResponse::RemoteControlStatus(RemoteControlStatus {
        configured: state.edition.remote.relay_origin.is_some(),
        enabled: machine.as_ref().is_some_and(|value| value.enabled),
        machine_id: machine.as_ref().map(|value| value.machine_id),
        owner_id: machine.as_ref().and_then(|value| value.owner_id),
        machine_name: machine
            .as_ref()
            .map(|value| value.name.clone())
            .unwrap_or_else(machine_name),
        online: state.edition.remote.authenticated_socket_live,
        relay_origin: state.edition.remote.relay_origin.clone(),
        devices: devices
            .into_iter()
            .map(|device| RemoteDeviceSummary {
                device_id: device.device_id,
                name: device.name,
                state: device.state,
                last_seen_at: device.last_seen_at,
                currently_connected: false,
            })
            .collect(),
    })
}

fn reconcile_remote_devices(state: &Arc<Mutex<RuntimeState>>) -> Result<(), String> {
    let (origin, identity) =
        remote_identity(state).map_err(|_| "remote identity unavailable".to_owned())?;
    let path = machine_devices_path(identity.machine_id);
    let response = signed_request(&origin, &identity, "GET", &path, b"")?;
    let devices = active_devices_from_response(&response)?;
    state
        .lock()
        .map_err(|_| "runtime state lock poisoned".to_owned())?
        .store
        .replace_remote_devices(&devices)
        .map_err(|error| error.to_string())
}

fn machine_devices_path(machine_id: Uuid) -> String {
    format!("/v1/machines/{machine_id}/devices")
}

fn machine_device_path(machine_id: Uuid, device_id: Uuid) -> String {
    format!("{}/{device_id}", machine_devices_path(machine_id))
}

fn validated_public_key(device: &Value, field: &str) -> Result<String, String> {
    let encoded = device
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("relay device omitted {field}"))?;
    validate_p256_public_key(encoded).map_err(|_| format!("relay returned invalid {field}"))?;
    Ok(encoded.to_owned())
}

fn active_devices_from_response(value: &Value) -> Result<Vec<RemoteDeviceRecord>, String> {
    if value.get("protocol_version").and_then(Value::as_u64) != Some(1) {
        return Err("relay returned an unsupported device roster".to_owned());
    }
    let devices = value
        .get("devices")
        .and_then(Value::as_array)
        .ok_or("relay omitted devices")?;
    devices
        .iter()
        .map(|device| {
            let platform = device
                .get("platform")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 32)
                .ok_or("relay device omitted platform")?;
            if platform != "ios" {
                return Err("relay returned an unsupported device platform".to_owned());
            }
            let state = device
                .get("state")
                .and_then(Value::as_str)
                .ok_or("relay device omitted state")?;
            if state != "active" {
                return Err("relay returned a non-active device".to_owned());
            }
            device
                .get("authorized_at")
                .and_then(Value::as_i64)
                .ok_or("relay device omitted authorized_at")?;
            let last_seen_at = match device.get("last_seen_at") {
                None | Some(Value::Null) => None,
                Some(value) => Some(
                    Utc.timestamp_millis_opt(
                        value
                            .as_i64()
                            .ok_or("relay returned invalid device last_seen_at")?,
                    )
                    .single()
                    .ok_or("relay returned invalid device last_seen_at")?,
                ),
            };
            Ok(RemoteDeviceRecord {
                device_id: device
                    .get("id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .ok_or("relay device omitted id")?,
                name: device
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("relay device omitted name")?
                    .to_owned(),
                signing_public_key: validated_public_key(device, "signing_public_key")?,
                agreement_public_key: validated_public_key(device, "agreement_public_key")?,
                key_version: device
                    .get("key_version")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0)
                    .ok_or("relay device omitted key_version")?,
                state: state.to_owned(),
                last_seen_at,
            })
        })
        .collect()
}

pub(super) fn create_pairing(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    if let Some(response) = super::edition::require_remote_control_entitlement(&state) {
        return response;
    }
    let (origin, identity, installation_identity, machine_name) = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(origin) = state.edition.remote.relay_origin.clone() else {
            return protocol_error(
                "remote_not_configured",
                "The remote relay configuration is invalid.",
            );
        };
        let identity = match ensure_identity(&mut state) {
            Ok(value) => value,
            Err(error) => return protocol_error("remote_identity_failed", error),
        };
        let installation_identity = state.installation_identity.clone();
        let name = machine_name();
        (origin, identity, installation_identity, name)
    };
    if let Err(error) = refresh_official_build_authorization(&installation_identity) {
        return protocol_error("remote_official_build_failed", error);
    }
    if let Err(error) = register_machine(&origin, &identity, &machine_name) {
        return protocol_error("remote_registration_failed", error);
    }
    let response = match signed_request(&origin, &identity, "POST", "/v1/pairings", b"") {
        Ok(value) => value,
        Err(error) => return protocol_error("remote_pairing_failed", error),
    };
    let pairing = match pairing_from_response(&origin, response, true) {
        Ok(value) => value,
        Err(error) => return protocol_error("remote_pairing_failed", error),
    };
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state
        .edition
        .remote
        .pairings
        .insert(pairing.pairing_id, pairing.clone());
    ServerResponse::RemotePairing(pairing)
}

pub(super) fn get_pairing(state: Arc<Mutex<RuntimeState>>, pairing_id: Uuid) -> ServerResponse {
    if let Some(response) = super::edition::require_remote_control_entitlement(&state) {
        return response;
    }
    let (origin, identity) = match remote_identity(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let path = format!("/v1/pairings/{pairing_id}");
    let response = match signed_request(&origin, &identity, "GET", &path, b"") {
        Ok(value) => value,
        Err(error) => return protocol_error("remote_pairing_failed", error),
    };
    let pairing = match pairing_from_response(&origin, response, false) {
        Ok(value) => value,
        Err(error) => return protocol_error("remote_pairing_failed", error),
    };
    state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .edition
        .remote
        .pairings
        .insert(pairing_id, pairing.clone());
    ServerResponse::RemotePairing(pairing)
}

pub(super) fn confirm_pairing(state: Arc<Mutex<RuntimeState>>, pairing_id: Uuid) -> ServerResponse {
    if let Some(response) = super::edition::require_remote_control_entitlement(&state) {
        return response;
    }
    let (origin, identity) = match remote_identity(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let path = format!("/v1/pairings/{pairing_id}/confirm");
    let response = match signed_request(&origin, &identity, "POST", &path, b"") {
        Ok(value) => value,
        Err(error) => return protocol_error("remote_pairing_failed", error),
    };
    let owner_id = response
        .get("owner_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let device_id = response
        .get("device_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let Some(owner_id) = owner_id else {
        return protocol_error("remote_pairing_failed", "Relay omitted owner_id");
    };
    let Some(device_id) = device_id else {
        return protocol_error("remote_pairing_failed", "Relay omitted device_id");
    };
    let mut guard = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(mut machine) = guard.store.remote_machine().ok().flatten() else {
        return protocol_error(
            "remote_identity_failed",
            "Local machine identity is missing",
        );
    };
    machine.owner_id = Some(owner_id);
    machine.enabled = true;
    if let Err(error) = guard.store.upsert_remote_machine(&machine) {
        return protocol_error("remote_store_failed", error.to_string());
    }
    let device = RemoteDeviceRecord {
        device_id,
        name: response
            .get("device_name")
            .and_then(Value::as_str)
            .unwrap_or("iPhone")
            .to_owned(),
        signing_public_key: response
            .get("device_signing_public_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        agreement_public_key: response
            .get("device_agreement_public_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        key_version: 1,
        state: "active".to_owned(),
        last_seen_at: Some(Utc::now()),
    };
    if device.signing_public_key.is_empty() || device.agreement_public_key.is_empty() {
        return protocol_error("remote_pairing_failed", "Relay omitted device public keys");
    }
    if let Err(error) = guard.store.upsert_remote_device(&device) {
        return protocol_error("remote_store_failed", error.to_string());
    }
    let pairing = RemotePairing {
        pairing_id,
        machine_id: identity.machine_id,
        state: "consumed".to_owned(),
        expires_at: Utc::now(),
        qr_payload: None,
        pending_device_name: Some(device.name),
        pending_device_id: Some(device_id),
    };
    guard
        .edition
        .remote
        .pairings
        .insert(pairing_id, pairing.clone());
    drop(guard);
    start_connection(Arc::clone(&state));
    ServerResponse::RemotePairing(pairing)
}

pub(super) fn cancel_pairing(state: Arc<Mutex<RuntimeState>>, pairing_id: Uuid) -> ServerResponse {
    let (origin, identity) = match remote_identity(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let path = format!("/v1/pairings/{pairing_id}");
    if let Err(error) = signed_request(&origin, &identity, "DELETE", &path, b"") {
        return protocol_error("remote_pairing_failed", error);
    }
    state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .edition
        .remote
        .pairings
        .remove(&pairing_id);
    ServerResponse::Accepted
}

pub(super) fn revoke_device(state: Arc<Mutex<RuntimeState>>, device_id: Uuid) -> ServerResponse {
    let (origin, identity) = match remote_identity(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let path = machine_device_path(identity.machine_id, device_id);
    if let Err(error) = signed_request(&origin, &identity, "DELETE", &path, b"") {
        return protocol_error("remote_revoke_failed", error);
    }
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if let Err(error) = state.store.delete_remote_device(device_id) {
        return protocol_error("remote_store_failed", error.to_string());
    }
    ServerResponse::Accepted
}

pub(super) fn disable(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    let remote = remote_identity(&state);
    if let Ok((origin, identity)) = remote {
        let path = format!("/v1/machines/{}", identity.machine_id);
        if let Err(error) = signed_request(&origin, &identity, "DELETE", &path, b"") {
            return protocol_error("remote_disable_failed", error);
        }
    }
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state.edition.remote.authenticated_socket_live = false;
    if let Err(error) = state.store.set_remote_enabled(false) {
        return protocol_error("remote_store_failed", error.to_string());
    }
    if let Err(error) = state.store.replace_remote_devices(&[]) {
        return protocol_error("remote_store_failed", error.to_string());
    }
    ServerResponse::Accepted
}

fn ensure_identity(state: &mut RuntimeState) -> Result<MachineIdentity, String> {
    let existing = state
        .store
        .remote_machine()
        .map_err(|error| error.to_string())?;
    let identity = ditch_commercial::remote_machine_identity(&state.installation_identity)?;
    let machine_id = identity.machine_id;
    let record = RemoteMachineRecord {
        machine_id,
        owner_id: existing
            .as_ref()
            .filter(|value| value.machine_id == machine_id)
            .and_then(|value| value.owner_id),
        name: machine_name(),
        signing_public_key: identity.signing_public_key(),
        agreement_public_key: identity.agreement_public_key(),
        key_version: 1,
        enabled: existing.as_ref().is_some_and(|value| value.enabled),
        projection_epoch: existing
            .as_ref()
            .map(|value| value.projection_epoch)
            .unwrap_or_else(Uuid::new_v4),
        projection_sequence: existing
            .as_ref()
            .map(|value| value.projection_sequence)
            .unwrap_or(0),
    };
    state
        .store
        .upsert_remote_machine(&record)
        .map_err(|error| error.to_string())?;
    Ok(identity)
}

#[allow(clippy::result_large_err)]
fn remote_identity(
    state: &Arc<Mutex<RuntimeState>>,
) -> Result<(String, MachineIdentity), ServerResponse> {
    let (origin, identity, installation_identity) = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(origin) = state.edition.remote.relay_origin.clone() else {
            return Err(protocol_error(
                "remote_not_configured",
                "The remote relay configuration is invalid.",
            ));
        };
        let identity = ensure_identity(&mut state)
            .map_err(|error| protocol_error("remote_identity_failed", error))?;
        (origin, identity, state.installation_identity.clone())
    };
    refresh_official_build_authorization(&installation_identity)
        .map_err(|error| protocol_error("remote_official_build_failed", error))?;
    Ok((origin, identity))
}

fn refresh_official_build_authorization(
    identity: &ditch_identity::InstallationIdentity,
) -> Result<(), String> {
    ditch_upgrade::HttpUpgradeBackend::official()
        .and_then(|backend| backend.official_build_authorization(identity))
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn register_machine(origin: &str, identity: &MachineIdentity, name: &str) -> Result<(), String> {
    let signing = identity.signing_public_key();
    let agreement = identity.agreement_public_key();
    let canonical = format!(
        "DITCH-MACHINE-REGISTER-1\n{}\n{}\n{}\n{}",
        identity.machine_id, name, signing, agreement
    );
    let body = json!({ "protocol_version": 1, "machine_id": identity.machine_id, "name": name, "signing_public_key": signing, "agreement_public_key": agreement, "proof": identity.sign(canonical.as_bytes()) });
    let response = ureq::post(&format!("{origin}/v1/machines/register"))
        .send_json(body)
        .map_err(response_error)?;
    if response.status() != 201 {
        return Err(format!("registration returned {}", response.status()));
    }
    Ok(())
}

fn signed_request(
    origin: &str,
    identity: &MachineIdentity,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<Value, String> {
    let timestamp = Utc::now().timestamp_millis();
    let mut nonce = [0_u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let nonce = URL_SAFE_NO_PAD.encode(nonce);
    let canonical = canonical_request(method, path, body, timestamp, &nonce, identity.machine_id);
    let request = ureq::request(method, &format!("{origin}{path}"))
        .set("X-Ditch-Protocol", "1")
        .set("X-Ditch-Principal-Type", "machine")
        .set("X-Ditch-Principal-Id", &identity.machine_id.to_string())
        .set("X-Ditch-Timestamp", &timestamp.to_string())
        .set("X-Ditch-Nonce", &nonce)
        .set("X-Ditch-Signature", &identity.sign(canonical.as_bytes()));
    let request = match ditch_upgrade::cached_official_build_bearer() {
        Some(bearer) => request.set("X-Ditch-Official-Build", &bearer),
        None => request,
    };
    let response = request.send_bytes(body).map_err(response_error)?;
    if response.status() == 204 {
        return Ok(Value::Null);
    }
    response
        .into_json::<Value>()
        .map_err(|error| error.to_string())
}

fn response_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(_, response) => response
            .into_json::<Value>()
            .ok()
            .and_then(|value| {
                value
                    .pointer("/error/code")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "relay rejected request".to_owned()),
        other => other.to_string(),
    }
}

fn pairing_from_response(
    origin: &str,
    value: Value,
    include_secret: bool,
) -> Result<RemotePairing, String> {
    let pairing_id = uuid_field(&value, "pairing_id")?;
    let machine_id = uuid_field(&value, "machine_id")?;
    let expires_ms = value
        .get("expires_at")
        .and_then(Value::as_i64)
        .ok_or("relay omitted expires_at")?;
    let expires_at = Utc
        .timestamp_millis_opt(expires_ms)
        .single()
        .ok_or("relay returned invalid expiry")?;
    let qr_payload = if include_secret {
        let secret = value
            .get("secret")
            .and_then(Value::as_str)
            .ok_or("relay omitted one-time secret")?;
        Some(format!(
            "ditch://pair?v=1&relay={origin}&pairing_id={pairing_id}&machine_id={machine_id}&secret={secret}&expires_at={expires_ms}"
        ))
    } else {
        None
    };
    Ok(RemotePairing {
        pairing_id,
        machine_id,
        state: value
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_owned(),
        expires_at,
        qr_payload,
        pending_device_name: value
            .get("pending_device_name")
            .and_then(Value::as_str)
            .map(str::to_owned),
        pending_device_id: value
            .get("pending_device_id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok()),
    })
}

fn uuid_field(value: &Value, key: &str) -> Result<Uuid, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| format!("relay omitted {key}"))
}
pub(super) fn machine_name() -> String {
    preferred_machine_name(
        std::env::var("DITCH_MACHINE_NAME").ok().as_deref(),
        system_computer_name().as_deref(),
    )
}

fn preferred_machine_name(override_name: Option<&str>, system_name: Option<&str>) -> String {
    override_name
        .and_then(normalize_machine_name)
        .or_else(|| system_name.and_then(normalize_machine_name))
        .unwrap_or_else(|| "This Mac".to_owned())
}

fn normalize_machine_name(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let value = value
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_MACHINE_NAME_CHARS)
        .collect::<String>();
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(target_os = "macos")]
fn system_computer_name() -> Option<String> {
    let output = Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

#[cfg(not(target_os = "macos"))]
fn system_computer_name() -> Option<String> {
    None
}

pub(super) fn start_connection(state: Arc<Mutex<RuntimeState>>) {
    if !has_remote_control_entitlement(&state) {
        return;
    }
    let receiver = {
        let mut guard = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let enabled = guard
            .store
            .remote_machine()
            .ok()
            .flatten()
            .is_some_and(|machine| machine.enabled && machine.owner_id.is_some());
        if !enabled
            || guard.edition.remote.connection_started
            || guard.edition.remote.relay_origin.is_none()
        {
            return;
        }
        // Capacity one coalesces any number of local domain events into one
        // wake-up. The connection thread rebuilds sanitized state from the
        // authoritative runtime, so projection payloads cannot accumulate in
        // an unbounded channel.
        let (sender, receiver) = mpsc::sync_channel(1);
        guard.edition.remote.projection_wakeup = Some(sender);
        guard.edition.remote.connection_started = true;
        receiver
    };
    thread::spawn(move || connection_loop(state, receiver));
}

pub(super) fn publish_projection(state: &mut RuntimeState) {
    if !state
        .edition
        .commercial_allows(ditch_commercial::CommercialCapabilityId::REMOTE_CONTROL)
    {
        return;
    }
    if !state.edition.remote.authenticated_socket_live {
        return;
    }
    let Some(sender) = state.edition.remote.projection_wakeup.clone() else {
        return;
    };
    match sender.try_send(()) {
        Ok(()) | Err(TrySendError::Full(())) => {}
        Err(TrySendError::Disconnected(())) => {
            state.edition.remote.authenticated_socket_live = false;
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ProjectionSnapshot {
    records: BTreeMap<(String, String), Value>,
}

impl ProjectionSnapshot {
    fn from_state(state: &RuntimeState) -> Self {
        let agents = state
            .agents
            .values()
            .map(|record| record.run.clone())
            .collect::<Vec<_>>();
        let mut records = BTreeMap::new();
        for project in state.projects.values() {
            let value = RemoteProjector::project(project, &agents, &state.attention, 1);
            if let Ok(record) = serde_json::to_value(value) {
                records.insert(("project".to_owned(), project.id.0.to_string()), record);
            }
        }
        for run in &agents {
            let value = RemoteProjector::session(run, &state.attention, 1);
            if let Ok(record) = serde_json::to_value(value) {
                records.insert(("session".to_owned(), run.id.0.to_string()), record);
            }
        }
        for item in &state.attention {
            let value = RemoteProjector::attention(item, 1);
            if let Ok(record) = serde_json::to_value(value) {
                records.insert(("attention".to_owned(), item.id.to_string()), record);
            }
        }
        Self { records }
    }

    fn counts(&self) -> Value {
        let count = |kind: &str| {
            self.records
                .keys()
                .filter(|(record_kind, _)| record_kind == kind)
                .count()
        };
        json!({
            "projects": count("project"),
            "sessions": count("session"),
            "attention": count("attention"),
        })
    }

    fn persist(&self, state: &mut RuntimeState, epoch: Uuid, sequence: u64) -> Result<(), String> {
        let projects = self
            .records
            .iter()
            .filter(|((kind, _), _)| kind == "project")
            .map(|(_, value)| serde_json::from_value::<ProjectProjection>(value.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let sessions = self
            .records
            .iter()
            .filter(|((kind, _), _)| kind == "session")
            .map(|(_, value)| serde_json::from_value::<SessionProjection>(value.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let attention = self
            .records
            .iter()
            .filter(|((kind, _), _)| kind == "attention")
            .map(|(_, value)| serde_json::from_value::<AttentionProjection>(value.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        state
            .store
            .replace_remote_projection_cache(epoch, sequence, &projects, &sessions, &attention)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug)]
struct ProjectionWork {
    epoch: Uuid,
    sequence: u64,
    frame: String,
    resulting_snapshot: Option<ProjectionSnapshot>,
    sent_at: Option<Instant>,
}

#[derive(Debug)]
struct ProjectionTransport {
    owner_id: Uuid,
    machine_id: Uuid,
    epoch: Uuid,
    next_sequence: u64,
    acknowledged_sequence: u64,
    queue: VecDeque<ProjectionWork>,
    inflight: Option<ProjectionWork>,
    baseline: ProjectionSnapshot,
    dirty: bool,
}

#[derive(Debug)]
enum ProjectionAckOutcome {
    Ignored,
    Advanced,
    Committed(ProjectionSnapshot),
}

impl ProjectionTransport {
    fn new(owner_id: Uuid, machine_id: Uuid, snapshot: ProjectionSnapshot) -> Self {
        let mut transport = Self {
            owner_id,
            machine_id,
            epoch: Uuid::new_v4(),
            next_sequence: 0,
            acknowledged_sequence: 0,
            queue: VecDeque::new(),
            inflight: None,
            baseline: ProjectionSnapshot::default(),
            dirty: false,
        };
        transport.queue_snapshot(snapshot);
        transport
    }

    fn queue_snapshot(&mut self, mut snapshot: ProjectionSnapshot) {
        self.epoch = Uuid::new_v4();
        self.next_sequence = 0;
        self.acknowledged_sequence = 0;
        self.queue.clear();
        self.inflight = None;
        self.dirty = false;
        let counts = snapshot.counts();
        self.enqueue(
            "projection_snapshot_begin",
            json!({ "counts": counts }),
            None,
        );
        let keys = snapshot.records.keys().cloned().collect::<Vec<_>>();
        for key in keys {
            let sequence = self.next_sequence + 1;
            let Some(record) = snapshot.records.get_mut(&key) else {
                continue;
            };
            set_projection_version(record, sequence);
            let record = record.clone();
            self.enqueue(
                "projection_snapshot_record",
                json!({ "kind": key.0, "record": record }),
                None,
            );
        }
        self.enqueue(
            "projection_snapshot_commit",
            json!({ "counts": snapshot.counts() }),
            Some(snapshot),
        );
    }

    fn queue_incremental(&mut self, current: ProjectionSnapshot) {
        if !self.queue.is_empty() || self.inflight.is_some() {
            self.dirty = true;
            return;
        }
        let mut changes = Vec::new();
        for (key, record) in &current.records {
            let changed = self
                .baseline
                .records
                .get(key)
                .is_none_or(|baseline| !same_projection_content(baseline, record));
            if changed {
                changes.push((key.clone(), Some(record.clone())));
            }
        }
        for key in self.baseline.records.keys() {
            if !current.records.contains_key(key) {
                changes.push((key.clone(), None));
            }
        }
        changes.sort_by(|left, right| left.0.cmp(&right.0));
        self.dirty = changes.len() > MAX_INCREMENTAL_RECORDS;
        let mut next_baseline = self.baseline.clone();
        let selected = changes
            .into_iter()
            .take(MAX_INCREMENTAL_RECORDS)
            .collect::<Vec<_>>();
        for (index, (key, record)) in selected.iter().enumerate() {
            let is_last = index + 1 == selected.len();
            match record {
                Some(value) => {
                    let mut value = value.clone();
                    set_projection_version(&mut value, self.next_sequence + 1);
                    next_baseline.records.insert(key.clone(), value.clone());
                    self.enqueue(
                        "projection_update",
                        json!({ "kind": key.0, "record": value }),
                        is_last.then(|| next_baseline.clone()),
                    );
                }
                None => {
                    next_baseline.records.remove(key);
                    self.enqueue(
                        "projection_update",
                        json!({ "kind": "delete", "record": { "entity": key.0, "id": key.1 } }),
                        is_last.then(|| next_baseline.clone()),
                    );
                }
            }
        }
        if selected.is_empty() {
            self.dirty = false;
        }
    }

    fn enqueue(
        &mut self,
        frame_type: &str,
        details: Value,
        resulting_snapshot: Option<ProjectionSnapshot>,
    ) {
        self.next_sequence += 1;
        let mut payload = details.as_object().cloned().unwrap_or_default();
        payload.insert("owner_id".to_owned(), json!(self.owner_id));
        payload.insert("machine_id".to_owned(), json!(self.machine_id));
        payload.insert("epoch".to_owned(), json!(self.epoch));
        payload.insert("sequence".to_owned(), json!(self.next_sequence));
        self.queue.push_back(ProjectionWork {
            epoch: self.epoch,
            sequence: self.next_sequence,
            frame: websocket_frame(frame_type, Value::Object(payload)),
            resulting_snapshot,
            sent_at: None,
        });
    }

    fn acknowledge(
        &mut self,
        epoch: Uuid,
        sequence: u64,
        status: &str,
    ) -> Result<ProjectionAckOutcome, String> {
        if !matches!(status, "applied" | "duplicate") {
            return Err("relay returned invalid projection acknowledgement".to_owned());
        }
        if epoch != self.epoch || sequence <= self.acknowledged_sequence {
            return Ok(ProjectionAckOutcome::Ignored);
        }
        let Some(work) = self.inflight.take() else {
            return Err("relay acknowledged a projection that was not in flight".to_owned());
        };
        if work.epoch != epoch || work.sequence != sequence {
            self.inflight = Some(work);
            return Err("relay acknowledged an unexpected projection sequence".to_owned());
        }
        self.acknowledged_sequence = sequence;
        if let Some(snapshot) = work.resulting_snapshot {
            self.baseline = snapshot.clone();
            return Ok(ProjectionAckOutcome::Committed(snapshot));
        }
        Ok(ProjectionAckOutcome::Advanced)
    }

    fn next_frame(&mut self) -> Option<&mut ProjectionWork> {
        if self.inflight.is_none() {
            self.inflight = self.queue.pop_front();
        }
        self.inflight.as_mut()
    }
}

fn set_projection_version(record: &mut Value, version: u64) {
    if let Some(record) = record.as_object_mut() {
        record.insert("projection_version".to_owned(), json!(version));
    }
}

fn same_projection_content(left: &Value, right: &Value) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    if let Some(value) = left.as_object_mut() {
        value.remove("projection_version");
    }
    if let Some(value) = right.as_object_mut() {
        value.remove("projection_version");
    }
    left == right
}

fn websocket_frame(frame_type: &str, payload: Value) -> String {
    json!({
        "protocol_version": 1,
        "message_id": Uuid::new_v4(),
        "type": frame_type,
        "created_at": Utc::now().timestamp_millis(),
        "payload": payload,
    })
    .to_string()
}

fn persist_projection_progress(
    state: &mut RuntimeState,
    epoch: Uuid,
    sequence: u64,
) -> Result<(), String> {
    let Some(mut machine) = state
        .store
        .remote_machine()
        .map_err(|error| error.to_string())?
    else {
        return Err("remote identity unavailable".to_owned());
    };
    machine.projection_epoch = epoch;
    machine.projection_sequence = sequence;
    state
        .store
        .upsert_remote_machine(&machine)
        .map_err(|error| error.to_string())
}

fn connection_loop(state: Arc<Mutex<RuntimeState>>, receiver: Receiver<()>) {
    let mut backoff = 1_u64;
    loop {
        let enabled = state.lock().ok().is_some_and(|guard| {
            guard
                .edition
                .commercial_allows(ditch_commercial::CommercialCapabilityId::REMOTE_CONTROL)
                && guard
                    .store
                    .remote_machine()
                    .ok()
                    .flatten()
                    .is_some_and(|machine| machine.enabled)
        });
        if !enabled {
            break;
        }
        match connect_once(&state, &receiver) {
            Ok(()) => backoff = 1,
            Err(error) => {
                eprintln!(
                    "{} remote relay disconnected: {error}",
                    super::RUNTIME_IDENTITY
                );
                if let Ok(mut guard) = state.lock() {
                    guard.edition.remote.authenticated_socket_live = false;
                }
                let mut jitter = [0_u8; 1];
                OsRng.fill_bytes(&mut jitter);
                thread::sleep(StdDuration::from_millis(
                    backoff * 1000 + u64::from(jitter[0]),
                ));
                backoff = (backoff * 2).min(60);
            }
        }
    }
    if let Ok(mut guard) = state.lock() {
        guard.edition.remote.authenticated_socket_live = false;
        guard.edition.remote.connection_started = false;
        guard.edition.remote.projection_wakeup = None;
    }
}

fn connect_once(state: &Arc<Mutex<RuntimeState>>, receiver: &Receiver<()>) -> Result<(), String> {
    if !has_remote_control_entitlement(state) {
        return Err("commercial entitlement is inactive".to_owned());
    }
    let (origin, identity) =
        remote_identity(state).map_err(|_| "remote identity unavailable".to_owned())?;
    let epoch = Uuid::new_v4();
    let body = serde_json::to_vec(
        &json!({ "machine_id": identity.machine_id, "role": "machine", "connection_epoch": epoch }),
    )
    .map_err(|error| error.to_string())?;
    let ticket_response =
        signed_request(&origin, &identity, "POST", "/v1/auth/socket-ticket", &body)?;
    let ticket = ticket_response
        .get("ticket")
        .and_then(Value::as_str)
        .ok_or("relay omitted socket ticket")?;
    let websocket_origin = origin
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let (mut socket, _) =
        tungstenite::connect(format!("{websocket_origin}/v1/socket?ticket={ticket}"))
            .map_err(|error| error.to_string())?;
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            let _ = stream.set_read_timeout(Some(StdDuration::from_millis(200)));
        }
        MaybeTlsStream::Rustls(stream) => {
            let _ = stream
                .get_mut()
                .set_read_timeout(Some(StdDuration::from_millis(200)));
        }
        _ => {}
    }
    let mut transport = {
        let mut guard = state
            .lock()
            .map_err(|_| "runtime lock poisoned".to_owned())?;
        let machine = guard
            .store
            .remote_machine()
            .map_err(|error| error.to_string())?
            .ok_or("remote identity unavailable")?;
        let owner_id = machine.owner_id.ok_or("remote owner unavailable")?;
        guard.edition.remote.authenticated_socket_live = true;
        ProjectionTransport::new(
            owner_id,
            machine.machine_id,
            ProjectionSnapshot::from_state(&guard),
        )
    };
    let mut last_heartbeat = Instant::now();
    loop {
        if !has_remote_control_entitlement(state) {
            return Err("commercial entitlement expired".to_owned());
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                match handle_socket_frame(state, &identity, &mut socket, text.as_str())? {
                    RelayFrameAction::None => {}
                    RelayFrameAction::ProjectionAck {
                        epoch,
                        sequence,
                        status,
                    } => match transport.acknowledge(epoch, sequence, &status)? {
                        ProjectionAckOutcome::Ignored => {}
                        ProjectionAckOutcome::Advanced => {
                            let mut guard = state
                                .lock()
                                .map_err(|_| "runtime lock poisoned".to_owned())?;
                            persist_projection_progress(&mut guard, epoch, sequence)?;
                        }
                        ProjectionAckOutcome::Committed(snapshot) => {
                            let mut guard = state
                                .lock()
                                .map_err(|_| "runtime lock poisoned".to_owned())?;
                            snapshot.persist(&mut guard, epoch, sequence)?;
                        }
                    },
                    RelayFrameAction::ProjectionGap { epoch } => {
                        if epoch != transport.epoch {
                            continue;
                        }
                        let snapshot = {
                            let guard = state
                                .lock()
                                .map_err(|_| "runtime lock poisoned".to_owned())?;
                            ProjectionSnapshot::from_state(&guard)
                        };
                        transport.queue_snapshot(snapshot);
                    }
                }
            }
            Ok(Message::Close(_)) => return Err("relay closed connection".to_owned()),
            Ok(Message::Ping(value)) => socket
                .send(Message::Pong(value))
                .map_err(|error| error.to_string())?,
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.to_string()),
        }

        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            socket
                .send(Message::Text(
                    websocket_frame("heartbeat", json!({})).into(),
                ))
                .map_err(|error| error.to_string())?;
            last_heartbeat = Instant::now();
        }

        match receiver.try_recv() {
            Ok(()) => transport.dirty = true,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return Ok(()),
        }

        if transport.queue.is_empty() && transport.inflight.is_none() && transport.dirty {
            let snapshot = {
                let guard = state
                    .lock()
                    .map_err(|_| "runtime lock poisoned".to_owned())?;
                ProjectionSnapshot::from_state(&guard)
            };
            transport.queue_incremental(snapshot);
        }

        if let Some(work) = transport.next_frame() {
            if work
                .sent_at
                .is_some_and(|sent_at| sent_at.elapsed() >= PROJECTION_ACK_TIMEOUT)
            {
                return Err("projection acknowledgement timed out".to_owned());
            }
            if work.sent_at.is_none() {
                socket
                    .send(Message::Text(work.frame.clone().into()))
                    .map_err(|error| error.to_string())?;
                work.sent_at = Some(Instant::now());
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum RelayFrameAction {
    None,
    ProjectionAck {
        epoch: Uuid,
        sequence: u64,
        status: String,
    },
    ProjectionGap {
        epoch: Uuid,
    },
}

fn handle_socket_frame<S: std::io::Read + std::io::Write>(
    state: &Arc<Mutex<RuntimeState>>,
    identity: &MachineIdentity,
    socket: &mut tungstenite::WebSocket<S>,
    text: &str,
) -> Result<RelayFrameAction, String> {
    let frame: Value = serde_json::from_str(text).map_err(|_| "invalid relay frame".to_owned())?;
    if frame.get("protocol_version").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported relay protocol".to_owned());
    }
    let frame_type = frame
        .get("type")
        .and_then(Value::as_str)
        .ok_or("relay frame omitted type")?;
    if frame_type == "projection_ack" {
        let payload = frame
            .get("payload")
            .and_then(Value::as_object)
            .ok_or("projection acknowledgement omitted payload")?;
        let epoch = payload
            .get("epoch")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or("projection acknowledgement omitted epoch")?;
        let sequence = payload
            .get("accepted_sequence")
            .and_then(Value::as_u64)
            .ok_or("projection acknowledgement omitted sequence")?;
        let status = payload
            .get("status")
            .and_then(Value::as_str)
            .ok_or("projection acknowledgement omitted status")?
            .to_owned();
        return Ok(RelayFrameAction::ProjectionAck {
            epoch,
            sequence,
            status,
        });
    }
    if frame_type == "projection_gap" {
        let payload = frame
            .get("payload")
            .and_then(Value::as_object)
            .ok_or("projection gap omitted payload")?;
        let epoch = payload
            .get("epoch")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or("projection gap omitted epoch")?;
        payload
            .get("expected_sequence")
            .and_then(Value::as_u64)
            .ok_or("projection gap omitted expected sequence")?;
        payload
            .get("received_sequence")
            .and_then(Value::as_u64)
            .ok_or("projection gap omitted received sequence")?;
        if payload
            .get("full_snapshot_required")
            .and_then(Value::as_bool)
            != Some(true)
        {
            return Err("projection gap did not require reconciliation".to_owned());
        }
        return Ok(RelayFrameAction::ProjectionGap { epoch });
    }
    if matches!(frame_type, "hello" | "presence") {
        return Ok(RelayFrameAction::None);
    }
    if frame_type == "machine_access_revoked" {
        if let Some(device_id) = frame
            .get("payload")
            .and_then(|payload| payload.get("device_id"))
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
        {
            let mut guard = state
                .lock()
                .map_err(|_| "runtime lock poisoned".to_owned())?;
            guard
                .store
                .delete_remote_device(device_id)
                .map_err(|error| error.to_string())?;
        }
        return Ok(RelayFrameAction::None);
    }
    if frame_type == "revoked" {
        let mut guard = state
            .lock()
            .map_err(|_| "runtime lock poisoned".to_owned())?;
        guard.edition.remote.authenticated_socket_live = false;
        guard
            .store
            .set_remote_enabled(false)
            .map_err(|error| error.to_string())?;
        guard
            .store
            .replace_remote_devices(&[])
            .map_err(|error| error.to_string())?;
        return Err("machine revoked".to_owned());
    }
    if frame_type == "error" {
        let code = frame
            .get("payload")
            .and_then(|payload| payload.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("relay_error");
        return Err(format!("relay error: {code}"));
    }
    if frame_type != "command" {
        return Err("relay frame type denied".to_owned());
    }
    let message_id = frame
        .get("message_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or("command omitted message id")?;
    let command: RemoteCommand = serde_json::from_value(
        frame
            .get("payload")
            .cloned()
            .ok_or("command omitted payload")?,
    )
    .map_err(|_| "invalid command envelope".to_owned())?;
    let device = state
        .lock()
        .map_err(|_| "runtime lock poisoned".to_owned())?
        .store
        .remote_devices()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|device| device.device_id == command.device_id && device.state == "active")
        .ok_or("device_revoked")?;
    let context = PairwiseContext {
        owner_id: command.owner_id,
        machine_id: command.machine_id,
        device_id: command.device_id,
        key_version: command.payload.key_version,
    };
    let key = identity
        .derive_pairwise(&device.agreement_public_key, &context)
        .map_err(|_| "invalid device key".to_owned())?;
    let aad = Aad {
        command_or_event_type: serde_json::to_value(command.command_type)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default(),
        created_at: command.created_at,
        device_id: command.device_id,
        expires_at: command.expires_at,
        machine_id: command.machine_id,
        message_id,
        owner_id: command.owner_id,
        protocol_version: command.protocol_version,
    };
    let plaintext =
        decrypt(&key, &aad, &command.payload).map_err(|_| "modified ciphertext".to_owned())?;
    let wrapper: Value =
        serde_json::from_slice(&plaintext).map_err(|_| "invalid encrypted command".to_owned())?;
    let body = wrapper
        .get("body")
        .cloned()
        .unwrap_or_else(|| wrapper.clone());
    let authenticated = wrapper
        .get("owner_authenticated_at")
        .and_then(Value::as_i64)
        .is_some_and(|timestamp| {
            let age = Utc::now().timestamp_millis() - timestamp;
            (0..=60_000).contains(&age)
        });
    socket.send(Message::Text(json!({ "protocol_version": 1, "message_id": Uuid::new_v4(), "type": "command_ack", "created_at": Utc::now().timestamp_millis(), "payload": { "command_id": command.command_id, "device_id": command.device_id, "status": "accepted" } }).to_string().into())).map_err(|error| error.to_string())?;
    let result = execute_command(
        Arc::clone(state),
        &command,
        &serde_json::to_vec(&body).map_err(|error| error.to_string())?,
        authenticated,
    );
    let result_message_id = Uuid::new_v4();
    let created_at = Utc::now().timestamp_millis();
    let expires_at = created_at + 60_000;
    let result_aad = Aad {
        command_or_event_type: "command_result".to_owned(),
        created_at,
        device_id: command.device_id,
        expires_at,
        machine_id: command.machine_id,
        message_id: result_message_id,
        owner_id: command.owner_id,
        protocol_version: 1,
    };
    let encrypted = encrypt(
        &key,
        &result_aad,
        &serde_json::to_vec(&result).map_err(|error| error.to_string())?,
    )
    .map_err(|_| "result encryption failed".to_owned())?;
    socket.send(Message::Text(json!({ "protocol_version": 1, "message_id": result_message_id, "type": "command_result", "created_at": created_at, "payload": { "command_id": command.command_id, "device_id": command.device_id, "expires_at": expires_at, "encrypted": encrypted } }).to_string().into())).map_err(|error| error.to_string())?;
    Ok(RelayFrameAction::None)
}

#[derive(Debug, serde::Serialize)]
pub(super) struct RemoteCommandResult {
    pub command_id: Uuid,
    pub status: &'static str,
    pub error_code: Option<&'static str>,
    pub result: Option<Value>,
}

/// Execute a decrypted, authenticated command through the same functions used
/// by the desktop IPC dispatcher. Network code must never bypass this adapter.
pub(super) fn execute_command(
    state: Arc<Mutex<RuntimeState>>,
    command: &RemoteCommand,
    plaintext: &[u8],
    device_owner_authenticated: bool,
) -> RemoteCommandResult {
    let failure = |code| RemoteCommandResult {
        command_id: command.command_id,
        status: "rejected",
        error_code: Some(code),
        result: None,
    };
    let confirmation = match command.validate(Utc::now()) {
        Ok(value) => value,
        Err(error) => return failure(remote_error_code(&error)),
    };
    if confirmation == ConfirmationClass::DesktopOnly
        || (confirmation >= ConfirmationClass::DeviceOwnerAuthentication
            && !device_owner_authenticated)
    {
        return failure("action_not_allowed");
    }
    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(machine) = state.store.remote_machine().ok().flatten() else {
            return failure("unauthorized");
        };
        if !machine.enabled
            || machine.machine_id != command.machine_id
            || machine.owner_id != Some(command.owner_id)
        {
            return failure("unauthorized");
        }
        let device_authorized = state.store.remote_devices().ok().is_some_and(|devices| {
            devices
                .iter()
                .any(|device| device.device_id == command.device_id && device.state == "active")
        });
        if !device_authorized {
            return failure("device_revoked");
        }
        let command_type = serde_json::to_value(command.command_type)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".to_owned());
        let Some(expires_at) = Utc.timestamp_millis_opt(command.expires_at).single() else {
            return failure("command_expired");
        };
        match state.store.begin_remote_command(
            command.command_id,
            &command.idempotency_key,
            &command_type,
            command.device_id,
            expires_at,
        ) {
            Ok(true) => {}
            Ok(false) => return failure("command_duplicate"),
            Err(_) => return failure("internal_error"),
        }
    }

    let response = match command.command_type {
        RemoteCommandType::SessionStart => {
            let payload: SessionStartPayload = match serde_json::from_slice(plaintext) {
                Ok(value) => value,
                Err(_) => return finish_rejected(&state, command, "action_not_allowed"),
            };
            if payload.initial_prompt.is_empty()
                || payload.initial_prompt.chars().count() > 100_000
                || payload.task_title.chars().count() > 200
            {
                return finish_rejected(&state, command, "action_not_allowed");
            }
            let project = {
                let state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                state
                    .projects
                    .values()
                    .find(|project| project.id.0 == payload.project_id)
                    .cloned()
            };
            let Some(project) = project else {
                return finish_rejected(&state, command, "session_not_found");
            };
            super::start_codex_session(
                state.clone(),
                project.name,
                project.root.to_string_lossy().into_owned(),
                payload.initial_prompt,
                ditch_core::CodexLaunchMode::Exec,
                ditch_core::AgentExecutionProfile::default(),
            )
        }
        RemoteCommandType::SessionPrompt => {
            let payload: SessionPromptPayload = match serde_json::from_slice(plaintext) {
                Ok(value) => value,
                Err(_) => return finish_rejected(&state, command, "action_not_allowed"),
            };
            if payload.text.is_empty() || payload.text.chars().count() > 100_000 {
                return finish_rejected(&state, command, "action_not_allowed");
            }
            super::prompt_agent(
                state.clone(),
                ditch_core::AgentId(payload.session_id),
                payload.text,
                ditch_core::AgentExecutionProfile::default(),
            )
        }
        RemoteCommandType::SessionStop => {
            let payload: SessionTargetPayload = match serde_json::from_slice(plaintext) {
                Ok(value) => value,
                Err(_) => return finish_rejected(&state, command, "action_not_allowed"),
            };
            super::stop_agent(state.clone(), ditch_core::AgentId(payload.session_id))
        }
        RemoteCommandType::SessionForceKill => {
            let payload: SessionTargetPayload = match serde_json::from_slice(plaintext) {
                Ok(value) => value,
                Err(_) => return finish_rejected(&state, command, "action_not_allowed"),
            };
            super::force_kill_agent(state.clone(), ditch_core::AgentId(payload.session_id))
        }
        RemoteCommandType::AttentionAcknowledge => {
            let value: Value = match serde_json::from_slice(plaintext) {
                Ok(value) => value,
                Err(_) => return finish_rejected(&state, command, "action_not_allowed"),
            };
            let Some(id) = value
                .get("attention_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
            else {
                return finish_rejected(&state, command, "action_not_allowed");
            };
            super::mark_attention_read(state.clone(), Some(&[id]))
        }
        RemoteCommandType::QuerySessionTranscript => {
            let payload: TranscriptQueryPayload = match serde_json::from_slice(plaintext) {
                Ok(value) => value,
                Err(_) => return finish_rejected(&state, command, "action_not_allowed"),
            };
            if payload.limit == 0 || payload.limit > 200 {
                return finish_rejected(&state, command, "action_not_allowed");
            }
            let page = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .store
                .list_agent_messages(
                    ditch_core::AgentId(payload.session_id),
                    payload.before_sequence,
                    payload.limit,
                );
            match page {
                Ok(value) => ServerResponse::AgentMessages(value),
                Err(_) => return finish_rejected(&state, command, "session_not_found"),
            }
        }
        // These domain services do not exist in the current shipped runtime.
        // Fail closed until desktop and remote can share a canonical service.
        RemoteCommandType::ApprovalRespond
        | RemoteCommandType::AttentionExecute
        | RemoteCommandType::AttentionSnooze
        | RemoteCommandType::IntegrationApply
        | RemoteCommandType::QuerySessionReview => {
            return finish_rejected(&state, command, "action_not_allowed");
        }
    };
    let result = serde_json::to_value(&response).ok();
    let (status, error_code) = match &response {
        ServerResponse::Error(error) => ("rejected", Some(error.code.as_str())),
        _ => ("completed", None),
    };
    let owned_error = error_code.map(str::to_owned);
    if let Ok(mut state) = state.lock() {
        let _ = state.store.finish_remote_command(
            command.command_id,
            status,
            owned_error.as_deref(),
            result
                .as_ref()
                .and_then(|value| serde_json::to_string(value).ok())
                .as_deref(),
        );
    }
    RemoteCommandResult {
        command_id: command.command_id,
        status,
        error_code: match owned_error.as_deref() {
            Some("agent_not_found") => Some("session_not_found"),
            Some("agent_busy") => Some("session_not_running"),
            Some(_) => Some("action_not_allowed"),
            None => None,
        },
        result,
    }
}

fn finish_rejected(
    state: &Arc<Mutex<RuntimeState>>,
    command: &RemoteCommand,
    code: &'static str,
) -> RemoteCommandResult {
    if let Ok(mut state) = state.lock() {
        let _ = state
            .store
            .finish_remote_command(command.command_id, "rejected", Some(code), None);
    }
    RemoteCommandResult {
        command_id: command.command_id,
        status: "rejected",
        error_code: Some(code),
        result: None,
    }
}

fn remote_error_code(error: &ditch_remote::RemoteError) -> &'static str {
    match error {
        ditch_remote::RemoteError::UnsupportedProtocol => "unsupported_protocol",
        ditch_remote::RemoteError::CommandExpired => "command_expired",
        ditch_remote::RemoteError::CommandDuplicate => "command_duplicate",
        _ => "action_not_allowed",
    }
}

#[cfg(test)]
mod relay_origin_tests {
    use super::{
        MAX_MACHINE_NAME_CHARS, ProjectionAckOutcome, ProjectionSnapshot, ProjectionTransport,
        active_devices_from_response, configured_relay_origin, machine_device_path,
        machine_devices_path, preferred_machine_name,
    };
    use ditch_remote::ProjectProjection;
    use serde_json::{Value, json};
    use uuid::Uuid;

    #[derive(Default)]
    struct FakeProjectionRelay {
        active: ProjectionSnapshot,
        staging: Option<(Uuid, u64, ProjectionSnapshot)>,
    }

    impl FakeProjectionRelay {
        fn receive(&mut self, frame: &str) -> Value {
            let frame: Value = serde_json::from_str(frame).unwrap();
            let payload = frame["payload"].as_object().unwrap();
            let epoch = Uuid::parse_str(payload["epoch"].as_str().unwrap()).unwrap();
            let sequence = payload["sequence"].as_u64().unwrap();
            let expected = self
                .staging
                .as_ref()
                .filter(|(staging_epoch, _, _)| *staging_epoch == epoch)
                .map_or(1, |(_, accepted, _)| accepted + 1);
            if sequence != expected {
                return json!({
                    "protocol_version": 1,
                    "message_id": Uuid::new_v4(),
                    "type": "projection_gap",
                    "created_at": 1_787_596_800_000_i64,
                    "payload": {
                        "epoch": epoch,
                        "expected_sequence": expected,
                        "received_sequence": sequence,
                        "full_snapshot_required": true
                    }
                });
            }
            match frame["type"].as_str().unwrap() {
                "projection_snapshot_begin" => {
                    self.staging = Some((epoch, sequence, ProjectionSnapshot::default()));
                }
                "projection_snapshot_record" => {
                    let (_, accepted, snapshot) = self.staging.as_mut().unwrap();
                    *accepted = sequence;
                    let kind = payload["kind"].as_str().unwrap().to_owned();
                    let record = payload["record"].clone();
                    let id_key = match kind.as_str() {
                        "project" => "project_id",
                        "session" => "session_id",
                        "attention" => "attention_id",
                        _ => panic!("unexpected snapshot kind"),
                    };
                    let id = record[id_key].as_str().unwrap().to_owned();
                    snapshot.records.insert((kind, id), record);
                }
                "projection_snapshot_commit" => {
                    let (_, accepted, _) = self.staging.as_mut().unwrap();
                    *accepted = sequence;
                    self.active = self.staging.take().unwrap().2;
                }
                _ => panic!("unexpected fake-relay frame"),
            }
            if let Some((_, accepted, _)) = self.staging.as_mut() {
                *accepted = sequence;
            }
            json!({
                "protocol_version": 1,
                "message_id": Uuid::new_v4(),
                "type": "projection_ack",
                "created_at": 1_787_596_800_000_i64,
                "payload": {
                    "epoch": epoch,
                    "accepted_sequence": sequence,
                    "status": "applied"
                }
            })
        }
    }

    #[test]
    fn production_relay_is_the_default() {
        const PRODUCTION_RELAY_ORIGIN: &str = "https://relay.ditchnow.nl";
        assert_eq!(
            configured_relay_origin(None, PRODUCTION_RELAY_ORIGIN).as_deref(),
            Ok(PRODUCTION_RELAY_ORIGIN)
        );
        assert_eq!(
            configured_relay_origin(Some("   "), PRODUCTION_RELAY_ORIGIN).as_deref(),
            Ok(PRODUCTION_RELAY_ORIGIN)
        );
    }

    #[test]
    fn machine_name_prefers_an_explicit_normalized_override() {
        assert_eq!(
            preferred_machine_name(Some("  Studio Mac  \n"), Some("System Mac")),
            "Studio Mac"
        );
    }

    #[test]
    fn machine_name_uses_the_system_name_and_has_a_safe_fallback() {
        assert_eq!(
            preferred_machine_name(Some("  "), Some("  Metin’s MacBook Pro\n")),
            "Metin’s MacBook Pro"
        );
        assert_eq!(preferred_machine_name(None, None), "This Mac");
    }

    #[test]
    fn machine_name_respects_the_relay_contract_limit() {
        let long_name = "m".repeat(MAX_MACHINE_NAME_CHARS + 1);
        assert_eq!(
            preferred_machine_name(None, Some(&long_name))
                .chars()
                .count(),
            MAX_MACHINE_NAME_CHARS
        );
    }

    #[test]
    fn secure_environment_override_is_normalized() {
        assert_eq!(
            configured_relay_origin(
                Some(" https://staging-relay.ditchnow.nl/ "),
                "https://relay.ditchnow.nl",
            )
            .as_deref(),
            Ok("https://staging-relay.ditchnow.nl")
        );
        assert_eq!(
            configured_relay_origin(Some("http://localhost:8787/"), "https://relay.ditchnow.nl",)
                .as_deref(),
            Ok("http://localhost:8787")
        );
    }

    #[test]
    fn unsafe_or_non_origin_overrides_are_rejected() {
        for value in [
            "relay.ditchnow.nl",
            "http://relay.ditchnow.nl",
            "https://user@example.test",
            "https://example.test/path",
            "https://example.test?environment=staging",
            "https://example.test/#fragment",
        ] {
            assert!(
                configured_relay_origin(Some(value), "https://relay.ditchnow.nl").is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn device_roster_and_revoke_paths_are_machine_scoped() {
        let machine_id = Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap();
        let device_id = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        assert_eq!(
            machine_devices_path(machine_id),
            "/v1/machines/22222222-2222-4222-8222-222222222222/devices"
        );
        assert_eq!(
            machine_device_path(machine_id, device_id),
            "/v1/machines/22222222-2222-4222-8222-222222222222/devices/11111111-1111-4111-8111-111111111111"
        );
    }

    #[test]
    fn transmitted_projection_frame_keeps_timestamp_as_json_integer() {
        let project = ProjectProjection {
            project_id: Uuid::new_v4(),
            name: "Fixture project".into(),
            status: "idle".into(),
            running_count: 0,
            waiting_count: 0,
            ready_count: 0,
            failed_count: 0,
            attention_count: 0,
            last_activity_at: 1_787_596_800_123,
            projection_version: 1,
        };
        let key = ("project".to_owned(), project.project_id.to_string());
        let snapshot = ProjectionSnapshot {
            records: [(key, serde_json::to_value(project).unwrap())]
                .into_iter()
                .collect(),
        };
        let transport = ProjectionTransport::new(Uuid::new_v4(), Uuid::new_v4(), snapshot);
        let json: Value = serde_json::from_str(&transport.queue[1].frame).unwrap();
        assert_eq!(json["type"], "projection_snapshot_record");
        assert_eq!(
            json.pointer("/payload/record/last_activity_at")
                .and_then(Value::as_i64),
            Some(1_787_596_800_123)
        );
    }

    #[test]
    fn active_device_roster_uses_integer_timestamps_and_rejects_tombstones() {
        let device_id = Uuid::new_v4();
        let signing_public_key = "BEpzUDasQTezvS3DbSVeuAqDbQ7lL27drGz6T2wFxigZ7q0Cq5KV0ZgV3gGsCWAQq6d1GgTTArxB7r-tF3UsNeY";
        let agreement_public_key = "BKLLvBAvoHGRc8aMEJgGlYX6M_FcwKQqu8B5czZ-XzsEoEvQgqAK7cDxfc1NpOQVmXFm6pfcyXSoSm5bfEoo05E";
        let response = json!({
            "protocol_version": 1,
            "devices": [{
                "id": device_id,
                "name": "iPhone",
                "platform": "ios",
                "state": "active",
                "signing_public_key": signing_public_key,
                "agreement_public_key": agreement_public_key,
                "key_version": 3,
                "authorized_at": 1_787_596_700_000_i64,
                "last_seen_at": 1_787_596_800_123_i64
            }]
        });
        let devices = active_devices_from_response(&response).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, device_id);
        assert_eq!(
            devices[0]
                .last_seen_at
                .map(|value| value.timestamp_millis()),
            Some(1_787_596_800_123)
        );

        let mut revoked = response;
        revoked["devices"][0]["state"] = json!("revoked");
        assert!(active_devices_from_response(&revoked).is_err());
    }

    fn snapshot_with_projects(count: usize) -> ProjectionSnapshot {
        let records = (0..count)
            .map(|index| {
                let id = Uuid::new_v4();
                (
                    ("project".to_owned(), id.to_string()),
                    json!({
                        "project_id": id,
                        "name": format!("Project {index}"),
                        "status": "idle",
                        "running_count": 0,
                        "waiting_count": 0,
                        "ready_count": 0,
                        "failed_count": 0,
                        "attention_count": 0,
                        "last_activity_at": 1_787_596_800_123_i64,
                        "projection_version": 1
                    }),
                )
            })
            .collect();
        ProjectionSnapshot { records }
    }

    #[test]
    fn staged_snapshot_waits_for_each_ack_and_commits_at_the_end() {
        let snapshot = snapshot_with_projects(2);
        let mut transport = ProjectionTransport::new(Uuid::new_v4(), Uuid::new_v4(), snapshot);
        assert_eq!(transport.queue.len(), 4);

        let mut frame_types = Vec::new();
        let mut committed = false;
        while let Some(work) = transport.next_frame() {
            let frame: Value = serde_json::from_str(&work.frame).unwrap();
            frame_types.push(frame["type"].as_str().unwrap().to_owned());
            let epoch = work.epoch;
            let sequence = work.sequence;
            match transport.acknowledge(epoch, sequence, "applied").unwrap() {
                ProjectionAckOutcome::Committed(snapshot) => {
                    committed = true;
                    assert_eq!(snapshot.records.len(), 2);
                }
                ProjectionAckOutcome::Advanced => {}
                ProjectionAckOutcome::Ignored => panic!("current ACK must advance"),
            }
        }
        assert!(committed);
        assert_eq!(
            frame_types,
            [
                "projection_snapshot_begin",
                "projection_snapshot_record",
                "projection_snapshot_record",
                "projection_snapshot_commit"
            ]
        );
    }

    #[test]
    fn ordinary_change_emits_only_the_changed_projection() {
        let original = snapshot_with_projects(2);
        let mut transport =
            ProjectionTransport::new(Uuid::new_v4(), Uuid::new_v4(), original.clone());
        while let Some(work) = transport.next_frame() {
            let epoch = work.epoch;
            let sequence = work.sequence;
            transport.acknowledge(epoch, sequence, "applied").unwrap();
        }
        let mut changed = original;
        changed.records.values_mut().next().unwrap()["status"] = json!("running");
        transport.queue_incremental(changed);
        assert_eq!(transport.queue.len(), 1);
        let frame: Value = serde_json::from_str(&transport.queue[0].frame).unwrap();
        assert_eq!(frame["type"], "projection_update");
        assert_eq!(frame["payload"]["kind"], "project");
    }

    #[test]
    fn stale_ack_is_ignored_without_changing_current_epoch() {
        let mut transport =
            ProjectionTransport::new(Uuid::new_v4(), Uuid::new_v4(), snapshot_with_projects(1));
        let current_epoch = transport.epoch;
        assert!(matches!(
            transport.acknowledge(Uuid::new_v4(), 1, "applied").unwrap(),
            ProjectionAckOutcome::Ignored
        ));
        assert_eq!(transport.epoch, current_epoch);
        assert_eq!(transport.acknowledged_sequence, 0);
    }

    #[test]
    fn deterministic_relay_keeps_old_projection_until_snapshot_commit() {
        let old = snapshot_with_projects(1);
        let replacement = snapshot_with_projects(2);
        let mut relay = FakeProjectionRelay {
            active: old.clone(),
            staging: None,
        };
        let mut transport =
            ProjectionTransport::new(Uuid::new_v4(), Uuid::new_v4(), replacement.clone());

        for _ in 0..3 {
            let work = transport.next_frame().unwrap();
            let epoch = work.epoch;
            let sequence = work.sequence;
            let response = relay.receive(&work.frame);
            assert_eq!(response["type"], "projection_ack");
            transport.acknowledge(epoch, sequence, "applied").unwrap();
        }
        assert_eq!(relay.active, old);

        let work = transport.next_frame().unwrap();
        let epoch = work.epoch;
        let sequence = work.sequence;
        relay.receive(&work.frame);
        assert!(matches!(
            transport.acknowledge(epoch, sequence, "applied").unwrap(),
            ProjectionAckOutcome::Committed(_)
        ));
        assert_eq!(relay.active.records.len(), replacement.records.len());
    }

    #[test]
    fn deterministic_relay_requests_reconciliation_for_a_gap() {
        let mut relay = FakeProjectionRelay::default();
        let transport =
            ProjectionTransport::new(Uuid::new_v4(), Uuid::new_v4(), snapshot_with_projects(1));
        let mut skipped: Value = serde_json::from_str(&transport.queue[1].frame).unwrap();
        skipped["payload"]["sequence"] = json!(2);
        let response = relay.receive(&skipped.to_string());
        assert_eq!(response["type"], "projection_gap");
        assert_eq!(response["payload"]["expected_sequence"], 1);
        assert_eq!(response["payload"]["full_snapshot_required"], true);
    }

    #[test]
    fn incremental_projection_work_is_bounded_to_thirty_two_records() {
        let mut transport = ProjectionTransport::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ProjectionSnapshot::default(),
        );
        while let Some(work) = transport.next_frame() {
            let epoch = work.epoch;
            let sequence = work.sequence;
            transport.acknowledge(epoch, sequence, "applied").unwrap();
        }
        transport.queue_incremental(snapshot_with_projects(40));
        assert_eq!(transport.queue.len(), 32);
        assert!(transport.dirty);
    }
}
