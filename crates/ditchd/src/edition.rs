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

pub fn handle_request(_request: ClientRequest, _state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    protocol_error(
        "unsupported_request",
        "request is not implemented by the Community runtime",
    )
}

pub fn task_turn_allowed(_state: &RuntimeState, _id: ditch_core::TaskId) -> bool {
    true
}

pub fn restricted_agent(_state: &RuntimeState, _id: ditch_core::AgentId) -> bool {
    false
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
