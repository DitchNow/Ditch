//! Shared local and SSH Codex App Server adapter. The daemon owns each process.

use chrono::Utc;
use ditch_core::{
    AgentExecutionProfile, AgentId, PermissionActionKind, PermissionRequest, ProjectId,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, Weak};
use uuid::Uuid;

const INITIALIZE_ID: i64 = 1;
const THREAD_ID: i64 = 2;
const TURN_ID: i64 = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionDecision {
    ApproveOnce,
    ApproveForSession,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    ThreadReady(String),
    Title(String),
    ProviderUsage(Value),
    SkillsChanged,
    AssistantDelta(String),
    TurnStarted,
    Action(String),
    AssistantMessage(String),
    ToolMessage(String),
    ApprovalRequested(PermissionRequest),
    TurnCompleted {
        status: String,
        error: Option<String>,
    },
    Diagnostic(String),
    Failed(String),
}

#[derive(Clone)]
pub struct ActiveTurn {
    writer: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<Uuid, PendingApproval>>>,
}

#[derive(Clone)]
struct PendingApproval {
    rpc_id: Value,
    kind: ApprovalKind,
}

#[derive(Clone)]
enum ApprovalKind {
    Decision,
    Permissions(Value),
}

pub struct SpawnedTurn {
    pub child: Arc<Mutex<Child>>,
    pub control: ActiveTurn,
}

pub struct Launch<'a> {
    pub restricted: bool,
    pub protected_roots: Vec<std::path::PathBuf>,
    pub remote: bool,
    pub extra_roots: &'a [std::path::PathBuf],
    pub on_spawn: Option<&'a dyn Fn(i32) -> io::Result<()>>,
    pub binary: &'a str,
    pub cwd: &'a Path,
    pub project_id: ProjectId,
    pub agent_id: AgentId,
    pub prompt: &'a str,
    pub resume_thread: Option<&'a str>,
    pub execution_profile: &'a AgentExecutionProfile,
    pub codex_home: Option<OsString>,
}

impl ActiveTurn {
    pub fn respond(&self, request_id: Uuid, decision: PermissionDecision) -> io::Result<()> {
        let pending = self
            .pending
            .lock()
            .expect("App Server approval lock should not be poisoned")
            .remove(&request_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "approval request expired"))?;
        let result = match pending.kind {
            ApprovalKind::Decision => json!({
                "decision": match decision {
                    PermissionDecision::ApproveOnce => "accept",
                    PermissionDecision::ApproveForSession => "acceptForSession",
                    PermissionDecision::Deny => "decline",
                }
            }),
            ApprovalKind::Permissions(ref requested) => json!({
                "permissions": if decision == PermissionDecision::Deny {
                    json!({})
                } else {
                    requested.clone()
                },
                "scope": if decision == PermissionDecision::ApproveForSession {
                    "session"
                } else {
                    "turn"
                }
            }),
        };
        if let Err(error) = write_message(
            &self.writer,
            &json!({"id": pending.rpc_id.clone(), "result": result}),
        ) {
            self.pending
                .lock()
                .expect("App Server approval lock should not be poisoned")
                .insert(request_id, pending);
            return Err(error);
        }
        Ok(())
    }
}

