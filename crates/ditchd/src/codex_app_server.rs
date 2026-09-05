//! Codex App Server transport used exclusively by SSH-hosted projects.
//!
//! The desktop runtime never calls this module for local projects. The SSH
//! daemon starts one stdio App Server process per active turn, translates its
//! JSON-RPC events into the stable Ditch protocol, and closes the process when
//! the turn finishes. A later prompt resumes the persisted Codex thread in a
//! fresh App Server process.

use chrono::Utc;
use ditch_core::{
    AgentExecutionProfile, AgentId, PermissionActionKind, PermissionRequest, ProjectId,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
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
    pub binary: &'a str,
    pub cwd: &'a Path,
    pub project_id: ProjectId,
    pub agent_id: AgentId,
    pub prompt: &'a str,
    pub resume_thread: Option<&'a str>,
    pub execution_profile: &'a AgentExecutionProfile,
    pub path: Option<OsString>,
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
    let mut command = Command::new(launch.binary);
    command.process_group(0);
    command.arg("app-server");
    command.current_dir(launch.cwd);
    command.env("TERM", "xterm-256color");
    if let Some(path) = launch.path {
        command.env("PATH", path);
    }
    if let Some(codex_home) = launch.codex_home {
        command.env("CODEX_HOME", codex_home);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing App Server stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing App Server stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing App Server stderr"))?;
    let writer = Arc::new(Mutex::new(stdin));
    let pending = Arc::new(Mutex::new(HashMap::new()));

    write_message(
        &writer,
        &json!({
            "id": INITIALIZE_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "the_ditch_ssh",
                    "title": "The Ditch SSH",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        }),
    )?;
    write_message(&writer, &json!({"method": "initialized", "params": {}}))?;

    let (approval_policy, sandbox) = remote_policy(launch.execution_profile);
    let mut thread_params = json!({
        "cwd": launch.cwd.to_string_lossy(),
        "approvalPolicy": approval_policy,
        "approvalsReviewer": "user",
        "sandbox": sandbox,
        "serviceName": "the_ditch_ssh",
        "config": {
            "web_search": "live",
            "tools": {"web_search": true}
        }
    });
    if let Some(model) = launch.execution_profile.model.as_deref() {
        thread_params["model"] = Value::String(model.to_owned());
    }
    let thread_request = if let Some(thread_id) = launch.resume_thread {
        thread_params["threadId"] = Value::String(thread_id.to_owned());
        json!({"id": THREAD_ID, "method": "thread/resume", "params": thread_params})
    } else {
        json!({"id": THREAD_ID, "method": "thread/start", "params": thread_params})
    };
    write_message(&writer, &thread_request)?;

    let child = Arc::new(Mutex::new(child));
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
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => handle_line(
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
                ),
                Err(error) => {
                    let _ = reader_events.send(Event::Failed(format!(
                        "Failed to read Codex App Server output: {error}"
                    )));
                    break;
                }
            }
        }
    });

    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let line = line.trim();
            if !line.is_empty() {
                let _ = events.send(Event::Diagnostic(line.to_owned()));
            }
        }
    });

    Ok(SpawnedTurn {
        child,
        control: ActiveTurn { writer, pending },
    })
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
) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        let text = line.trim();
        if !text.is_empty() {
            let _ = events.send(Event::Diagnostic(text.to_owned()));
        }
        return;
    };

    if value.get("id").and_then(Value::as_i64) == Some(INITIALIZE_ID) {
        if let Some(error) = rpc_error(&value) {
            let _ = events.send(Event::Failed(error));
        }
        return;
    }
    if value.get("id").and_then(Value::as_i64) == Some(THREAD_ID) {
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
            "sandboxPolicy": {"type": "dangerFullAccess"}
        });
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
    if value.get("id").and_then(Value::as_i64) == Some(TURN_ID) {
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
        _ => {}
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
        .or_else(|| command.as_deref())
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
                binary: binary.to_str().unwrap(),
                cwd: &root,
                project_id,
                agent_id,
                prompt: "Run tests",
                resume_thread: None,
                execution_profile: &profile,
                path: None,
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
