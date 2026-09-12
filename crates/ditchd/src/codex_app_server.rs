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
    Questions(Vec<ditch_core::AgentQuestion>),
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
        write_message(
            &self.writer,
            &json!({"id":entry.rpc_id,"result":{"answers":values}}),
        )?;
        pending.remove(&request_id);
        Ok(())
    }

    pub fn respond(&self, request_id: Uuid, decision: PermissionDecision) -> io::Result<()> {
        let pending = self
            .pending
            .lock()
            .expect("App Server approval lock should not be poisoned")
            .remove(&request_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "approval request expired"))?;
        let result = match pending.kind {
            ApprovalKind::Questions(_) => {
                self.pending.lock().unwrap().insert(request_id, pending);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "This request needs answers, not an approval",
                ));
            }
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

    if let Err(error) = write_message(
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
    ) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }

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
    let (lines_tx, lines_rx) = mpsc::sync_channel(128);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if lines_tx.send(line).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        let mut waiting_for = Some(INITIALIZE_ID);
        let mut deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if waiting_for.is_some() && Instant::now() >= deadline {
                let _ = reader_events.send(Event::Failed(format!(
                    "Codex App Server timed out waiting for {}",
                    waiting_for.unwrap()
                )));
                break;
            }
            let line = match lines_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(line)) => line,
                Ok(Err(error)) => {
                    let _ = reader_events.send(Event::Failed(format!(
                        "Failed to read Codex App Server output: {error}"
                    )));
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if waiting_for.is_some() && Instant::now() >= deadline {
                        let _ = reader_events.send(Event::Failed(format!(
                            "Codex App Server timed out waiting for {}",
                            waiting_for.unwrap()
                        )));
                        break;
                    }
                    continue;
                }
            };
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
                Some(&thread_request),
            );
        }
        // Emitted after all stdout messages; child exit alone cannot finalize a run.
        let _ = reader_events.send(Event::OutputClosed);
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
    thread_request: Option<&Value>,
) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        let text = line.trim();
        if !text.is_empty() {
            let _ = events.send(Event::Diagnostic(text.to_owned()));
        }
        return;
    };

    if value.get("method").is_none()
        && value.get("id").and_then(Value::as_str) == Some(INITIALIZE_ID)
    {
        if let Some(error) = rpc_error(&value) {
            let _ = events.send(Event::Failed(error));
        } else if let (Some(writer), Some(request)) = (writer.upgrade(), thread_request) {
            if let Err(error) = write_message(&writer, &json!({"method":"initialized","params":{}}))
                .and_then(|_| write_message(&writer, request))
            {
                let _ = events.send(Event::Failed(format!(
                    "Failed to initialize Codex: {error}"
                )));
            }
        }
        return;
    }
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
                        &json!({"id":id,"error":{"code":-32601,"message":format!("Ditch does not support {method}")}}),
                    );
                }
                let _ = events.send(Event::ToolMessage(format!(
                    "Unsupported Codex request was rejected: {method}"
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
        .unwrap_or("Remote project")
        .to_owned();
    let summary = params
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| match action {
            PermissionActionKind::AnswerQuestion => {
                "Codex needs your answer to continue".to_owned()
            }
            PermissionActionKind::AccessNetwork => {
                "Codex requests network access on the SSH host".to_owned()
            }
            PermissionActionKind::EditFiles => {
                "Codex requests permission to change remote files".to_owned()
            }
            _ => "Codex requests permission to run a remote command".to_owned(),
        });
    let request_id = Uuid::new_v4();
    let questions = match &kind {
        ApprovalKind::Questions(questions) => questions.clone(),
        _ => Vec::new(),
    };
    pending
        .lock()
        .unwrap()
        .insert(request_id, PendingApproval { rpc_id, kind });
    let _ = events.send(Event::ApprovalRequested(PermissionRequest {
        id: request_id,
        questions,
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
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;

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
                    "untrusted",
                    None,
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
        };
        let (tx, rx) = mpsc::channel();
        handle_line(&json!({"id":1,"method":"item/tool/requestUserInput","params":{"questions":[
            {"id":"choice","header":"Select","question":"Which path?","options":[{"label":"A","description":"First"}]},
            {"id":"note","header":"Note","question":"Any details?","isSecret":true}
        ]}}).to_string(), &Arc::downgrade(&writer), &pending, &tx, ProjectId::new(), AgentId::new(), Path::new("/tmp"), "", None, None, "untrusted", None);
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
        assert!(pending.lock().unwrap().is_empty());
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
            None,
        );
        assert!(matches!(rx.try_recv(), Ok(Event::Action(_))));
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
            None,
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
            None,
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
printf '%s\n' '{{"id":"ditch:initialize","result":{{"userAgent":"fake"}}}}'
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
