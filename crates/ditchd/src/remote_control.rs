use super::{RuntimeState, protocol_error};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{TimeZone, Utc};
use ditch_protocol::{RemoteControlStatus, RemoteDeviceSummary, RemotePairing, ServerResponse};
use ditch_remote::{
    Aad, ConfirmationClass, PairwiseContext, RemoteCommand, RemoteCommandType, RemoteProjector,
    SessionPromptPayload, SessionStartPayload, SessionTargetPayload, TranscriptQueryPayload,
    decrypt, encrypt,
};
use ditch_remote::{IdentityStore, KeychainIdentityStore, MachineIdentity, canonical_request};
use ditch_store::{RemoteDeviceRecord, RemoteMachineRecord};
use rand_core::{OsRng, RngCore};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, Sender, TryRecvError},
};
use std::thread;
use std::time::{Duration as StdDuration, Instant};
use tungstenite::{Message, stream::MaybeTlsStream};
use url::Url;
use uuid::Uuid;

const PRODUCTION_RELAY_ORIGIN: &str = "https://relay.ditchnow.nl";

pub(super) struct RemoteController {
    relay_origin: Option<String>,
    pairings: HashMap<Uuid, RemotePairing>,
    authenticated_socket_live: bool,
    connection_started: bool,
    outbound: Option<Sender<String>>,
}

impl RemoteController {
    pub(super) fn new() -> Self {
        let relay_origin =
            configured_relay_origin(std::env::var("DITCH_REMOTE_RELAY_ORIGIN").ok().as_deref());
        if let Err(error) = &relay_origin {
            eprintln!("ditchd ignored invalid DITCH_REMOTE_RELAY_ORIGIN: {error}");
        }
        Self {
            relay_origin: relay_origin.ok(),
            pairings: HashMap::new(),
            authenticated_socket_live: false,
            connection_started: false,
            outbound: None,
        }
    }
}

fn configured_relay_origin(override_value: Option<&str>) -> Result<String, String> {
    let candidate = override_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(PRODUCTION_RELAY_ORIGIN);
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

pub(super) fn status(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
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
        configured: state.remote.relay_origin.is_some(),
        enabled: machine.as_ref().is_some_and(|value| value.enabled),
        machine_id: machine.as_ref().map(|value| value.machine_id),
        owner_id: machine.as_ref().and_then(|value| value.owner_id),
        machine_name: machine
            .as_ref()
            .map(|value| value.name.clone())
            .unwrap_or_else(machine_name),
        online: state.remote.authenticated_socket_live,
        relay_origin: state.remote.relay_origin.clone(),
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
    let response = signed_request(&origin, &identity, "GET", "/v1/devices", b"")?;
    let devices = active_devices_from_response(&response)?;
    state
        .lock()
        .map_err(|_| "runtime state lock poisoned".to_owned())?
        .store
        .replace_remote_devices(&devices)
        .map_err(|error| error.to_string())
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
            let state = device
                .get("state")
                .and_then(Value::as_str)
                .ok_or("relay device omitted state")?;
            if state != "active" {
                return Err("relay returned a non-active device".to_owned());
            }
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
                signing_public_key: device
                    .get("signing_public_key")
                    .and_then(Value::as_str)
                    .ok_or("relay device omitted signing_public_key")?
                    .to_owned(),
                agreement_public_key: device
                    .get("agreement_public_key")
                    .and_then(Value::as_str)
                    .ok_or("relay device omitted agreement_public_key")?
                    .to_owned(),
                key_version: device
                    .get("key_version")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or("relay device omitted key_version")?,
                state: state.to_owned(),
                last_seen_at,
            })
        })
        .collect()
}

pub(super) fn create_pairing(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    let (origin, identity, machine_name) = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(origin) = state.remote.relay_origin.clone() else {
            return protocol_error(
                "remote_not_configured",
                "The remote relay configuration is invalid.",
            );
        };
        let identity = match ensure_identity(&mut state) {
            Ok(value) => value,
            Err(error) => return protocol_error("remote_identity_failed", error),
        };
        let name = machine_name();
        (origin, identity, name)
    };
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
        .remote
        .pairings
        .insert(pairing.pairing_id, pairing.clone());
    ServerResponse::RemotePairing(pairing)
}

pub(super) fn get_pairing(state: Arc<Mutex<RuntimeState>>, pairing_id: Uuid) -> ServerResponse {
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
        .remote
        .pairings
        .insert(pairing_id, pairing.clone());
    ServerResponse::RemotePairing(pairing)
}

pub(super) fn confirm_pairing(state: Arc<Mutex<RuntimeState>>, pairing_id: Uuid) -> ServerResponse {
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
    guard.remote.pairings.insert(pairing_id, pairing.clone());
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
    let path = format!("/v1/devices/{device_id}");
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
    state.remote.authenticated_socket_live = false;
    if let Err(error) = state.store.set_remote_enabled(false) {
        return protocol_error("remote_store_failed", error.to_string());
    }
    ServerResponse::Accepted
}

