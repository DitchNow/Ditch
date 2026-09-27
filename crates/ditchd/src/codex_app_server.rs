//! Codex App Server transport shared by local and SSH-hosted projects.
//!
//! Each runtime starts one stdio App Server process per active turn, translates its
//! JSON-RPC events into the stable Ditch protocol, and closes the process when
//! the turn finishes. A later prompt resumes the persisted Codex thread in a
//! fresh App Server process.

use chrono::Utc;
use ditch_core::{
    AgentApprovalPreset, AgentExecutionProfile, AgentId, PermissionActionKind, PermissionRequest,
    ProjectId,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use uuid::Uuid;

const INITIALIZE_ID: &str = "ditch:initialize";
const THREAD_ID: &str = "ditch:thread";
const TURN_ID: &str = "ditch:turn";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionDecision {
    ApproveOnce,
    ApproveForSession,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    ThreadReady(String),
    EffectiveModel(String),
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
    OutputClosed,
    PermissionResolved(Uuid),
}

#[derive(Clone)]
pub struct ActiveTurn {
    writer: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<Uuid, PendingApproval>>>,
    active_ids: Arc<Mutex<Option<(String, String)>>>,
}

#[derive(Clone)]
struct PendingApproval {
    rpc_id: Value,
    kind: ApprovalKind,
    item_id: Option<String>,
    response: Option<Value>,
    sent_at: Option<Instant>,
}

#[derive(Clone)]
enum ApprovalKind {
    Decision,
    Permissions(Value),
    Questions(Vec<ditch_core::AgentQuestion>),
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
    pub fn answer(
        &self,
        request_id: Uuid,
        answers: std::collections::BTreeMap<String, Vec<String>>,
    ) -> io::Result<()> {
        let mut pending = self.pending.lock().unwrap();
        let entry = pending
            .get(&request_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Question expired"))?;
        let ApprovalKind::Questions(questions) = &entry.kind else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Not a question",
            ));
        };
        if answers.len() != questions.len()
            || questions.iter().any(|question| {
                answers.get(&question.id).is_none_or(|values| {
                    values.is_empty()
                        || values.len() > 20
                        || values.iter().any(|v| v.len() > 20_000)
                })
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Answer each requested question",
            ));
        }
        let values = answers
            .into_iter()
            .map(|(id, answers)| (id, json!({"answers":answers})))
            .collect::<serde_json::Map<_, _>>();
        let response = json!({"answers":values});
        send_approval_response(
            &self.writer,
            pending.get_mut(&request_id).unwrap(),
            response,
        )
    }

    pub fn interrupt(&self) -> io::Result<()> {
        let (thread_id, turn_id) = self
            .active_ids
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "Turn has not started"))?;
        write_message(
            &self.writer,
            &json!({"id":"ditch:interrupt","method":"turn/interrupt",
            "params":{"threadId":thread_id,"turnId":turn_id}}),
        )
    }

    pub fn respond(&self, request_id: Uuid, decision: PermissionDecision) -> io::Result<()> {
        let mut entries = self.pending.lock().unwrap();
        let pending = entries
            .get_mut(&request_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "approval request expired"))?;
        let result = match pending.kind {
            ApprovalKind::Questions(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "This request needs answers, not an approval",
                ));
            }
            ApprovalKind::Decision => json!({
                "decision": match decision {
                    PermissionDecision::ApproveOnce => "accept",
                    PermissionDecision::ApproveForSession => "acceptForSession",
                    PermissionDecision::Deny => "cancel",
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
        send_approval_response(&self.writer, pending, result)
    }
}

fn send_approval_response(
    writer: &Arc<Mutex<ChildStdin>>,
    pending: &mut PendingApproval,
    result: Value,
) -> io::Result<()> {
    if let Some(previous) = &pending.response {
        return if previous == &result {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "A different response is already awaiting confirmation",
            ))
        };
    }
    // Reserve before writing: a partial write must never be blindly repeated.
    pending.response = Some(result.clone());
    pending.sent_at = Some(Instant::now());
    write_message(writer, &json!({"id":pending.rpc_id,"result":result}))
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
    let active_ids = Arc::new(Mutex::new(None));
    let reader_writer = Arc::downgrade(&writer);
    let reader_pending = Arc::clone(&pending);
    let reader_ids = Arc::clone(&active_ids);
    let prompt = launch.prompt.to_owned();
    let cwd = launch.cwd.to_path_buf();
    let model = launch.execution_profile.model.clone();
    let effort = launch.execution_profile.reasoning_effort.clone();
    let project_id = launch.project_id;
    let agent_id = launch.agent_id;
    let reader_events = events.clone();
    std::thread::spawn(move || {
        let mut waiting_for = Some(THREAD_ID);
        let mut thread_id: Option<String> = None;
        let mut deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut gate = TurnEventGate::default();
        loop {
            if reader_pending
                .lock()
                .unwrap()
                .values()
                .any(|entry: &PendingApproval| {
                    entry
                        .sent_at
                        .is_some_and(|at| at.elapsed() > Duration::from_secs(30))
                })
            {
                let _ = reader_events.send(Event::Failed("Codex did not confirm the approval response within 30 seconds. The turn was stopped; rejoin before continuing.".into()));
                break;
            }
            if let Some(step) = waiting_for
                && Instant::now() >= deadline
            {
                let _ = reader_events.send(Event::Failed(format!(
                    "Codex App Server timed out waiting for {}",
                    step
                )));
                break;
            }
            let line = match incoming.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(step) = waiting_for
                        && Instant::now() >= deadline
                    {
                        let _ = reader_events.send(Event::Failed(format!(
                            "Codex App Server timed out waiting for {}",
                            step
                        )));
                        break;
                    }
                    continue;
                }
            };
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                if !gate.accept(&value) {
                    continue;
                }
                if value.get("method").is_none()
                    && value.get("id").and_then(Value::as_str) == Some(THREAD_ID)
                {
                    thread_id = value
                        .pointer("/result/thread/id")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                if let (Some(thread), Some(turn)) = (
                    thread_id.as_ref(),
                    value
                        .pointer("/result/turn/id")
                        .or_else(|| value.pointer("/params/turn/id"))
                        .and_then(Value::as_str),
                ) {
                    *reader_ids.lock().unwrap() = Some((thread.clone(), turn.to_owned()));
                }
                if value.get("method").is_none()
                    && value.get("id").and_then(Value::as_str) == Some("ditch:interrupt")
                {
                    if let Some(error) = rpc_error(&value) {
                        let _ = reader_events
                            .send(Event::Diagnostic(format!("Interrupt failed: {error}")));
                    }
                    continue;
                }
            }
            // Responses and server-initiated requests have independent ID spaces.
            // Only consume a response to the currently outstanding startup step.
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && value.get("method").is_none()
            {
                let response_id = value.get("id").and_then(Value::as_str);
                if response_id.is_none() || response_id != waiting_for {
                    continue;
                }
                if rpc_error(&value).is_none() {
                    waiting_for = match response_id {
                        Some(INITIALIZE_ID) => Some(THREAD_ID),
                        Some(THREAD_ID) => Some(TURN_ID),
                        _ => None,
                    };
                    deadline = Instant::now() + STARTUP_TIMEOUT;
                }
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
        let _ = reader_events.send(Event::OutputClosed);
    });

    Ok(SpawnedTurn {
        child,
        control: ActiveTurn {
            writer,
            pending,
            active_ids,
        },
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
            match value.get("id").and_then(Value::as_str) {
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
    _remote: bool,
) -> (&'static str, &'static str, Value) {
    match profile.approval {
        AgentApprovalPreset::FullAccess => (
            "never",
            "danger-full-access",
            json!({"type":"dangerFullAccess"}),
        ),
        _ => (
            if profile.approval == AgentApprovalPreset::Ask {
                "on-request"
            } else {
                "never"
            },
            "workspace-write",
            json!({"type":"workspaceWrite","writableRoots":[cwd],"networkAccess":true,"excludeSlashTmp":true,"excludeTmpdirEnvVar":true}),
        ),
    }
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

    if value.get("method").is_none() && value.get("id").and_then(Value::as_str) == Some(THREAD_ID) {
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
        if let Some(model) = value.pointer("/result/model").and_then(Value::as_str) {
            let _ = events.send(Event::EffectiveModel(model.to_owned()));
        }
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
    if value.get("method").is_none() && value.get("id").and_then(Value::as_str) == Some(TURN_ID) {
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
        "serverRequest/resolved" => {
            if let Some(rpc_id) = params.get("requestId") {
                let mut pending = pending.lock().unwrap();
                let ids = pending
                    .iter()
                    .filter_map(|(id, entry)| (&entry.rpc_id == rpc_id).then_some(*id))
                    .collect::<Vec<_>>();
                for id in ids {
                    pending.remove(&id);
                    let _ = events.send(Event::PermissionResolved(id));
                }
            }
        }
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
        "item/completed" => {
            if let Some(item) = params.pointer("/item/id").and_then(Value::as_str) {
                let mut pending = pending.lock().unwrap();
                let ids: Vec<_> = pending
                    .iter()
                    .filter_map(|(id, entry)| {
                        (entry.item_id.as_deref() == Some(item)).then_some(*id)
                    })
                    .collect();
                for id in ids {
                    pending.remove(&id);
                    let _ = events.send(Event::PermissionResolved(id));
                }
            }
            handle_completed_item(params, events);
        }

        "item/commandExecution/outputDelta" | "item/mcpToolCall/progress" => {
            let _ = events.send(Event::Action("Receiving tool output".into()));
        }
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
        "item/tool/requestUserInput" | "tool/requestUserInput" => {
            match serde_json::from_value::<Vec<ditch_core::AgentQuestion>>(
                params.get("questions").cloned().unwrap_or(Value::Null),
            ) {
                Ok(questions) if !questions.is_empty() && questions.len() <= 3 => {
                    queue_approval(
                        &value,
                        pending,
                        events,
                        project_id,
                        agent_id,
                        PermissionActionKind::AnswerQuestion,
                        ApprovalKind::Questions(questions),
                    );
                }
                _ => {
                    if let Some(writer) = writer.upgrade() {
                        let _ = write_message(
                            &writer,
                            &json!({"id":value.get("id"),"error":{"code":-32602,"message":"Invalid questions"}}),
                        );
                    }
                }
            }
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
                let event = if params.get("willRetry").and_then(Value::as_bool) == Some(true) {
                    Event::Action(format!("Codex reconnecting: {message}"))
                } else {
                    Event::Failed(message.to_owned())
                };
                let _ = events.send(event);
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
    if pending
        .lock()
        .unwrap()
        .values()
        .any(|entry| entry.rpc_id == rpc_id)
    {
        return;
    }
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
        .unwrap_or("Project")
        .to_owned();
    let summary = params
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| match action {
            PermissionActionKind::AnswerQuestion => {
                "Codex needs your answer to continue".to_owned()
            }
            PermissionActionKind::AccessNetwork => "Codex requests network access".to_owned(),
            PermissionActionKind::EditFiles => {
                "Codex requests permission to change project files".to_owned()
            }
            _ => "Codex requests permission to run a command".to_owned(),
        });
    let request_id = Uuid::new_v4();
    let questions = match &kind {
        ApprovalKind::Questions(questions) => questions.clone(),
        _ => Vec::new(),
    };
    pending.lock().unwrap().insert(
        request_id,
        PendingApproval {
            rpc_id,
            kind,
            item_id: params
                .get("itemId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            response: None,
            sent_at: None,
        },
    );
    let _ = events.send(Event::ApprovalRequested(PermissionRequest {
        id: request_id,
        questions,
        response_pending: false,
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
    use std::os::fd::AsRawFd;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut writer = loop {
        match writer.try_lock() {
            Ok(writer) => break writer,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(io::Error::other("App Server writer lock was poisoned"));
            }
            Err(_) if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "App Server writer busy",
                ));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let fd = writer.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    let mut remaining = bytes.as_slice();
    while !remaining.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "App Server stopped reading input",
            ));
        }
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pollfd, 1, left.as_millis().clamp(1, 100) as i32) };
        if ready < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
        if ready <= 0 {
            continue;
        }
        match writer.write(remaining) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "App Server input closed",
                ));
            }
            Ok(count) => remaining = &remaining[count..],
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ditch_core::AgentApprovalPreset;
    use std::fs;
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
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
    fn server_approval_ids_never_collide_with_client_responses() {
        for rpc_id in [
            json!(0),
            json!(1),
            json!(2),
            json!(3),
            json!(77),
            json!("ditch:initialize"),
        ] {
            let (project_id, agent_id) = ids();
            let pending = Arc::new(Mutex::new(HashMap::new()));
            let (tx, rx) = mpsc::channel();
            let request = json!({"id":rpc_id,"method":"item/commandExecution/requestApproval",
                "params":{"command":"cat README.md","cwd":"/tmp"}})
            .to_string();
            for _ in 0..2 {
                handle_line(
                    &request,
                    &Weak::new(),
                    &pending,
                    &tx,
                    project_id,
                    agent_id,
                    Path::new("/tmp"),
                    "",
                    None,
                    None,
                    "on-request",
                    &json!({"type":"workspaceWrite"}),
                    &[],
                );
            }
            assert!(
                matches!(rx.try_recv(), Ok(Event::ApprovalRequested(_))),
                "{rpc_id}"
            );
            assert!(rx.try_recv().is_err(), "duplicate request {rpc_id}");
            assert_eq!(pending.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn questions_require_complete_answers_and_use_server_request_id() {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let writer = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let control = ActiveTurn {
            writer: writer.clone(),
            pending: pending.clone(),
            active_ids: Arc::new(Mutex::new(None)),
        };
        let (tx, rx) = mpsc::channel();
        handle_line(&json!({"id":1,"method":"item/tool/requestUserInput","params":{"questions":[
            {"id":"choice","header":"Select","question":"Which path?","options":[{"label":"A","description":"First"}]},
            {"id":"note","header":"Note","question":"Any details?","isSecret":true}
        ]}}).to_string(), &Arc::downgrade(&writer), &pending, &tx, ProjectId::new(), AgentId::new(), Path::new("/tmp"), "", None, None, "untrusted", &json!({"type":"workspaceWrite"}), &[]);
        let Event::ApprovalRequested(request) = rx.recv().unwrap() else {
            panic!("missing questions")
        };
        assert_eq!(request.questions.len(), 2);
        assert!(
            control
                .respond(request.id, PermissionDecision::ApproveOnce)
                .is_err()
        );
        assert!(
            control
                .answer(
                    request.id,
                    std::collections::BTreeMap::from([("choice".into(), vec!["A".into()])])
                )
                .is_err()
        );
        assert!(pending.lock().unwrap().contains_key(&request.id));
        control
            .answer(
                request.id,
                std::collections::BTreeMap::from([
                    ("choice".into(), vec!["A".into()]),
                    ("note".into(), vec!["Keep it simple".into()]),
                ]),
            )
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(
            response["result"]["answers"]["note"]["answers"][0],
            "Keep it simple"
        );
        assert!(pending.lock().unwrap()[&request.id].response.is_some());
        assert!(control.answer(request.id, Default::default()).is_err());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn retryable_errors_do_not_end_a_turn() {
        let (tx, rx) = mpsc::channel();
        handle_line(
            r#"{"method":"error","params":{"willRetry":true,"error":{"message":"connection lost"}}}"#,
            &Weak::new(),
            &Arc::new(Mutex::new(HashMap::new())),
            &tx,
            ProjectId::new(),
            AgentId::new(),
            Path::new("/tmp"),
            "",
            None,
            None,
            "untrusted",
            &json!({"type":"workspaceWrite"}),
            &[],
        );
        assert!(matches!(rx.try_recv(), Ok(Event::Action(_))));
    }

    #[test]
    fn execution_profiles_preserve_the_selected_approval_and_sandbox() {
        for (approval, expected) in [
            (AgentApprovalPreset::Ask, ("on-request", "workspace-write")),
            (
                AgentApprovalPreset::ApproveForMe,
                ("never", "workspace-write"),
            ),
            (
                AgentApprovalPreset::FullAccess,
                ("never", "danger-full-access"),
            ),
        ] {
            for remote in [false, true] {
                let (approval, sandbox, _) = execution_policy(
                    &AgentExecutionProfile {
                        approval: approval.clone(),
                        ..Default::default()
                    },
                    Path::new("/project"),
                    remote,
                );
                assert_eq!((approval, sandbox), expected);
            }
        }
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
printf '%s\n' '{{"id":"ditch:thread","result":{{"thread":{{"id":"thr_remote"}}}}}}'
read turn_start
printf '%s\n' '{{"id":"ditch:turn","result":{{}}}}'
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
            Some("never")
        );
        assert_eq!(
            thread_request
                .pointer("/params/sandbox")
                .and_then(Value::as_str),
            Some("workspace-write")
        );
        let turn_request: Value =
            serde_json::from_slice(&fs::read(&turn_capture).unwrap()).unwrap();
        assert_eq!(
            turn_request
                .pointer("/params/sandboxPolicy/type")
                .and_then(Value::as_str),
            Some("workspaceWrite")
        );
        let _ = spawned.child.lock().unwrap().wait();
        fs::remove_dir_all(root).unwrap();
    }

    /// Opt-in against an installed Codex or an SSH wrapper. Uses only a disposable
    /// project/home supplied by the caller, never a pre-existing Ditch session.
    #[test]
    #[ignore = "requires an authenticated isolated Codex home; see docs/runtime-sessions.md"]
    fn live_app_server_session_smoke() {
        let binary = std::env::var("DITCH_TEST_CODEX_BINARY").expect("set test executable");
        let root = std::env::var("DITCH_TEST_PROJECT_ROOT").expect("set disposable project root");
        let home = std::env::var_os("DITCH_TEST_CODEX_HOME");
        let mut thread = None;
        for (index, approval, model, prompt) in [
            (
                0,
                AgentApprovalPreset::ApproveForMe,
                "gpt-6-astra",
                "Run printf DITCH_SESSION_SMOKE in the project shell, then say done. Do not inspect any files or use other tools.",
            ),
            (
                1,
                AgentApprovalPreset::Ask,
                "gpt-5.6-sol",
                "For this approval integration test, run only printf DITCH_SESSION_SMOKE with sandbox_permissions=require_escalated and justification 'Ditch approval smoke test'. The test client will approve it. Do not inspect any files or run other commands.",
            ),
            (
                2,
                AgentApprovalPreset::ApproveForMe,
                "gpt-6-astra",
                "Say hello.",
            ),
        ] {
            let profile = AgentExecutionProfile {
                approval,
                model: Some(model.into()),
                ..Default::default()
            };
            let (tx, rx) = mpsc::channel();
            let spawned = spawn_turn(
                Launch {
                    binary: &binary,
                    cwd: Path::new(&root),
                    project_id: ProjectId::new(),
                    agent_id: AgentId::new(),
                    prompt,
                    resume_thread: thread.as_deref(),
                    execution_profile: &profile,
                    restricted: false,
                    protected_roots: vec![],
                    remote: false,
                    extra_roots: &[],
                    on_spawn: None,
                    codex_home: home.clone(),
                },
                tx,
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(120);
            let mut approved = false;
            let mut confirmed = false;
            let mut effective = None;
            let mut terminal = None;
            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(Event::ThreadReady(id)) => {
                        if let Some(previous) = &thread {
                            assert_eq!(previous, &id);
                        }
                        thread = Some(id);
                    }
                    Ok(Event::EffectiveModel(id)) => effective = Some(id),
                    Ok(Event::TurnStarted) if index == 2 => spawned.control.interrupt().unwrap(),
                    Ok(Event::ApprovalRequested(request)) => {
                        assert_eq!(index, 1, "Approve for me requested permission");
                        let command = request.command.as_deref().unwrap_or("");
                        assert!(
                            command.contains("printf") && command.contains("DITCH_SESSION_SMOKE"),
                            "unexpected smoke command: {command}"
                        );
                        spawned
                            .control
                            .respond(request.id, PermissionDecision::ApproveOnce)
                            .unwrap();
                        approved = true;
                    }
                    Ok(Event::PermissionResolved(_)) => confirmed = true,
                    Ok(Event::TurnCompleted { status, error }) => {
                        terminal = Some((status, error));
                        break;
                    }
                    Ok(Event::Failed(error)) => {
                        terminal = Some(("failed".into(), Some(error)));
                        break;
                    }
                    Ok(Event::OutputClosed) => break,
                    _ => {}
                }
            }
            let mut child = spawned.child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
            assert_eq!(effective.as_deref(), Some(model));
            let (status, error) = terminal.expect("live turn timed out");
            assert_eq!(error, None);
            assert_eq!(
                status,
                if index == 2 {
                    "interrupted"
                } else {
                    "completed"
                }
            );
            if index == 1 {
                assert!(
                    approved && confirmed,
                    "approval and its confirmation must both arrive"
                );
            }
            eprintln!(
                "live turn {index}: model={model}, status={status}, approval={approved}, confirmed={confirmed}"
            );
        }
    }
}
