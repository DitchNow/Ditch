//! Community-owned, statically composed tool integrations. The adapter is a
//! transport client; this registry owns all execution authority and lifecycle.
mod executor;
pub mod local_transport;
pub mod native;
mod peekaboo;

use crate::RuntimeState;
use ditch_core::tool_integrations::*;
use ditch_core::{AgentExecutionProfile, AgentId};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, AtomicI32, Ordering},
    mpsc,
};
use uuid::Uuid;

thread_local! { static REMOTE_ORIGIN: Cell<bool> = const { Cell::new(false) }; }
#[allow(dead_code)] // Commercial routes mobile-origin commands through this shared guard.
pub fn with_remote_origin<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            REMOTE_ORIGIN.set(self.0);
        }
    }
    let _reset = Reset(REMOTE_ORIGIN.replace(true));
    f()
}

pub trait ToolIntegration: Send + Sync {
    fn id(&self) -> &'static str;
    fn tools(&self) -> Vec<String>;
    fn invoke(
        &self,
        request: &Value,
        cancelled: &Arc<AtomicBool>,
        lock: Option<std::fs::File>,
    ) -> Value;
}

pub struct Lease {
    pub agent_id: AgentId,
    pub run_id: Uuid,
    pub cancelled: Arc<AtomicBool>,
    state: Weak<Mutex<RuntimeState>>,
    provider: Arc<dyn ToolIntegration>,
    socket: PathBuf,
    credential: String,
    credential_path: PathBuf,
    cli: PathBuf,
    adapter_pid: AtomicI32,
    tools_registered: AtomicBool,
    binding: Mutex<(Option<String>, Option<String>)>,
    completed: Mutex<HashMap<Uuid, Value>>,
    vision: bool,
    visualizer: bool,
}
static LEASES: OnceLock<Mutex<HashMap<AgentId, Arc<Lease>>>> = OnceLock::new();
static SUBSCRIBERS: OnceLock<Mutex<Vec<mpsc::SyncSender<ToolIntegrationEvent>>>> = OnceLock::new();
fn leases() -> &'static Mutex<HashMap<AgentId, Arc<Lease>>> {
    LEASES.get_or_init(Default::default)
}

impl Lease {
    pub fn authenticated_adapter(&self) -> std::io::Result<i32> {
        let pid = self.adapter_pid.load(Ordering::Acquire);
        if self.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("Ditch desktop turn was revoked during startup; no model turn was started"));
        }
        if pid == 0 {
            return Err(std::io::Error::other("Ditch adapter did not authenticate with the runtime; no model turn was started"));
        }
        // Signal zero checks existence without signalling or changing the peer.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return Err(std::io::Error::other("Ditch execution adapter exited during startup; start a fresh turn"));
        }
        Ok(pid)
    }
    pub fn confirm_registered(&self, expected_pid: i32) -> std::io::Result<()> {
        if self.authenticated_adapter()? != expected_pid {
            return Err(std::io::Error::other("Ditch execution adapter changed during discovery; no model turn was started"));
        }
        self.tools_registered.store(true, Ordering::Release);
        Ok(())
    }
    pub fn config(&self) -> Value {
        json!({"command":self.cli,"args":["tools","serve","--stdio"],
            "env":{"DITCH_TOOL_SOCKET":self.socket,"DITCH_TOOL_CREDENTIAL_FILE":self.credential_path},
            "enabled_tools":self.provider.tools(),"startup_timeout_sec":5,"tool_timeout_sec":20})
    }
    pub fn bind_thread(&self, thread: &str) {
        self.binding.lock().unwrap().0 = Some(thread.into());
    }
    pub fn bind_turn(&self, turn: &str) {
        self.binding.lock().unwrap().1 = Some(turn.into());
    }
    pub fn revoke(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn drain(&self) -> bool {
        self.revoke();
        let drained = native::drain_scope(&self.cancelled);
        if drained {
            native::forget_session(self.run_id);
            self.completed.lock().unwrap().clear();
        }
        drained
    }
    fn bound(&self) -> bool {
        let b = self.binding.lock().unwrap();
        b.0.is_some() && b.1.is_some()
    }
    fn event(&self, operation_id: Uuid, tool: &str, status: &str) {
        let event = ToolIntegrationEvent {
            agent_id: self.agent_id,
            operation_id,
            integration: self.provider.id().into(),
            tool: tool.into(),
            state: status.into(),
        };
        SUBSCRIBERS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .retain(|tx| tx.try_send(event.clone()).is_ok());
    }
}

pub fn lease_for(id: AgentId) -> Option<Arc<Lease>> {
    leases()
        .lock()
        .unwrap()
        .get(&id)
        .filter(|l| !l.cancelled.load(Ordering::Acquire))
        .cloned()
}