pub fn spawn_turn(launch: Launch<'_>, events: Sender<Event>) -> io::Result<SpawnedTurn> {
    let home = launch.codex_home.as_ref().map(std::path::Path::new);
    let mut client =
        crate::app_server_client::Client::open(launch.binary, launch.cwd, home, launch.on_spawn)?;
    client.set_roots(launch.extra_roots)?;
    // Discovery must happen in the same process that will load the thread.
    if !launch.execution_profile.skills.is_empty() {
        let catalog = client.call(
            "skills/list",
            json!({"cwds":[launch.cwd],"forceReload":true}),
        )?;
        for skill in &launch.execution_profile.skills {
            let found = catalog["data"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|entry| entry["skills"].as_array().into_iter().flatten())
                .any(|entry| {
                    entry["path"].as_str() == skill.path.to_str() && entry["enabled"] == true
                });
            if !found {
                return Err(io::Error::other(format!(
                    "Selected skill {} is unavailable to this App Server",
                    skill.name
                )));
            }
        }
    }
    let (approval_policy, sandbox, mut sandbox_policy) =
        execution_policy(launch.execution_profile, launch.cwd, launch.remote);
    let mut thread_params = json!({
        "cwd": launch.cwd, "approvalPolicy": approval_policy, "approvalsReviewer":"user",
        "sandbox":sandbox, "serviceName":"the_ditch",
        "config":{"features":{"multi_agent":false,"collab":false},"web_search":"live","tools":{"web_search":true},"sandbox_workspace_write":{"writable_roots":[launch.cwd],"exclude_slash_tmp":true,"exclude_tmpdir_env_var":true,"network_access":true}}
    });
    if launch.restricted {
        let config = client.call("config/read", json!({"includeLayers":false}))?;
        let mut disabled = serde_json::Map::new();
        if let Some(servers) = config["config"]["mcp_servers"].as_object() {
            for name in servers.keys() {
                disabled.insert(name.clone(), json!({"enabled":false}));
            }
        }
        thread_params["config"]["mcp_servers"] = json!(disabled);
        thread_params["config"]["features"] =
            json!({"multi_agent":false,"collab":false,"apps":false});
        if !launch.remote {
            let scope_name = format!("task_scope_{}", launch.agent_id.0.simple());
            let mut filesystem = serde_json::Map::new();
            for path in [
                Path::new(launch.binary).to_path_buf(),
                std::fs::canonicalize(launch.binary)?,
            ] {
                if let Some(parent) = path.parent() {
                    filesystem.insert(parent.to_string_lossy().into_owned(), json!("read"));
                }
            }
            filesystem.insert(":minimal".into(), json!("read"));
            filesystem.insert(launch.cwd.to_string_lossy().into_owned(), json!("write"));
            for root in launch
                .execution_profile
                .skills
                .iter()
                .filter_map(|skill| skill.path.parent())
            {
                filesystem.insert(root.to_string_lossy().into_owned(), json!("read"));
            }
            for name in [".git", ".codex", ".agents"] {
                filesystem.insert(
                    launch.cwd.join(name).to_string_lossy().into_owned(),
                    json!("read"),
                );
            }
            if let Ok(executable) = std::env::current_exe()
                && let Some(bundle) = executable
                    .ancestors()
                    .filter(|p| p.extension().is_some_and(|e| e == "app"))
                    .last()
            {
                filesystem.insert(bundle.to_string_lossy().into_owned(), json!("deny"));
            }
            for root in &launch.protected_roots {
                filesystem.insert(root.to_string_lossy().into_owned(), json!("deny"));
            }
            thread_params["config"]["permissions"] =
                json!({scope_name.clone():{"filesystem":filesystem,"network":{"enabled":true}}});
            thread_params.as_object_mut().unwrap().remove("sandbox");
            thread_params["config"]["default_permissions"] = json!(scope_name);
            sandbox_policy = json!({"inheritPermissions":scope_name});
        }
        sandbox_policy["outputSchema"] = json!({"type":"object","additionalProperties":false,
        "required":["summary","changes","checks","remaining_work"],"properties":{
            "summary":{"type":"string"},"changes":{"type":"array","items":{"type":"string"}},
            "checks":{"type":"array","items":{"type":"string"}},"remaining_work":{"type":"array","items":{"type":"string"}}
        }});
    }
    if let Some(model) = launch.execution_profile.model.as_deref() {
        thread_params["model"] = json!(model);
    }
    let method = if let Some(thread_id) = launch.resume_thread {
        thread_params["threadId"] = json!(thread_id);
        "thread/resume"
    } else {
        "thread/start"
    };
    client.send(&json!({"id":THREAD_ID,"method":method,"params":thread_params}))?;
    let child = client.child.take().unwrap();
    let writer = client.writer.take().expect("App Server input");
    let incoming = client.incoming.take().unwrap();
    let pending = Arc::new(Mutex::new(HashMap::new()));
    let skills = launch.execution_profile.skills.clone();
    let reader_writer = Arc::downgrade(&writer);
    let reader_pending = Arc::clone(&pending);
    let prompt = launch.prompt.to_owned();
    let cwd = launch.cwd.to_path_buf();
    let model = launch.execution_profile.model.clone();
    let effort = launch.execution_profile.reasoning_effort.clone();
    let project_id = launch.project_id;
    let agent_id = launch.agent_id;
    let reader_events = events.clone();
    std::thread::spawn(move || {
        let mut gate = TurnEventGate::default();
        for line in incoming {
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && !gate.accept(&value)
            {
                continue;
            }
            handle_line(
                &line,
                &reader_writer,
                &reader_pending,
                &reader_events,
                project_id,
                agent_id,
                &cwd,
                &prompt,
                model.as_deref(),
                effort.as_deref(),
                approval_policy,
                &sandbox_policy,
                &skills,
            );
        }
    });

    Ok(SpawnedTurn {
        child,
        control: ActiveTurn { writer, pending },
    })
}

