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

pub fn handle_request(_request: ClientRequest, _state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    protocol_error(
        "unsupported_request",
        "request is not implemented by the Community runtime",
    )
}
