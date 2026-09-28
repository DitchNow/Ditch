use super::{ClientRequest, DitchStore, RuntimeState, ServerResponse, protocol_error};
use std::io;
use std::sync::{Arc, Mutex};

pub const RUNTIME_EDITION: &str = "community";

/// Zero-sized Community composition. Commercial-only providers cannot be
/// constructed because they are not dependencies of this repository.
pub struct State;

impl State {
    pub fn new() -> Self {
        Self
    }
}

pub fn initialize_store(_store: &mut DitchStore) -> io::Result<()> {
    Ok(())
}

pub fn extend_runtime_capabilities(_state: &State, _capabilities: &mut Vec<String>) {}

pub fn after_broadcast(_state: &mut RuntimeState) {}

pub fn start(_state: Arc<Mutex<RuntimeState>>) {}

/// Community keeps the provider-neutral upgrade endpoint implemented by the
/// shared runtime. Commercial overrides it so capability gating and the UI
/// consume the same refreshed entitlement snapshot.
pub fn commercial_entitlement(_state: Arc<Mutex<RuntimeState>>) -> Option<ServerResponse> {
    None
}

// The Community runtime has no proprietary capabilities to enable. Its UI
// receives the authoritative activation summary and offers the Commercial app.
pub fn accept_commercial_entitlement(
    _state: Arc<Mutex<RuntimeState>>,
    _summary: ditch_upgrade::EntitlementSummary,
) {
}

pub fn handle_request(_request: ClientRequest, _state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    protocol_error(
        "unsupported_request",
        "request is not implemented by the Community runtime",
    )
}

pub fn task_turn_allowed(_state: &RuntimeState, _id: ditch_core::TaskId) -> bool {
    true
}

pub fn restricted_agent(state: &RuntimeState, id: ditch_core::AgentId) -> bool {
    state.agent_coordinator_group(id).is_some()
}

pub fn report_ready(
    _state: &RuntimeState,
    _id: ditch_core::TaskId,
    _summary: Option<&str>,
) -> bool {
    false
}

pub fn shutdown(_state: &mut RuntimeState) {}

pub fn authorize_client(
    _stream: &std::os::unix::net::UnixStream,
    _request: &ClientRequest,
    _state: &Arc<Mutex<RuntimeState>>,
) -> Result<(), String> {
    Ok(())
}