fn ensure_identity(state: &mut RuntimeState) -> Result<MachineIdentity, String> {
    let existing = state
        .store
        .remote_machine()
        .map_err(|error| error.to_string())?;
    let machine_id = existing
        .as_ref()
        .map(|value| value.machine_id)
        .unwrap_or_else(Uuid::new_v4);
    let keychain = KeychainIdentityStore;
    let identity = match keychain
        .load(machine_id)
        .map_err(|error| error.to_string())?
    {
        Some(value) => value,
        None => {
            let value = MachineIdentity::generate(machine_id);
            keychain.save(&value).map_err(|error| error.to_string())?;
            value
        }
    };
    let record = RemoteMachineRecord {
        machine_id,
        owner_id: existing.as_ref().and_then(|value| value.owner_id),
        name: existing
            .as_ref()
            .map(|value| value.name.clone())
            .unwrap_or_else(machine_name),
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

fn remote_identity(
    state: &Arc<Mutex<RuntimeState>>,
) -> Result<(String, MachineIdentity), ServerResponse> {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(origin) = state.remote.relay_origin.clone() else {
        return Err(protocol_error(
            "remote_not_configured",
            "The remote relay configuration is invalid.",
        ));
    };
    let identity = ensure_identity(&mut state)
        .map_err(|error| protocol_error("remote_identity_failed", error))?;
    Ok((origin, identity))
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
fn machine_name() -> String {
    std::env::var("DITCH_MACHINE_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "This Mac".to_owned())
}

pub(super) fn start_connection(state: Arc<Mutex<RuntimeState>>) {
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
        if !enabled || guard.remote.connection_started || guard.remote.relay_origin.is_none() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        guard.remote.outbound = Some(sender);
        guard.remote.connection_started = true;
        receiver
    };
    thread::spawn(move || connection_loop(state, receiver));
}

pub(super) fn publish_projection(state: &mut RuntimeState) {
    if !state.remote.authenticated_socket_live {
        return;
    }
    let Some(sender) = state.remote.outbound.clone() else {
        return;
    };
    for frame in projection_frames(state, false) {
        let _ = sender.send(frame);
    }
}

fn projection_frames(state: &mut RuntimeState, new_epoch: bool) -> Vec<String> {
    let Some(mut machine) = state.store.remote_machine().ok().flatten() else {
        return Vec::new();
    };
    let Some(owner_id) = machine.owner_id else {
        return Vec::new();
    };
    if !machine.enabled {
        return Vec::new();
    }
    if new_epoch {
        machine.projection_epoch = Uuid::new_v4();
        machine.projection_sequence = 0;
    }
    let agents = state
        .agents
        .values()
        .map(|record| record.run.clone())
        .collect::<Vec<_>>();
    let projects = state
        .projects
        .values()
        .map(|project| {
            RemoteProjector::project(
                project,
                &agents,
                &state.attention,
                machine.projection_sequence + 1,
            )
        })
        .collect::<Vec<_>>();
    let sessions = agents
        .iter()
        .map(|run| RemoteProjector::session(run, &state.attention, machine.projection_sequence + 1))
        .collect::<Vec<_>>();
    let attention = state
        .attention
        .iter()
        .map(|item| RemoteProjector::attention(item, machine.projection_sequence + 1))
        .collect::<Vec<_>>();
    let mut records: Vec<(&str, Value)> = Vec::new();
    records.push(("full_snapshot", json!({})));
    records.extend(projects.iter().filter_map(|value| {
        serde_json::to_value(value)
            .ok()
            .map(|value| ("project", value))
    }));
    records.extend(sessions.iter().filter_map(|value| {
        serde_json::to_value(value)
            .ok()
            .map(|value| ("session", value))
    }));
    records.extend(attention.iter().filter_map(|value| {
        serde_json::to_value(value)
            .ok()
            .map(|value| ("attention", value))
    }));
    let mut frames = Vec::with_capacity(records.len());
    for (kind, record) in records {
        machine.projection_sequence += 1;
        frames.push(projection_frame(
            owner_id,
            machine.machine_id,
            machine.projection_epoch,
            machine.projection_sequence,
            kind,
            record,
            Utc::now().timestamp_millis(),
        ));
    }
    let _ = state.store.upsert_remote_machine(&machine);
    let _ = state.store.replace_remote_projection_cache(
        machine.projection_epoch,
        machine.projection_sequence,
        &projects,
        &sessions,
        &attention,
    );
    frames
}

fn projection_frame(
    owner_id: Uuid,
    machine_id: Uuid,
    epoch: Uuid,
    sequence: u64,
    kind: &str,
    record: Value,
    created_at: i64,
) -> String {
    json!({
        "protocol_version": 1,
        "message_id": Uuid::new_v4(),
        "type": "projection_update",
        "created_at": created_at,
        "payload": {
            "owner_id": owner_id,
            "machine_id": machine_id,
            "epoch": epoch,
            "sequence": sequence,
            "kind": kind,
            "record": record,
        }
    })
    .to_string()
}

fn connection_loop(state: Arc<Mutex<RuntimeState>>, receiver: Receiver<String>) {
    let mut backoff = 1_u64;
    loop {
        let enabled = state
            .lock()
            .ok()
            .and_then(|guard| guard.store.remote_machine().ok().flatten())
            .is_some_and(|machine| machine.enabled);
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
                    guard.remote.authenticated_socket_live = false;
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
        guard.remote.authenticated_socket_live = false;
        guard.remote.connection_started = false;
        guard.remote.outbound = None;
    }
}

fn connect_once(
    state: &Arc<Mutex<RuntimeState>>,
    receiver: &Receiver<String>,
) -> Result<(), String> {
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
            let _ = stream.set_read_timeout(Some(StdDuration::from_secs(1)));
        }
        MaybeTlsStream::Rustls(stream) => {
            let _ = stream
                .get_mut()
                .set_read_timeout(Some(StdDuration::from_secs(1)));
        }
        _ => {}
    }
    let initial = {
        let mut guard = state
            .lock()
            .map_err(|_| "runtime lock poisoned".to_owned())?;
        guard.remote.authenticated_socket_live = true;
        projection_frames(&mut guard, true)
    };
    for frame in initial {
        socket
            .send(Message::Text(frame.into()))
            .map_err(|error| error.to_string())?;
    }
    let mut last_heartbeat = Instant::now();
    loop {
        loop {
            match receiver.try_recv() {
                Ok(frame) => socket
                    .send(Message::Text(frame.into()))
                    .map_err(|error| error.to_string())?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if last_heartbeat.elapsed() >= StdDuration::from_secs(20) {
            socket.send(Message::Text(json!({ "protocol_version": 1, "message_id": Uuid::new_v4(), "type": "heartbeat", "created_at": Utc::now().timestamp_millis(), "payload": {} }).to_string().into())).map_err(|error| error.to_string())?;
            last_heartbeat = Instant::now();
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                handle_socket_frame(state, &identity, &mut socket, text.as_str())?
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
    }
}

fn handle_socket_frame<S: std::io::Read + std::io::Write>(
    state: &Arc<Mutex<RuntimeState>>,
    identity: &MachineIdentity,
    socket: &mut tungstenite::WebSocket<S>,
    text: &str,
) -> Result<(), String> {
    let frame: Value = serde_json::from_str(text).map_err(|_| "invalid relay frame".to_owned())?;
    if frame.get("protocol_version").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported relay protocol".to_owned());
    }
    if frame.get("type").and_then(Value::as_str) != Some("command") {
        return Ok(());
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
    socket.send(Message::Text(json!({ "protocol_version": 1, "message_id": result_message_id, "type": "command_result", "created_at": created_at, "payload": { "command_id": command.command_id, "device_id": command.device_id, "expires_at": expires_at, "encrypted": encrypted } }).to_string().into())).map_err(|error| error.to_string())
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
        PRODUCTION_RELAY_ORIGIN, active_devices_from_response, configured_relay_origin,
        projection_frame,
    };
    use ditch_remote::ProjectProjection;
    use serde_json::{Value, json};
    use uuid::Uuid;

    #[test]
    fn production_relay_is_the_default() {
        assert_eq!(
            configured_relay_origin(None).as_deref(),
            Ok(PRODUCTION_RELAY_ORIGIN)
        );
        assert_eq!(
            configured_relay_origin(Some("   ")).as_deref(),
            Ok(PRODUCTION_RELAY_ORIGIN)
        );
    }

    #[test]
    fn secure_environment_override_is_normalized() {
        assert_eq!(
            configured_relay_origin(Some(" https://staging-relay.ditchnow.nl/ ")).as_deref(),
            Ok("https://staging-relay.ditchnow.nl")
        );
        assert_eq!(
            configured_relay_origin(Some("http://localhost:8787/")).as_deref(),
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
            assert!(configured_relay_origin(Some(value)).is_err(), "{value}");
        }
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
        let frame = projection_frame(
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            2,
            "project",
            serde_json::to_value(project).unwrap(),
            1_787_596_800_456,
        );
        let json: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(
            json.pointer("/payload/record/last_activity_at")
                .and_then(Value::as_i64),
            Some(1_787_596_800_123)
        );
    }

    #[test]
    fn active_device_roster_uses_integer_timestamps_and_rejects_tombstones() {
        let device_id = Uuid::new_v4();
        let response = json!({
            "protocol_version": 1,
            "devices": [{
                "id": device_id,
                "name": "iPhone",
                "state": "active",
                "signing_public_key": "signing",
                "agreement_public_key": "agreement",
                "key_version": 3,
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
}