#[derive(Default)]
struct TurnEventGate {
    thread_response: bool,
    thread: Option<String>,
    turn: Option<String>,
}
impl TurnEventGate {
    fn accept(&mut self, value: &Value) -> bool {
        if value.get("method").is_none() {
            match value.get("id").and_then(Value::as_i64) {
                Some(THREAD_ID) => {
                    if self.thread_response {
                        return false;
                    }
                    self.thread_response = true;
                    self.thread = value
                        .pointer("/result/thread/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                Some(TURN_ID) => {
                    if self.thread.is_none() {
                        return false;
                    }
                    self.turn = value
                        .pointer("/result/turn/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                _ => {}
            }
            return true;
        }
        let method = value["method"].as_str().unwrap_or("");
        if (method.starts_with("turn/") || method.starts_with("item/")) && self.thread.is_none() {
            return false;
        }
        if let Some(thread) = value.pointer("/params/threadId").and_then(Value::as_str)
            && self.thread.as_deref() != Some(thread)
        {
            return false;
        }
        let turn = value
            .pointer("/params/turnId")
            .or_else(|| value.pointer("/params/turn/id"))
            .and_then(Value::as_str);
        if let (Some(expected), Some(actual)) = (&self.turn, turn)
            && expected != actual
        {
            return false;
        }
        if method == "turn/started" {
            self.turn = turn.map(str::to_owned);
        }
        true
    }
}

pub fn execution_policy(
    profile: &AgentExecutionProfile,
    cwd: &Path,
    remote: bool,
) -> (&'static str, &'static str, Value) {
    if remote {
        let (approval, sandbox) = remote_policy(profile);
        return (approval, sandbox, json!({"type":"dangerFullAccess"}));
    }
    match profile.approval {
        ditch_core::AgentApprovalPreset::FullAccess => (
            "never",
            "danger-full-access",
            json!({"type":"dangerFullAccess"}),
        ),
        _ => (
            if profile.approval == ditch_core::AgentApprovalPreset::Ask {
                "untrusted"
            } else {
                "never"
            },
            "workspace-write",
            json!({"type":"workspaceWrite","writableRoots":[cwd],"networkAccess":true,"excludeSlashTmp":true,"excludeTmpdirEnvVar":true}),
        ),
    }
}

fn remote_policy(_: &AgentExecutionProfile) -> (&'static str, &'static str) {
    // SSH hosts are always guarded by App Server approvals. The unconfined
    // sandbox policy avoids making Bubblewrap a connection prerequisite, but
    // it does not disable approvals: approved work runs with the SSH user's
    // normal privileges and nothing silently receives root privileges.
    ("untrusted", "danger-full-access")
}

#[allow(clippy::too_many_arguments)]
fn handle_line(
    line: &str,
    writer: &Weak<Mutex<ChildStdin>>,
    pending: &Arc<Mutex<HashMap<Uuid, PendingApproval>>>,
    events: &Sender<Event>,
    project_id: ProjectId,
    agent_id: AgentId,
    cwd: &Path,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    approval_policy: &str,
    sandbox_policy: &Value,
    skills: &[ditch_core::SkillBinding],
) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        let text = line.trim();
        if !text.is_empty() {
            let _ = events.send(Event::Diagnostic(text.to_owned()));
        }
        return;
    };

    if value.get("method").is_none()
        && value.get("id").and_then(Value::as_i64) == Some(INITIALIZE_ID)
    {
        if let Some(error) = rpc_error(&value) {
            let _ = events.send(Event::Failed(error));
        }
        return;
    }
    if value.get("method").is_none() && value.get("id").and_then(Value::as_i64) == Some(THREAD_ID) {
        if let Some(error) = rpc_error(&value) {
            let _ = events.send(Event::Failed(error));
            return;
        }
        let Some(thread_id) = value.pointer("/result/thread/id").and_then(Value::as_str) else {
            let _ = events.send(Event::Failed(
                "Codex App Server did not return a thread id".to_owned(),
            ));
            return;
        };
        let _ = events.send(Event::ThreadReady(thread_id.to_owned()));
        let Some(writer) = writer.upgrade() else {
            return;
        };
        let mut params = json!({
            "threadId": thread_id,
            "input": [{"type": "text", "text": prompt}],
            "cwd": cwd.to_string_lossy(),
            "approvalPolicy": approval_policy,
            "approvalsReviewer": "user",
            "sandboxPolicy": sandbox_policy
        });
        if let Some(expected) = sandbox_policy.get("inheritPermissions") {
            if value["result"]["activePermissionProfile"]["id"] != *expected {
                let _ = events.send(Event::Failed(
                    "App Server did not confirm the restricted execution profile".into(),
                ));
                return;
            }
            params.as_object_mut().unwrap().remove("sandboxPolicy");
        }
        if let Some(schema) = sandbox_policy.get("outputSchema") {
            params["outputSchema"] = schema.clone();
            if let Some(policy) = params
                .get_mut("sandboxPolicy")
                .and_then(Value::as_object_mut)
            {
                policy.remove("outputSchema");
            }
        }
        for skill in skills {
            let matches = crate::skill_files::read_tree(skill.path.parent().unwrap_or(cwd))
                .and_then(|f| crate::skill_files::validate_files(&f))
                .is_ok_and(|v| v.0 == skill.content_hash);
            if !matches {
                let _ = events.send(Event::Failed(format!(
                    "Selected skill {} changed immediately before the turn; no turn was sent",
                    skill.name
                )));
                return;
            }
            params["input"]
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"skill","name":skill.name,"path":skill.path}));
        }
        if let Some(model) = model {
            params["model"] = Value::String(model.to_owned());
        }
        if let Some(effort) = effort {
            params["effort"] = Value::String(effort.to_owned());
        }
        if let Err(error) = write_message(
            &writer,
            &json!({"id": TURN_ID, "method": "turn/start", "params": params}),
        ) {
            let _ = events.send(Event::Failed(format!(
                "Failed to start Codex App Server turn: {error}"
            )));
        }
        return;
    }
    if value.get("method").is_none() && value.get("id").and_then(Value::as_i64) == Some(TURN_ID) {
        if let Some(error) = rpc_error(&value) {
            let _ = events.send(Event::Failed(error));
        }
        return;
    }

    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = value.get("params").unwrap_or(&Value::Null);
    match method {
        "skills/changed" => {
            let _ = events.send(Event::SkillsChanged);
        }
        "thread/tokenUsage/updated" => {
            let usage = &params["tokenUsage"];
            let mut result = serde_json::Map::new();
            for scope in ["last", "total"] {
                let mut values = serde_json::Map::new();
                for key in [
                    "totalTokens",
                    "inputTokens",
                    "cachedInputTokens",
                    "outputTokens",
                    "reasoningOutputTokens",
                ] {
                    if let Some(number) = usage[scope][key].as_u64() {
                        values.insert(key.into(), json!(number));
                    }
                }
                result.insert(scope.into(), Value::Object(values));
            }
            let _ = events.send(Event::ProviderUsage(Value::Object(result)));
        }
        "thread/name/updated" => {
            if let Some(name) = params
                .get("threadName")
                .or_else(|| params.get("name"))
                .and_then(Value::as_str)
            {
                let _ = events.send(Event::Title(name.into()));
            }
        }
        "item/agentMessage/delta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                let _ = events.send(Event::AssistantDelta(delta.into()));
            }
        }
        "turn/started" => {
            let _ = events.send(Event::TurnStarted);
        }
        "item/started" => {
            let kind = params.pointer("/item/type").and_then(Value::as_str);
            let action = match kind {
                Some("commandExecution") => "Running a command",
                Some("fileChange") => "Changing project files",
                Some("agentMessage") => "Writing a response",
                Some("webSearch") => "Searching the web",
                _ => "Codex is working",
            };
            let _ = events.send(Event::Action(action.to_owned()));
        }
        "item/completed" => handle_completed_item(params, events),
        "item/commandExecution/requestApproval" => {
            queue_approval(
                &value,
                pending,
                events,
                project_id,
                agent_id,
                PermissionActionKind::RunCommand,
                ApprovalKind::Decision,
            );
        }
        "item/fileChange/requestApproval" => {
            queue_approval(
                &value,
                pending,
                events,
                project_id,
                agent_id,
                PermissionActionKind::EditFiles,
                ApprovalKind::Decision,
            );
        }
        "item/permissions/requestApproval" => {
            let requested = params
                .get("permissions")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let action = if requested
                .pointer("/network/enabled")
                .and_then(Value::as_bool)
                == Some(true)
            {
                PermissionActionKind::AccessNetwork
            } else {
                PermissionActionKind::EditFiles
            };
            queue_approval(
                &value,
                pending,
                events,
                project_id,
                agent_id,
                action,
                ApprovalKind::Permissions(requested),
            );
        }
        "turn/completed" => {
            let status = params
                .pointer("/turn/status")
                .and_then(Value::as_str)
                .unwrap_or("failed")
                .to_owned();
            let error = params
                .pointer("/turn/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let _ = events.send(Event::TurnCompleted { status, error });
        }
        "error" => {
            if let Some(message) = params.pointer("/error/message").and_then(Value::as_str) {
                let _ = events.send(Event::Failed(message.to_owned()));
            }
        }
        "warning" | "configWarning" => {
            let message = params
                .get("message")
                .or_else(|| params.get("summary"))
                .and_then(Value::as_str)
                .unwrap_or("Codex App Server warning");
            let _ = events.send(Event::Diagnostic(message.to_owned()));
        }
        _ => {
            if let Some(id) = value.get("id") {
                if let Some(writer) = writer.upgrade() {
                    let _ = write_message(
                        &writer,
                        &json!({"id":id,"error":{"code":-32601,"message":"Ditch does not support this interactive request"}}),
                    );
                }
                let _ = events.send(Event::Failed(format!(
                    "Unsupported interactive App Server request: {method}"
                )));
            }
        }
    }
}