/// Called before launching a normal user turn. An unavailable integration must
/// never prevent ordinary chat; it simply has no adapter entry in that turn.
pub fn prepare(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: Uuid,
    profile: &AgentExecutionProfile,
    turn_options: TurnOptions,
    purpose: InvocationPurpose,
) -> Option<Arc<Lease>> {
    if !revoke(agent_id) {
        return None;
    }
    let (remote, paths) = {
        let state = state.lock().unwrap();
        (state.remote_runtime, state.paths.clone())
    };
    let origin = if REMOTE_ORIGIN.get() {
        InvocationOrigin::Remote
    } else {
        InvocationOrigin::Local
    };
    if !desktop_eligible(profile, purpose, origin, remote, native::available()) {
        return None;
    }
    let model_status = {
        let locked = state.lock().unwrap();
        let endpoint = locked
            .store
            .ollama_endpoint(profile.inference_endpoint_id?)
            .ok()??;
        endpoint.model_status.get(profile.model.as_ref()?)?.clone()
    };
    if !model_status.tools_declared {
        return None;
    }
    let readiness = native::invoke(
        &json!({"sessionId":"status","operationId":Uuid::new_v4(),"tool":"desktop_status","arguments":{}}),
        &Arc::new(AtomicBool::new(false)),
        None,
    );
    if readiness["result"]["activeDesktop"] != true {
        return None;
    }
    // The signed StatusHost and bundled CLI are siblings in the runtime bundle.
    let cli = std::env::current_exe().ok()?.parent()?.join("ditch_cli");
    if !cli.is_file() {
        return None;
    }
    let socket = PathBuf::from("/tmp").join(format!(
        "ditch-tools-{}-{}.sock",
        unsafe { libc::geteuid() },
        Uuid::new_v4().simple()
    ));
    let credential_path = paths
        .data_dir
        .join(format!("tools-{}.token", Uuid::new_v4().simple()));
    let lease = Arc::new(Lease {
        agent_id,
        run_id,
        state: Arc::downgrade(state),
        provider: Arc::new(peekaboo::Peekaboo),
        socket,
        cli,
        credential_path,
        credential: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        cancelled: Arc::new(AtomicBool::new(false)),
        adapter_pid: AtomicI32::new(0),
        tools_registered: AtomicBool::new(false),
        binding: Mutex::new((None, None)),
        completed: Mutex::new(HashMap::new()),
        vision: model_status.vision_declared,
        visualizer: turn_options.computer_use_visualizer,
    });
    if local_transport::start(lease.clone()).is_err() {
        return None;
    }
    leases().lock().unwrap().insert(agent_id, lease.clone());
    Some(lease)
}

pub fn revoke(id: AgentId) -> bool {
    let lease = leases().lock().unwrap().get(&id).cloned();
    let Some(lease) = lease else { return true };
    if !lease.drain() {
        return false;
    }
    let mut table = leases().lock().unwrap();
    if table.get(&id).is_some_and(|l| l.run_id == lease.run_id) {
        table.remove(&id);
    }
    true
}

pub fn has_lease(id: AgentId) -> bool {
    leases().lock().unwrap().contains_key(&id)
}
pub fn revoke_run(id: AgentId, run: Uuid) {
    let lease = leases()
        .lock()
        .unwrap()
        .get(&id)
        .filter(|l| l.run_id == run)
        .cloned();
    if let Some(lease) = lease {
        if lease.drain() {
            let mut table = leases().lock().unwrap();
            if table.get(&id).is_some_and(|l| l.run_id == run) {
                table.remove(&id);
            }
        }
    }
}

pub fn revoke_all() {
    // Called on the native main thread on lock/sleep. Never block that thread
    // while waiting for a Swift task; invalidate authority synchronously.
    let mut table = leases().lock().unwrap();
    for lease in table.values() {
        lease.revoke();
    }
    table.clear();
}

pub fn subscribe() -> mpsc::Receiver<ToolIntegrationEvent> {
    let (tx, rx) = mpsc::sync_channel(32);
    SUBSCRIBERS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push(tx);
    rx
}

pub fn status(agent: Option<AgentId>) -> ToolIntegrationStatus {
    let lease = agent.and_then(lease_for);
    let mut status = ToolIntegrationStatus {
        integration: "peekaboo".into(),
        native_available: native::available(),
        eligible: lease.is_some(),
        adapter_connected: lease.as_ref().is_some_and(|l| l.authenticated_adapter().is_ok()),
        tools_registered: lease.as_ref().is_some_and(|l| l.tools_registered.load(Ordering::Acquire)),
        state: if native::available() {
            "requiresFullAccess"
        } else {
            "unavailable"
        }
        .into(),
        host_name: native::host_name(),
        ..Default::default()
    };
    if native::available() {
        let result = native::invoke(
            &json!({"sessionId":"status","operationId":Uuid::new_v4(),"tool":"desktop_status","arguments":{}}),
            &Arc::new(AtomicBool::new(false)),
            None,
        );
        if result["ok"] == true {
            status.accessibility = result["result"]["accessibility"] == true;
            status.screen_recording = result["result"]["screenRecording"] == true;
            status.post_event = result["result"]["postEvent"] == true;
            if result["result"]["activeDesktop"] != true {
                status.state = "desktopInactive".into();
            } else if lease.is_some() {
                status.state = if status.accessibility && status.screen_recording {
                    if status.tools_registered && status.adapter_connected { "ready" } else { "connecting" }
                } else {
                    "permissionRequired"
                }
                .into();
            }
        } else {
            status.state = "busy".into();
        }
    }
    status
}