fn handle_completed_item(params: &Value, events: &Sender<Event>) {
    let Some(item) = params.get("item") else {
        return;
    };
    match item.get("type").and_then(Value::as_str) {
        Some("agentMessage") => {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                let _ = events.send(Event::AssistantMessage(text.trim().to_owned()));
            }
        }
        Some("commandExecution") => {
            let command = item
                .get("command")
                .map(display_json_text)
                .unwrap_or_default();
            if !command.trim().is_empty() {
                let _ = events.send(Event::ToolMessage(command));
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn queue_approval(
    value: &Value,
    pending: &Arc<Mutex<HashMap<Uuid, PendingApproval>>>,
    events: &Sender<Event>,
    project_id: ProjectId,
    agent_id: AgentId,
    mut action: PermissionActionKind,
    kind: ApprovalKind,
) {
    let Some(rpc_id) = value.get("id").cloned() else {
        let _ = events.send(Event::Failed(
            "Codex App Server sent an approval without a request id".to_owned(),
        ));
        return;
    };
    let params = value.get("params").unwrap_or(&Value::Null);
    let command = params.get("command").map(display_json_text);
    let network = params.get("networkApprovalContext");
    if network.is_some() {
        action = PermissionActionKind::AccessNetwork;
    }
    let target = network
        .and_then(|value| value.get("host"))
        .and_then(Value::as_str)
        .or_else(|| params.get("grantRoot").and_then(Value::as_str))
        .or_else(|| params.get("cwd").and_then(Value::as_str))
        .or(command.as_deref())
        .unwrap_or("Remote project")
        .to_owned();
    let summary = params
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| match action {
            PermissionActionKind::AccessNetwork => {
                "Codex requests network access on the SSH host".to_owned()
            }
            PermissionActionKind::EditFiles => {
                "Codex requests permission to change remote files".to_owned()
            }
            _ => "Codex requests permission to run a remote command".to_owned(),
        });
    let request_id = Uuid::new_v4();
    pending
        .lock()
        .expect("App Server approval lock should not be poisoned")
        .insert(request_id, PendingApproval { rpc_id, kind });
    let _ = events.send(Event::ApprovalRequested(PermissionRequest {
        id: request_id,
        project_id,
        agent_id: Some(agent_id),
        action,
        summary,
        target,
        command,
        created_at: Utc::now(),
        expires_at: None,
    }));
}

fn display_json_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}

fn rpc_error(value: &Value) -> Option<String> {
    value.get("error").map(|error| {
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex App Server request failed")
            .to_owned()
    })
}

fn write_message(writer: &Arc<Mutex<ChildStdin>>, value: &Value) -> io::Result<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| io::Error::other("App Server writer lock was poisoned"))?;
    serde_json::to_writer(&mut *writer, value).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ditch_core::AgentApprovalPreset;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn event_gate_rejects_stale_and_out_of_order_events() {
        let mut gate = TurnEventGate::default();
        assert!(!gate.accept(&json!({"method":"item/completed","params":{}})));
        assert!(!gate.accept(&json!({"id":TURN_ID,"result":{"turn":{"id":"early"}}})));
        assert!(gate.accept(&json!({"id":THREAD_ID,"result":{"thread":{"id":"thread"}}})));
        assert!(!gate.accept(&json!({"id":THREAD_ID,"result":{"thread":{"id":"duplicate"}}})));
        assert!(gate.accept(&json!({"id":TURN_ID,"result":{"turn":{"id":"turn"}}})));
        assert!(!gate.accept(&json!({"method":"item/completed","params":{"threadId":"other"}})));
        assert!(!gate.accept(&json!({"method":"turn/completed","params":{"turnId":"old"}})));
        assert!(gate.accept(&json!({"id":THREAD_ID,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread","turnId":"turn"}})));
        assert!(gate.accept(
            &json!({"method":"turn/completed","params":{"threadId":"thread","turnId":"turn"}})
        ));
    }

    fn ids() -> (ProjectId, AgentId) {
        (ProjectId::new(), AgentId::new())
    }

    #[test]
    fn remote_policy_never_inherits_the_desktop_full_access_setting() {
        let guarded = AgentExecutionProfile::default();
        assert_eq!(remote_policy(&guarded), ("untrusted", "danger-full-access"));
        let full = AgentExecutionProfile {
            approval: AgentApprovalPreset::FullAccess,
            ..AgentExecutionProfile::default()
        };
        assert_eq!(remote_policy(&full), ("untrusted", "danger-full-access"));
    }

    #[test]
    fn command_approval_is_translated_to_a_ditch_permission_request() {
        let (project_id, agent_id) = ids();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        handle_line(
            r#"{"id":44,"method":"item/commandExecution/requestApproval","params":{"reason":"Install dependencies","command":"npm install","cwd":"/srv/app"}}"#,
            &Weak::new(),
            &pending,
            &tx,
            project_id,
            agent_id,
            Path::new("/srv/app"),
            "build",
            None,
            None,
            "untrusted",
            &json!({"type":"dangerFullAccess"}),
            &[],
        );
        let Event::ApprovalRequested(request) = rx.recv().unwrap() else {
            panic!("expected approval event");
        };
        assert_eq!(request.project_id, project_id);
        assert_eq!(request.agent_id, Some(agent_id));
        assert_eq!(request.command.as_deref(), Some("npm install"));
        assert_eq!(request.target, "/srv/app");
        assert!(pending.lock().unwrap().contains_key(&request.id));
    }

    #[test]
    fn completed_agent_message_uses_the_authoritative_item_text() {
        let (project_id, agent_id) = ids();
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = mpsc::channel();
        handle_line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","text":"Done."}}}"#,
            &Weak::new(),
            &pending,
            &tx,
            project_id,
            agent_id,
            Path::new("/srv/app"),
            "build",
            None,
            None,
            "untrusted",
            &json!({"type":"dangerFullAccess"}),
            &[],
        );
        assert_eq!(
            rx.recv().unwrap(),
            Event::AssistantMessage("Done.".to_owned())
        );
    }

    #[test]
    fn stdio_turn_round_trips_an_app_server_approval() {
        let root = std::env::temp_dir().join(format!("ditch-app-server-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join("fake-codex");
        let capture = root.join("approval.json");
        let thread_capture = root.join("thread.json");
        let turn_capture = root.join("turn.json");
        fs::write(
            &binary,
            format!(
                r#"#!/bin/sh
read initialize
printf '%s\n' '{{"id":1,"result":{{"userAgent":"fake"}}}}'
read initialized
read thread_start
printf '%s\n' "$thread_start" > '{}'
printf '%s\n' '{{"id":2,"result":{{"thread":{{"id":"thr_remote"}}}}}}'
read turn_start
printf '%s\n' "$turn_start" > '{}'
printf '%s\n' '{{"method":"turn/started","params":{{"turn":{{"id":"turn_remote","status":"inProgress","items":[]}}}}}}'
printf '%s\n' '{{"id":77,"method":"item/commandExecution/requestApproval","params":{{"threadId":"thr_remote","turnId":"turn_remote","itemId":"item_1","reason":"Run tests","command":"cargo test","cwd":"/srv/app"}}}}'
read approval
printf '%s\n' "$approval" > '{}'
printf '%s\n' '{{"method":"item/completed","params":{{"item":{{"type":"agentMessage","id":"msg_1","text":"Tests passed."}}}}}}'
printf '%s\n' '{{"method":"turn/completed","params":{{"turn":{{"id":"turn_remote","status":"completed","items":[]}}}}}}'
"#,
                thread_capture.display(),
                turn_capture.display(),
                capture.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&binary, permissions).unwrap();

        let (project_id, agent_id) = ids();
        let profile = AgentExecutionProfile::default();
        let (tx, rx) = mpsc::channel();
        let spawned = spawn_turn(
            Launch {
                restricted: false,
                protected_roots: vec![],
                remote: true,
                extra_roots: &[],
                on_spawn: None,
                binary: binary.to_str().unwrap(),
                cwd: &root,
                project_id,
                agent_id,
                prompt: "Run tests",
                resume_thread: None,
                execution_profile: &profile,
                codex_home: None,
            },
            tx,
        )
        .unwrap();

        let mut request_id = None;
        let mut completed = false;
        for _ in 0..8 {
            match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
                Event::ApprovalRequested(request) => {
                    request_id = Some(request.id);
                    spawned
                        .control
                        .respond(request.id, PermissionDecision::ApproveForSession)
                        .unwrap();
                }
                Event::TurnCompleted { status, error } => {
                    assert_eq!(status, "completed");
                    assert_eq!(error, None);
                    completed = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(request_id.is_some());
        assert!(completed);
        let response: Value = serde_json::from_slice(&fs::read(&capture).unwrap()).unwrap();
        assert_eq!(response.get("id").and_then(Value::as_i64), Some(77));
        assert_eq!(
            response.pointer("/result/decision").and_then(Value::as_str),
            Some("acceptForSession")
        );
        let thread_request: Value =
            serde_json::from_slice(&fs::read(&thread_capture).unwrap()).unwrap();
        assert_eq!(
            thread_request
                .pointer("/params/approvalPolicy")
                .and_then(Value::as_str),
            Some("untrusted")
        );
        assert_eq!(
            thread_request
                .pointer("/params/sandbox")
                .and_then(Value::as_str),
            Some("danger-full-access")
        );
        let turn_request: Value =
            serde_json::from_slice(&fs::read(&turn_capture).unwrap()).unwrap();
        assert_eq!(
            turn_request
                .pointer("/params/sandboxPolicy/type")
                .and_then(Value::as_str),
            Some("dangerFullAccess")
        );
        let _ = spawned.child.lock().unwrap().wait();
        fs::remove_dir_all(root).unwrap();
    }
}
