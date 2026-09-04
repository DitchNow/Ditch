use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use ditch_core::{
    AgentApprovalPreset, AgentExecutionProfile, AgentId, AgentProvider, AgentResumeBlockReason,
    AgentRun, AgentState, AppPaths, AttentionKind, CodexLaunchMode, Project,
    ProjectExecutionTarget, ProjectGitPolicy, ProjectId,
};
use ditch_identity::{FileIdentityStore, IdentityStore, InstallationIdentity};
#[cfg(target_os = "macos")]
use ditch_identity::MacOsIdentityStore;
use ditch_protocol::{
    AgentChatMessage, AgentChatRole, AgentModel, ClientRequest, CodexInstallation, CodexReadiness,
    Envelope, HealthResponse, HostIdentityStatus, ProjectDirectory, ProjectFile, ProjectFileEntry,
    ProjectFileKind, ProjectFileSaved, ProjectTerminal, ProtocolError, RemoteHostPresence,
    RuntimeAttention, RuntimeStatus, ServerEvent, ServerResponse, SetupTerminal,
    SetupTerminalOutput, Snapshot,
};
use ditch_store::{
    DitchStore, discover_legacy_projects, ensure_app_dirs, ensure_project_metadata,
    load_project_config, save_project_config,
};
use ditch_upgrade::{
    HttpUpgradeBackend, LicenseKey, ReleaseVerifier, UpgradeBackend, UpgradeError,
};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const RUNTIME_IDENTITY: &str = "The Ditch Runtime";
const RUNTIME_EDITION: &str = edition::RUNTIME_EDITION;
const CODEX_BINARY_SETTING: &str = "codex_binary";
const STOP_INTERRUPT_GRACE: Duration = Duration::from_millis(1500);
const STOP_TERMINATE_GRACE: Duration = Duration::from_millis(500);
const STOP_KILL_GRACE: Duration = Duration::from_millis(1500);
static LOGIN_SHELL_PATH: OnceLock<Option<OsString>> = OnceLock::new();

#[allow(dead_code)]
fn main() {
    let result = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [command] if command == "daemon" => {
            run_runtime_with_paths(AppPaths::for_remote_user(), true)
        }
        [command, flag] if command == "daemon" && flag == "--remote" => {
            run_runtime_with_paths(AppPaths::for_remote_user(), true)
        }
        [command, flag] if command == "bridge" && flag == "--stdio" => {
            bridge_stdio(AppPaths::for_remote_user())
        }
        [command] if command == "version-json" => {
            println!(
                "{}",
                serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "protocol_version": ditch_protocol::PROTOCOL_VERSION,
                    "remote_runtime_protocol_version": ditch_protocol::REMOTE_RUNTIME_PROTOCOL_VERSION,
                    "build_identifier": option_env!("DITCH_BUILD_IDENTIFIER").unwrap_or(env!("CARGO_PKG_VERSION")),
                    "edition": RUNTIME_EDITION,
                    "os": std::env::consts::OS,
                    "architecture": std::env::consts::ARCH,
                })
            );
            Ok(())
        }
        [] => run_runtime(),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported ditchd command",
        )),
    };
    if let Err(error) = result {
        eprintln!("{RUNTIME_IDENTITY} failed: {error}");
        std::process::exit(1);
    }
}

fn run_runtime() -> io::Result<()> {
    let paths = AppPaths::for_current_user();
    run_runtime_with_paths(paths, false)
}

fn run_runtime_with_paths(paths: AppPaths, remote_runtime: bool) -> io::Result<()> {
    validate_codex_home()
        .and_then(|_| ensure_app_dirs(&paths).map_err(io::Error::other))
        .and_then(|_| serve(paths, remote_runtime))
}

/// Stdio transport used by OpenSSH. It is intentionally only a byte bridge to
/// the daemon's versioned typed protocol; it does not parse or execute shell
/// commands and exiting it never affects the remote daemon.
fn bridge_stdio(paths: AppPaths) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.len() > 4 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bridge request is too large",
            ));
        }
        let mut stream = UnixStream::connect(&paths.socket_path)?;
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        let subscription = serde_json::from_str::<Envelope<ClientRequest>>(&line)
            .ok()
            .is_some_and(|envelope| {
                matches!(
                    envelope.body,
                    ClientRequest::SubscribeEvents { .. } | ClientRequest::SubscribeAttention
                )
            });
        if subscription {
            io::copy(&mut stream, &mut stdout)?;
            stdout.flush()?;
            return Ok(());
        }
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response)?;
        stdout.write_all(response.as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}

fn validate_codex_home() -> io::Result<()> {
    let Some(value) = std::env::var_os("CODEX_HOME") else {
        return Ok(());
    };
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "CODEX_HOME must be an absolute path; use $HOME/.codex-name instead of {}",
            path.display()
        ),
    ))
}

struct RuntimeState {
    paths: AppPaths,
    store: DitchStore,
    projects: HashMap<String, Project>,
    agents: HashMap<AgentId, AgentRecord>,
    children: HashMap<AgentId, ActiveAgentChild>,
    terminals: HashMap<uuid::Uuid, ProjectTerminalRecord>,
    terminal_by_project: HashMap<ditch_core::ProjectId, uuid::Uuid>,
    subscribers: Vec<Sender<String>>,
    attention_subscribers: Vec<Sender<String>>,
    next_sequence: u64,
    instance_id: uuid::Uuid,
    attention: Vec<RuntimeAttention>,
    started_at: DateTime<Utc>,
    codex_home: Option<PathBuf>,
    codex_binary: Option<String>,
    installation_identity: Arc<InstallationIdentity>,
    edition: edition::State,
    remote_connections: ssh_remote::RemoteConnectionManager,
    remote_agent_hosts: HashMap<AgentId, String>,
    remote_terminal_hosts: HashMap<uuid::Uuid, String>,
    setup_terminals: HashMap<uuid::Uuid, SetupTerminalRecord>,
    remote_attention_hosts: HashMap<uuid::Uuid, String>,
    remote_event_subscriptions: HashSet<String>,
    remote_epochs: HashMap<String, uuid::Uuid>,
    remote_connection_states: HashMap<String, String>,
    remote_permission_hosts: HashMap<uuid::Uuid, String>,
    pending_permissions: HashMap<uuid::Uuid, ditch_core::PermissionRequest>,
    app_server_turns: HashMap<AgentId, codex_app_server::ActiveTurn>,
    app_server_permission_agents: HashMap<uuid::Uuid, AgentId>,
    remote_runtime: bool,
}

struct ProjectTerminalRecord {
    descriptor: ProjectTerminal,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send>,
}

struct SetupTerminalRecord {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send>,
    output: Arc<Mutex<Vec<u8>>>,
    exited: Arc<AtomicBool>,
}

struct AgentRecord {
    run: AgentRun,
    project_root: PathBuf,
    allow_non_git: bool,
    messages: Vec<AgentChatMessage>,
    terminal_failure: Option<String>,
}

/// A child belongs to exactly one prompt run. Its token prevents a late exit
/// watcher from an earlier run from changing the newer run's state.
#[derive(Clone)]
struct ActiveAgentChild {
    run_id: uuid::Uuid,
    process_group_id: i32,
    child: Arc<Mutex<Child>>,
    app_server: bool,
}

impl RuntimeState {
    fn new(paths: AppPaths, remote_runtime: bool) -> Result<Self, io::Error> {
        let mut store = DitchStore::open(&paths).map_err(io::Error::other)?;
        edition::initialize_store(&mut store)?;
        store.reconcile_active_agents().map_err(io::Error::other)?;
        let durable = store.load().map_err(io::Error::other)?;
        let projects = durable
            .projects
            .into_iter()
            .map(|project| (project.root_key(), project))
            .collect::<HashMap<_, _>>();
        let agents = durable
            .agents
            .into_iter()
            .filter_map(|agent| {
                let project = projects
                    .values()
                    .find(|project| project.id == agent.run.project_id)?;
                let mut run = agent.run;
                // Child processes are not durable. Never expose a persisted
                // stop affordance after a runtime restart.
                run.can_stop = false;
                Some((
                    run.id,
                    AgentRecord {
                        run,
                        project_root: project.root.clone(),
                        allow_non_git: project.git_policy == ProjectGitPolicy::AllowOutsideGit,
                        messages: agent.messages,
                        terminal_failure: agent.terminal_failure,
                    },
                ))
            })
            .collect();
        let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from);
        let selected_codex = store
            .setting(CODEX_BINARY_SETTING)
            .map_err(io::Error::other)?;
        let codex_binary = resolve_codex_binary(selected_codex.as_deref());
        if selected_codex.as_deref() != codex_binary.as_deref()
            && let Some(binary) = codex_binary.as_deref()
        {
            store
                .set_setting(CODEX_BINARY_SETTING, binary)
                .map_err(io::Error::other)?;
        }
        let legacy_identity_path = paths.data_dir.join("identity/installation-v1.json");
        #[cfg(target_os = "macos")]
        let installation_identity = if paths == AppPaths::for_current_user() {
            MacOsIdentityStore::new(&legacy_identity_path).load_or_create()
        } else {
            FileIdentityStore::new(&legacy_identity_path).load_or_create()
        };
        #[cfg(not(target_os = "macos"))]
        let installation_identity =
            FileIdentityStore::new(&legacy_identity_path).load_or_create();
        let installation_identity = Arc::new(installation_identity.map_err(io::Error::other)?);
        Ok(Self {
            paths,
            store,
            projects,
            agents,
            children: HashMap::new(),
            terminals: HashMap::new(),
            terminal_by_project: HashMap::new(),
            subscribers: Vec::new(),
            attention_subscribers: Vec::new(),
            next_sequence: 1,
            instance_id: uuid::Uuid::new_v4(),
            attention: durable.attention,
            started_at: Utc::now(),
            codex_home,
            codex_binary,
            installation_identity,
            edition: edition::State::new(),
            remote_connections: ssh_remote::RemoteConnectionManager::default(),
            remote_agent_hosts: HashMap::new(),
            remote_terminal_hosts: HashMap::new(),
            setup_terminals: HashMap::new(),
            remote_attention_hosts: HashMap::new(),
            remote_event_subscriptions: HashSet::new(),
            remote_epochs: HashMap::new(),
            remote_connection_states: HashMap::new(),
            remote_permission_hosts: HashMap::new(),
            pending_permissions: HashMap::new(),
            app_server_turns: HashMap::new(),
            app_server_permission_agents: HashMap::new(),
            remote_runtime,
        })
    }

    fn runtime_status(&self) -> RuntimeStatus {
        let mut capabilities = vec![
            format!("edition_{RUNTIME_EDITION}"),
            "persistent_sessions_v1".to_owned(),
            "attention_stream_v1".to_owned(),
            "transcript_pagination_v1".to_owned(),
            "project_files_v1".to_owned(),
            "persistent_attention_read_v1".to_owned(),
            "always_on_web_access_v1".to_owned(),
            "ssh_remote_runtime_v1".to_owned(),
            "remote_directory_picker_v1".to_owned(),
            format!(
                "remote_runtime_protocol_v{}",
                ditch_protocol::REMOTE_RUNTIME_PROTOCOL_VERSION
            ),
        ];
        edition::extend_runtime_capabilities(&self.edition, &mut capabilities);
        RuntimeStatus {
            identity: RUNTIME_IDENTITY.to_owned(),
            edition: RUNTIME_EDITION.to_owned(),
            deployment_environment: option_env!("DITCH_DEPLOYMENT_ENVIRONMENT")
                .unwrap_or("production")
                .to_owned(),
            build_identifier: option_env!("DITCH_BUILD_IDENTIFIER")
                .unwrap_or(env!("CARGO_PKG_VERSION"))
                .to_owned(),
            build_number: option_env!("DITCH_BUILD_NUMBER").unwrap_or("0").to_owned(),
            release_sequence: option_env!("DITCH_RELEASE_SEQUENCE")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            community_revision: option_env!("DITCH_COMMUNITY_REVISION").map(str::to_owned),
            pid: std::process::id(),
            socket_path: self.paths.socket_path.to_string_lossy().into_owned(),
            active_session_count: self
                .agents
                .values()
                .filter(|record| {
                    matches!(
                        record.run.state,
                        AgentState::Starting
                            | AgentState::Working
                            | AgentState::Stopping
                            | AgentState::AwaitingApproval
                            | AgentState::Blocked
                    )
                })
                .count(),
            attention_count: self.attention.len(),
            unread_attention_count: self
                .attention
                .iter()
                .filter(|attention| attention.read_at.is_none())
                .count(),
            instance_id: self.instance_id,
            codex_home: self
                .codex_home
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            codex_binary: self.codex_binary.clone(),
            started_at: Some(self.started_at),
            build_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities,
            protocol_version: ditch_protocol::PROTOCOL_VERSION,
            platform: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            projects: self.projects.values().cloned().collect(),
            tasks: Vec::new(),
            agents: self
                .agents
                .values()
                .map(|record| record.run.clone())
                .collect(),
            attention: self.attention.clone(),
            messages: Vec::new(),
            permissions: self.pending_permissions.values().cloned().collect(),
        }
    }

    fn broadcast(&mut self, event: ServerEvent) {
        let is_attention_event = matches!(
            event,
            ServerEvent::AttentionRaised(_)
                | ServerEvent::AttentionDismissed { .. }
                | ServerEvent::AttentionRead { .. }
        );
        let envelope = Envelope::new(SequencedEvent {
            sequence: self.next_sequence,
            event,
        });
        self.next_sequence += 1;
        let Ok(line) = serde_json::to_string(&envelope) else {
            return;
        };
        let mut live = Vec::new();
        for tx in self.subscribers.drain(..) {
            if tx.send(line.clone()).is_ok() {
                live.push(tx);
            }
        }
        self.subscribers = live;

        if is_attention_event {
            let mut live = Vec::new();
            for tx in self.attention_subscribers.drain(..) {
                if tx.send(line.clone()).is_ok() {
                    live.push(tx);
                }
            }
            self.attention_subscribers = live;
        }
        edition::after_broadcast(self);
    }

    fn persist_agent(&mut self, agent_id: AgentId) {
        let Some(record) = self.agents.get(&agent_id) else {
            return;
        };
        let run = record.run.clone();
        let failure = record.terminal_failure.clone();
        let home = self
            .codex_home
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned());
        if let Err(error) = self
            .store
            .upsert_agent(&run, failure.as_deref(), home.as_deref())
        {
            eprintln!("{RUNTIME_IDENTITY} failed to persist agent {agent_id:?}: {error}");
        }
    }

    fn persist_message(&mut self, message: &AgentChatMessage) {
        if let Err(error) = self.store.append_message(message) {
            eprintln!("{RUNTIME_IDENTITY} failed to persist message: {error}");
        }
    }
}

#[derive(serde::Serialize)]
struct SequencedEvent {
    sequence: u64,
    event: ServerEvent,
}

fn serve(paths: AppPaths, remote_runtime: bool) -> io::Result<()> {
    remove_stale_socket(&paths.socket_path)?;
    let listener = UnixListener::bind(&paths.socket_path)?;
    fs::set_permissions(&paths.socket_path, fs::Permissions::from_mode(0o600))?;
    let state = Arc::new(Mutex::new(RuntimeState::new(paths, remote_runtime)?));
    write_runtime_metadata(&state)?;
    edition::start(Arc::clone(&state));
    start_remote_reconciler(Arc::clone(&state));

    for stream in listener.incoming() {
        let stream = stream?;
        let state = Arc::clone(&state);
        thread::spawn(move || {
            if let Err(error) = handle_client(stream, state) {
                eprintln!("{RUNTIME_IDENTITY} client error: {error}");
            }
        });
    }

    Ok(())
}

/// Reconciles sanitized projections from remote authorities. A failed poll is
/// deliberately a no-op: losing SSH never changes an agent to stopped and no
/// command is queued for replay. Every successful poll replaces the cache for
/// that host, so remote truth wins after reconnect or a daemon epoch change.
fn start_remote_reconciler(state: Arc<Mutex<RuntimeState>>) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(3));
            let (hosts, connections) = {
                let state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                let mut hosts: HashMap<String, Vec<Project>> = HashMap::new();
                for project in state
                    .projects
                    .values()
                    .filter(|project| project.is_remote())
                {
                    if let Some(alias) = ssh_remote::remote_alias(project) {
                        hosts
                            .entry(alias.to_owned())
                            .or_default()
                            .push(project.clone());
                    }
                }
                (hosts, state.remote_connections.clone())
            };
            for (alias, projects) in hosts {
                ensure_remote_event_subscription(&state, &alias);
                let status = match connections.request(&alias, ClientRequest::RuntimeStatus) {
                    Ok(ServerResponse::RuntimeStatus(status)) => status,
                    _ => {
                        update_remote_presence(
                            &state,
                            &alias,
                            "offline",
                            Some("SSH bridge unavailable"),
                        );
                        continue;
                    }
                };
                let snapshot = match connections.request(&alias, ClientRequest::Snapshot) {
                    Ok(ServerResponse::Snapshot(snapshot)) => snapshot,
                    _ => {
                        update_remote_presence(
                            &state,
                            &alias,
                            "offline",
                            Some("Remote daemon unavailable"),
                        );
                        continue;
                    }
                };
                update_remote_presence(&state, &alias, "online", None);
                reconcile_remote_snapshot(&state, &alias, &projects, status.instance_id, snapshot);
            }
        }
    });
}

fn ensure_remote_event_subscription(state: &Arc<Mutex<RuntimeState>>, alias: &str) {
    let connections = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if !state.remote_event_subscriptions.insert(alias.to_owned()) {
            return;
        }
        state.remote_connections.clone()
    };
    let state_for_events = Arc::clone(state);
    let alias_for_thread = alias.to_owned();
    thread::spawn(move || {
        let alias_for_event = alias_for_thread.clone();
        let result = connections.stream_events(&alias_for_thread, |event| {
            forward_remote_event(&state_for_events, &alias_for_event, event);
        });
        let mut state = state_for_events
            .lock()
            .expect("runtime state lock should not be poisoned");
        state.remote_event_subscriptions.remove(&alias_for_thread);
        if let Err(error) = result {
            ssh_remote::observe(
                "remote_daemon_disconnected",
                &alias_for_thread,
                Some(match error {
                    ditch_ssh::SshError::Timeout => "event_stream_timeout",
                    _ => "event_stream_ended",
                }),
            );
        }
    });
}

fn forward_remote_event(state: &Arc<Mutex<RuntimeState>>, alias: &str, event: ServerEvent) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    match event {
        ServerEvent::AgentChanged(run) => {
            let owns_project = state.projects.values().any(|project| {
                project.id == run.project_id
                    && ssh_remote::remote_alias(project).is_some_and(|host| host == alias)
            });
            if !owns_project {
                return;
            }
            state.remote_agent_hosts.insert(run.id, alias.to_owned());
            if let Some(record) = state.agents.get_mut(&run.id) {
                record.run = run.clone();
            }
            state.broadcast(ServerEvent::AgentChanged(run));
        }
        ServerEvent::AgentMessageAppended(message) => {
            if !state
                .remote_agent_hosts
                .get(&message.agent_id)
                .is_some_and(|host| host == alias)
            {
                return;
            }
            if let Some(record) = state.agents.get_mut(&message.agent_id)
                && !record.messages.iter().any(|existing| existing == &message)
            {
                record.messages.push(message.clone());
            }
            state.broadcast(ServerEvent::AgentMessageAppended(message));
        }
        ServerEvent::PermissionRequested(request) => {
            let owned = state.projects.values().any(|project| {
                project.id == request.project_id
                    && ssh_remote::remote_alias(project).is_some_and(|host| host == alias)
            });
            if !owned {
                return;
            }
            if let Some(agent_id) = request.agent_id {
                state.remote_agent_hosts.insert(agent_id, alias.to_owned());
            }
            state
                .remote_permission_hosts
                .insert(request.id, alias.to_owned());
            state.pending_permissions.insert(request.id, request.clone());
            state.broadcast(ServerEvent::PermissionRequested(request));
        }
        ServerEvent::ProjectTerminalOutput { terminal_id, data } => {
            if state
                .remote_terminal_hosts
                .get(&terminal_id)
                .is_some_and(|host| host == alias)
            {
                state.broadcast(ServerEvent::ProjectTerminalOutput { terminal_id, data });
            }
        }
        ServerEvent::ProjectTerminalExited { terminal_id }
            if state
                .remote_terminal_hosts
                .get(&terminal_id)
                .is_some_and(|host| host == alias) =>
        {
            state.remote_terminal_hosts.remove(&terminal_id);
            state.broadcast(ServerEvent::ProjectTerminalExited { terminal_id });
        }
        // Snapshots and attention are reconciled authoritatively by the
        // bounded poller. Agent, permission, and terminal events need low
        // latency and are forwarded directly.
        _ => {}
    }
}

fn update_remote_presence(
    state: &Arc<Mutex<RuntimeState>>,
    alias: &str,
    status: &str,
    detail: Option<&str>,
) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if state
        .remote_connection_states
        .get(alias)
        .is_some_and(|value| value == status)
    {
        return;
    }
    state
        .remote_connection_states
        .insert(alias.to_owned(), status.to_owned());
    state.broadcast(ServerEvent::RemoteHostStatusChanged(RemoteHostPresence {
        ssh_host_alias: alias.to_owned(),
        state: status.to_owned(),
        updated_at: Utc::now(),
        detail: detail.map(str::to_owned),
    }));
}

fn reconcile_remote_snapshot(
    state: &Arc<Mutex<RuntimeState>>,
    alias: &str,
    projects: &[Project],
    epoch: uuid::Uuid,
    snapshot: Snapshot,
) {
    let project_ids = projects
        .iter()
        .map(|project| project.id)
        .collect::<HashSet<_>>();
    let projects_by_id = projects
        .iter()
        .map(|project| (project.id, project))
        .collect::<HashMap<_, _>>();
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let previous_epoch = state.remote_epochs.insert(alias.to_owned(), epoch);
    if previous_epoch != Some(epoch) {
        ssh_remote::observe(
            "remote_daemon_reconciled",
            alias,
            previous_epoch.map(|_| "daemon_restarted"),
        );
    }

    let authoritative_ids = snapshot
        .agents
        .iter()
        .filter(|run| project_ids.contains(&run.project_id))
        .map(|run| run.id)
        .collect::<HashSet<_>>();
    let stale = state
        .remote_agent_hosts
        .iter()
        .filter(|(_, host)| host.as_str() == alias)
        .filter_map(|(id, _)| (!authoritative_ids.contains(id)).then_some(*id))
        .collect::<Vec<_>>();
    for id in stale {
        state.remote_agent_hosts.remove(&id);
        state.agents.remove(&id);
    }
    for run in snapshot
        .agents
        .into_iter()
        .filter(|run| project_ids.contains(&run.project_id))
    {
        let Some(project) = projects_by_id.get(&run.project_id) else {
            continue;
        };
        state.remote_agent_hosts.insert(run.id, alias.to_owned());
        state.agents.insert(
            run.id,
            AgentRecord {
                run,
                project_root: project.root.clone(),
                allow_non_git: project.git_policy == ProjectGitPolicy::AllowOutsideGit,
                messages: Vec::new(),
                terminal_failure: None,
            },
        );
    }

    let authoritative_permission_ids = snapshot
        .permissions
        .iter()
        .filter(|request| project_ids.contains(&request.project_id))
        .map(|request| request.id)
        .collect::<HashSet<_>>();
    let stale_permissions = state
        .remote_permission_hosts
        .iter()
        .filter(|(_, host)| host.as_str() == alias)
        .filter_map(|(id, _)| (!authoritative_permission_ids.contains(id)).then_some(*id))
        .collect::<Vec<_>>();
    for id in stale_permissions {
        state.remote_permission_hosts.remove(&id);
        state.pending_permissions.remove(&id);
    }
    for request in snapshot
        .permissions
        .into_iter()
        .filter(|request| project_ids.contains(&request.project_id))
    {
        state
            .remote_permission_hosts
            .insert(request.id, alias.to_owned());
        state.pending_permissions.insert(request.id, request);
    }

    let replaced_attention_ids = state
        .remote_attention_hosts
        .iter()
        .filter_map(|(id, host)| (host == alias).then_some(*id))
        .collect::<HashSet<_>>();
    state
        .attention
        .retain(|attention| !replaced_attention_ids.contains(&attention.id));
    state.remote_attention_hosts.retain(|_, host| host != alias);
    for attention in snapshot.attention.into_iter().filter(|item| {
        item.project_id.is_some_and(|id| project_ids.contains(&id))
            || item
                .agent_id
                .is_some_and(|id| authoritative_ids.contains(&id))
    }) {
        state
            .remote_attention_hosts
            .insert(attention.id, alias.to_owned());
        state.attention.push(attention);
    }
    let snapshot = state.snapshot();
    state.broadcast(ServerEvent::SnapshotReplaced(snapshot));
    let attention = state.attention.clone();
    state.broadcast(ServerEvent::AttentionSnapshotReplaced(attention));
}

fn write_runtime_metadata(state: &Arc<Mutex<RuntimeState>>) -> io::Result<()> {
    let state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let run_dir = state
        .paths
        .socket_path
        .parent()
        .unwrap_or(&state.paths.data_dir);
    let path = run_dir.join("daemon.json");
    let temporary = run_dir.join(format!(".daemon-{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec(&serde_json::json!({
        "pid": std::process::id(),
        "epoch": state.instance_id,
        "started_at": state.started_at,
        "version": env!("CARGO_PKG_VERSION"),
    }))?;
    fs::write(&temporary, bytes)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(temporary, path)
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "Ditch Runtime is already running",
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) =>
            {
                fs::remove_file(path)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn handle_client(mut stream: UnixStream, state: Arc<Mutex<RuntimeState>>) -> io::Result<()> {
    let request = {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        serde_json::from_str::<Envelope<ClientRequest>>(&line)
            .map(|envelope| envelope.body)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    };

    if let ClientRequest::SubscribeEvents { .. } = request {
        return subscribe(stream, state);
    }
    if let ClientRequest::SubscribeAttention = request {
        return subscribe_attention(stream, state);
    }

    let response = handle_request(request, state);
    let envelope = Envelope::new(response);
    serde_json::to_writer(&mut stream, &envelope)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn subscribe(mut stream: UnixStream, state: Arc<Mutex<RuntimeState>>) -> io::Result<()> {
    let (tx, rx) = mpsc::channel::<String>();
    let initial = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state.subscribers.push(tx);
        let envelope = Envelope::new(SequencedEvent {
            sequence: state.next_sequence,
            event: ServerEvent::SnapshotReplaced(state.snapshot()),
        });
        state.next_sequence += 1;
        serde_json::to_string(&envelope)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    };

    stream.write_all(initial.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    for line in rx {
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }

    Ok(())
}

fn subscribe_attention(mut stream: UnixStream, state: Arc<Mutex<RuntimeState>>) -> io::Result<()> {
    let (tx, rx) = mpsc::channel::<String>();
    let initial = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state.attention_subscribers.push(tx);
        let envelope = Envelope::new(SequencedEvent {
            sequence: state.next_sequence,
            event: ServerEvent::AttentionSnapshotReplaced(state.attention.clone()),
        });
        state.next_sequence += 1;
        serde_json::to_string(&envelope)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
    };

    stream.write_all(initial.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    for line in rx {
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }

    Ok(())
}

#[allow(unreachable_patterns)]
fn handle_request(request: ClientRequest, state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    match request {
        ClientRequest::Health => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            ServerResponse::Health(HealthResponse {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                app_paths: state.paths.clone(),
                codex_binary: state.codex_binary.clone(),
                claude_binary: find_binary("claude"),
            })
        }
        ClientRequest::RuntimeStatus => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            ServerResponse::RuntimeStatus(state.runtime_status())
        }
        ClientRequest::DiscoverCodexInstallations => {
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            let installations = discover_codex_installations(state.codex_binary.as_deref());
            let selected_is_available = state.codex_binary.as_deref().is_some_and(|selected| {
                installations
                    .iter()
                    .any(|installation| installation.path == selected)
            });
            if !selected_is_available && let Some(installation) = installations.first() {
                state.codex_binary = Some(installation.path.clone());
                if let Err(error) = state
                    .store
                    .set_setting(CODEX_BINARY_SETTING, &installation.path)
                {
                    return protocol_error("codex_selection_failed", error.to_string());
                }
            }
            ServerResponse::CodexInstallations(
                installations
                    .into_iter()
                    .map(|mut installation| {
                        installation.selected =
                            state.codex_binary.as_deref() == Some(installation.path.as_str());
                        installation
                    })
                    .collect(),
            )
        }
        ClientRequest::CheckCodexReadiness => {
            let binary = active_codex_binary(&state);
            ServerResponse::CodexReadiness(check_codex_readiness(binary.as_deref()))
        }
        ClientRequest::UpdateSelectedCodex => {
            let Some(binary) = active_codex_binary(&state) else {
                return protocol_error(
                    "codex_not_found",
                    "No working Codex CLI installation was found for this user",
                );
            };
            let readiness = check_codex_readiness(Some(&binary));
            if !readiness.update_supported {
                return protocol_error(
                    "codex_update_unsupported",
                    "This Codex installation does not support `codex update`. Ditch will not guess or alter its package manager.",
                );
            }
            let path = effective_path_for_binary(Path::new(&binary));
            let Some(output) = command_output_with_timeout(
                Path::new(&binary),
                &["update"],
                Duration::from_secs(180),
                path,
            ) else {
                return protocol_error(
                    "codex_update_failed",
                    "Codex update did not finish within three minutes",
                );
            };
            if !output.status.success() {
                let detail = command_output_detail(&output);
                return protocol_error("codex_update_failed", detail);
            }
            ServerResponse::CodexReadiness(check_codex_readiness(Some(&binary)))
        }
        ClientRequest::SelectCodexBinary { path } => {
            let installations = discover_codex_installations(Some(&path));
            let Some(selected) = installations
                .iter()
                .find(|installation| installation.path == path)
            else {
                return protocol_error(
                    "invalid_codex_binary",
                    "The selected Codex executable is unavailable or did not report a version",
                );
            };
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if let Err(error) = state
                .store
                .set_setting(CODEX_BINARY_SETTING, &selected.path)
            {
                return protocol_error("codex_selection_failed", error.to_string());
            }
            state.codex_binary = Some(selected.path.clone());
            ServerResponse::Accepted
        }
        ClientRequest::Snapshot => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            ServerResponse::Snapshot(state.snapshot())
        }
        ClientRequest::ListAgentMessages {
            agent_id,
            before_sequence,
            limit,
        } => {
            let remote = {
                let state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                state
                    .remote_agent_hosts
                    .get(&agent_id)
                    .cloned()
                    .map(|alias| (alias, state.remote_connections.clone()))
            };
            if let Some((alias, connections)) = remote {
                return match connections.request(
                    &alias,
                    ClientRequest::ListAgentMessages {
                        agent_id,
                        before_sequence,
                        limit,
                    },
                ) {
                    Ok(response) => response,
                    Err(error) => protocol_error("remote_unavailable", error.to_string()),
                };
            }
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            match state
                .store
                .list_agent_messages(agent_id, before_sequence, limit)
            {
                Ok(page) => ServerResponse::AgentMessages(page),
                Err(error) => protocol_error("message_history_failed", error.to_string()),
            }
        }
        ClientRequest::Shutdown => shutdown_runtime(state),
        ClientRequest::ListProjects => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            ServerResponse::Projects(state.projects.values().cloned().collect())
        }
        ClientRequest::DiscoverProjects { search_root } => {
            match discover_legacy_projects(Path::new(&search_root)) {
                Ok(projects) => ServerResponse::Projects(projects),
                Err(error) => protocol_error("project_discovery_failed", error.to_string()),
            }
        }
        ClientRequest::DiscoverSshHosts => match ssh_remote::ssh_hosts() {
            Ok(hosts) => ServerResponse::SshHosts(hosts),
            Err(error) => protocol_error("ssh_config_failed", error.to_string()),
        },
        ClientRequest::ResolveSshHost { alias } => match ssh_remote::ssh_host(&alias) {
            Ok(host) => ServerResponse::ResolvedSshHost(host),
            Err(error) => protocol_error("ssh_config_failed", error.to_string()),
        },
        ClientRequest::PreviewSshHost { host } => match ssh_remote::preview_host(&host) {
            Ok(preview) => ServerResponse::SshConfigPreview(preview),
            Err(error) => protocol_error("ssh_config_failed", error.to_string()),
        },
        ClientRequest::AddSshHost { host } => match ssh_remote::add_host(&host) {
            Ok(()) => ServerResponse::Accepted,
            Err(error) => protocol_error("ssh_config_failed", error.to_string()),
        },
        ClientRequest::CheckRemoteSetup {
            alias,
            password,
            remember_password,
            trust_unknown_host,
        } => {
            let connections = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_connections
                .clone();
            match ssh_remote::check_setup(
                &connections,
                &alias,
                password,
                remember_password,
                trust_unknown_host,
            ) {
                Ok(status) => ServerResponse::RemoteSetup(status),
                Err(error) => protocol_error("remote_setup_failed", error.to_string()),
            }
        }
        ClientRequest::InstallRemoteRuntime { alias } => {
            let connections = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_connections
                .clone();
            match ssh_remote::install_remote_runtime(&connections, &alias) {
                Ok(status) => ServerResponse::RemoteSetup(status),
                Err(error) => protocol_error("remote_runtime_install_failed", error.to_string()),
            }
        }
        ClientRequest::InstallRemoteCodex { alias } => {
            let connections = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_connections
                .clone();
            match ssh_remote::install_remote_codex(&connections, &alias) {
                Ok(status) => ServerResponse::RemoteSetup(status),
                Err(error) => protocol_error("remote_codex_install_failed", error.to_string()),
            }
        }
        ClientRequest::InstallRemoteGit { alias } => {
            let connections = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_connections
                .clone();
            match ssh_remote::install_remote_git(&connections, &alias) {
                Ok(status) => ServerResponse::RemoteSetup(status),
                Err(error) => protocol_error("remote_git_install_failed", error.to_string()),
            }
        }
        ClientRequest::OpenRemoteCodexSandboxSetup {
            alias,
            columns,
            rows,
        } => {
            let connections = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_connections
                .clone();
            match connections.request(
                &alias,
                ClientRequest::OpenCodexSandboxSetup { columns, rows },
            ) {
                Ok(ServerResponse::SetupTerminal(terminal)) => {
                    state
                        .lock()
                        .expect("runtime state lock should not be poisoned")
                        .remote_terminal_hosts
                        .insert(terminal.id, alias);
                    ServerResponse::SetupTerminal(terminal)
                }
                Ok(response) => response,
                Err(error) => protocol_error("remote_unavailable", error.to_string()),
            }
        }
        ClientRequest::OpenRemoteCodexAuthentication {
            alias,
            columns,
            rows,
        } => {
            let connections = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_connections
                .clone();
            match connections.request(
                &alias,
                ClientRequest::OpenCodexAuthentication { columns, rows },
            ) {
                Ok(ServerResponse::SetupTerminal(terminal)) => {
                    state
                        .lock()
                        .expect("runtime state lock should not be poisoned")
                        .remote_terminal_hosts
                        .insert(terminal.id, alias);
                    ServerResponse::SetupTerminal(terminal)
                }
                Ok(response) => response,
                Err(error) => protocol_error("remote_unavailable", error.to_string()),
            }
        }
        ClientRequest::OpenCodexAuthentication { columns, rows } => {
            open_codex_authentication(state, columns, rows)
        }
        ClientRequest::OpenCodexSandboxSetup { columns, rows } => {
            open_codex_sandbox_setup(state, columns, rows)
        }
        ClientRequest::WriteSetupTerminal { terminal_id, data } => {
            setup_terminal_request_for_target(
                state,
                terminal_id,
                ClientRequest::WriteSetupTerminal { terminal_id, data },
            )
        }
        ClientRequest::ResizeSetupTerminal {
            terminal_id,
            columns,
            rows,
        } => setup_terminal_request_for_target(
            state,
            terminal_id,
            ClientRequest::ResizeSetupTerminal {
                terminal_id,
                columns,
                rows,
            },
        ),
        ClientRequest::TakeSetupTerminalOutput { terminal_id } => {
            setup_terminal_request_for_target(
                state,
                terminal_id,
                ClientRequest::TakeSetupTerminalOutput { terminal_id },
            )
        }
        ClientRequest::CloseSetupTerminal { terminal_id } => setup_terminal_request_for_target(
            state,
            terminal_id,
            ClientRequest::CloseSetupTerminal { terminal_id },
        ),
        ClientRequest::ListRemoteDirectory {
            alias,
            absolute_path,
        } => {
            let connections = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_connections
                .clone();
            match ssh_remote::list_remote_directory(&connections, &alias, &absolute_path) {
                Ok(directory) => ServerResponse::RemoteDirectory(directory),
                Err(error) => protocol_error("remote_directory_failed", error.to_string()),
            }
        }
        ClientRequest::ListFilesystemDirectory { absolute_path } => {
            list_filesystem_directory(&absolute_path)
        }
        ClientRequest::CreateRemoteProject {
            ssh_host_alias,
            name,
            remote_root,
            git_policy,
        } => create_remote_project(state, ssh_host_alias, name, remote_root, git_policy),
        ClientRequest::CreateProject {
            name,
            root,
            git_policy,
        } => {
            let root = canonical_project_root(Path::new(&root));
            let existing = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .projects
                .get(root.to_string_lossy().as_ref())
                .cloned();
            let mut project = existing.unwrap_or_else(|| Project::new(&name, &root));
            project.name = name;
            project.root = root;
            project.git_policy = git_policy;
            if project.git_policy == ProjectGitPolicy::InitializeRepository {
                match Command::new("git")
                    .arg("init")
                    .current_dir(&project.root)
                    .output()
                {
                    Ok(output) if output.status.success() => {
                        project.git_policy = ProjectGitPolicy::RequireRepository;
                    }
                    Ok(output) => {
                        return protocol_error(
                            "git_init_failed",
                            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                        );
                    }
                    Err(error) => return protocol_error("git_init_failed", error.to_string()),
                }
            }
            if let Err(error) = ensure_project_metadata(&project) {
                return protocol_error("project_metadata_failed", error.to_string());
            }
            if let Err(error) = save_project_config(&project) {
                return protocol_error("project_config_failed", error.to_string());
            }
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if let Err(error) = state.store.upsert_project(&project) {
                return protocol_error("project_store_failed", error.to_string());
            }
            state.projects.insert(project.root_key(), project.clone());
            state.broadcast(ServerEvent::ProjectChanged(project.clone()));
            ServerResponse::ProjectCreated(project)
        }
        ClientRequest::DeleteProject { project_id } => delete_project_for_target(state, project_id),
        ClientRequest::StartCodexSession {
            project_id,
            project_name,
            project_root,
            prompt,
            mode,
            execution_profile,
        } => {
            if let Some(project) = project_id
                .and_then(|id| project_by_id(&state, id))
                .filter(Project::is_remote)
            {
                forward_remote_start(
                    state,
                    project,
                    project_name,
                    project_root,
                    prompt,
                    mode,
                    execution_profile,
                )
            } else {
                start_codex_session(
                    state,
                    project_name,
                    project_root,
                    prompt,
                    mode,
                    execution_profile,
                )
            }
        }
        ClientRequest::ResumeCodexSession {
            project_id,
            project_name,
            project_root,
            thread_id,
            prompt,
            execution_profile,
        } => {
            if let Some(project) = project_id
                .and_then(|id| project_by_id(&state, id))
                .filter(Project::is_remote)
            {
                forward_remote_resume(
                    state,
                    project,
                    project_name,
                    project_root,
                    thread_id,
                    prompt,
                    execution_profile,
                )
            } else {
                resume_codex_session(
                    state,
                    project_name,
                    project_root,
                    thread_id,
                    prompt,
                    execution_profile,
                )
            }
        }
        ClientRequest::StartRemoteCodexAppServerSession {
            project_id,
            project_name,
            project_root,
            prompt,
            execution_profile,
        } => start_remote_app_server_session(
            state,
            project_id,
            project_name,
            project_root,
            None,
            prompt,
            execution_profile,
        ),
        ClientRequest::ResumeRemoteCodexAppServerSession {
            project_id,
            project_name,
            project_root,
            thread_id,
            prompt,
            execution_profile,
        } => start_remote_app_server_session(
            state,
            project_id,
            project_name,
            project_root,
            Some(thread_id),
            prompt,
            execution_profile,
        ),
        ClientRequest::PromptRemoteCodexAppServerAgent {
            agent_id,
            prompt,
            execution_profile,
        } => prompt_remote_app_server_agent(state, agent_id, prompt, execution_profile),
        ClientRequest::PromptAgent {
            agent_id,
            prompt,
            execution_profile,
        } => forward_or_prompt_agent(state, agent_id, prompt, execution_profile),
        ClientRequest::ListAgentModels {
            provider,
            project_id,
        } => {
            if let Some(project) = project_id
                .and_then(|id| project_by_id(&state, id))
                .filter(Project::is_remote)
            {
                forward_project_command(
                    &state,
                    &project,
                    ClientRequest::ListAgentModels {
                        provider,
                        project_id: Some(project.id),
                    },
                )
            } else {
                list_agent_models(state, provider)
            }
        }
        ClientRequest::OpenProjectTerminal {
            project_id,
            columns,
            rows,
        } => open_project_terminal_for_target(state, project_id, columns, rows),
        ClientRequest::WriteProjectTerminal { terminal_id, data } => {
            write_project_terminal_for_target(state, terminal_id, data)
        }
        ClientRequest::ResizeProjectTerminal {
            terminal_id,
            columns,
            rows,
        } => resize_project_terminal_for_target(state, terminal_id, columns, rows),
        ClientRequest::CloseProjectTerminal { terminal_id } => {
            close_project_terminal_for_target(state, terminal_id)
        }
        ClientRequest::ListProjectDirectory {
            project_id,
            relative_path,
        } => project_file_request_for_target(
            state,
            project_id,
            ClientRequest::ListProjectDirectory {
                project_id,
                relative_path,
            },
        ),
        ClientRequest::ReadProjectFile {
            project_id,
            relative_path,
        } => project_file_request_for_target(
            state,
            project_id,
            ClientRequest::ReadProjectFile {
                project_id,
                relative_path,
            },
        ),
        ClientRequest::WriteProjectFile {
            project_id,
            relative_path,
            expected_revision,
            content,
        } => project_file_request_for_target(
            state,
            project_id,
            ClientRequest::WriteProjectFile {
                project_id,
                relative_path,
                expected_revision,
                content,
            },
        ),
        ClientRequest::StopAgent { agent_id } => forward_or_stop_agent(state, agent_id, false),
        ClientRequest::ForceKillAgent { agent_id } => forward_or_stop_agent(state, agent_id, true),
        ClientRequest::DeleteAgent { agent_id } => {
            forward_or_agent_action(state, agent_id, ClientRequest::DeleteAgent { agent_id })
        }
        ClientRequest::RenameAgent { agent_id, title } => forward_or_agent_action(
            state,
            agent_id,
            ClientRequest::RenameAgent { agent_id, title },
        ),
        ClientRequest::DismissAttention { attention_id } => {
            let remote = {
                let state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                state
                    .remote_attention_hosts
                    .get(&attention_id)
                    .cloned()
                    .map(|alias| (alias, state.remote_connections.clone()))
            };
            if let Some((alias, connections)) = remote {
                return connections
                    .request(&alias, ClientRequest::DismissAttention { attention_id })
                    .unwrap_or_else(|error| {
                        protocol_error("remote_unavailable", error.to_string())
                    });
            }
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if let Err(error) = state.store.dismiss_attention(attention_id) {
                return protocol_error("attention_store_failed", error.to_string());
            }
            state.attention.retain(|item| item.id != attention_id);
            state.broadcast(ServerEvent::AttentionDismissed { attention_id });
            ServerResponse::Accepted
        }
        ClientRequest::MarkAttentionRead { attention_ids } => {
            mark_attention_read_for_targets(state, Some(attention_ids))
        }
        ClientRequest::MarkAllAttentionRead => mark_attention_read_for_targets(state, None),
        ClientRequest::HostIdentityStatus => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            let identity = state.installation_identity.summary();
            ServerResponse::HostIdentity(HostIdentityStatus {
                installation_id: identity.installation_id,
                signing_public_key: identity.signing_public_key,
                key_version: identity.key_version,
            })
        }
        ClientRequest::CommercialOffers => {
            let installation = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .installation_identity
                .clone();
            match HttpUpgradeBackend::official()
                .and_then(|backend| backend.commercial_offers(&installation))
            {
                Ok(catalog) => ServerResponse::CommercialOffers(catalog),
                Err(error) => protocol_error("commercial_offers_failed", error.to_string()),
            }
        }
        ClientRequest::CreateCommercialCheckout { offer_id } => {
            let installation = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .installation_identity
                .clone();
            match HttpUpgradeBackend::official()
                .and_then(|backend| backend.create_checkout(&installation, offer_id.trim()))
            {
                Ok(checkout) => ServerResponse::CommercialCheckout(checkout),
                Err(error) => protocol_error("commercial_checkout_failed", error.to_string()),
            }
        }
        ClientRequest::CommercialEntitlement => {
            let installation = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .installation_identity
                .clone();
            match HttpUpgradeBackend::official()
                .and_then(|backend| backend.entitlement(&installation))
            {
                Ok(entitlement) => ServerResponse::CommercialEntitlement(entitlement),
                Err(error) => protocol_error("commercial_entitlement_failed", error.to_string()),
            }
        }
        ClientRequest::RedeemCommercialLicense { mut license_key } => {
            let installation = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .installation_identity
                .clone();
            let secret = LicenseKey::new(std::mem::take(&mut license_key));
            let response = HttpUpgradeBackend::official()
                .and_then(|backend| backend.redeem(&installation, &secret));
            // The deserialized protocol buffer is cleared before returning;
            // LicenseKey clears the owned replacement on drop.
            license_key.clear();
            match response {
                Ok(entitlement) => ServerResponse::CommercialEntitlement(entitlement),
                Err(error) => protocol_error("commercial_redemption_failed", error.to_string()),
            }
        }
        ClientRequest::CommercialBillingManagement => {
            let installation = state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .installation_identity
                .clone();
            match HttpUpgradeBackend::official()
                .and_then(|backend| backend.billing_management(&installation))
            {
                Ok(session) => ServerResponse::CommercialBillingManagement(session),
                Err(error) => {
                    protocol_error("commercial_billing_management_failed", error.to_string())
                }
            }
        }
        ClientRequest::CheckCommercialRelease => current_commercial_release(state, false),
        ClientRequest::CurrentCommercialRelease => current_commercial_release(state, true),
        ClientRequest::CheckRemoteProject { project_id } => check_remote_project(state, project_id),
        ClientRequest::ApprovePermission { request_id } => respond_permission_for_target(
            state,
            request_id,
            codex_app_server::PermissionDecision::ApproveOnce,
        ),
        ClientRequest::ApprovePermissionForSession { request_id } => {
            respond_permission_for_target(
                state,
                request_id,
                codex_app_server::PermissionDecision::ApproveForSession,
            )
        }
        ClientRequest::DenyPermission { request_id, .. } => respond_permission_for_target(
            state,
            request_id,
            codex_app_server::PermissionDecision::Deny,
        ),
        ClientRequest::StartCodex { .. }
        | ClientRequest::SubscribeEvents { .. }
        | ClientRequest::SubscribeAttention => protocol_error(
            "unsupported_request",
            "request is not implemented by this runtime",
        ),
        request => edition::handle_request(request, state),
    }
}

fn current_commercial_release(
    state: Arc<Mutex<RuntimeState>>,
    require_idle_runtime: bool,
) -> ServerResponse {
    let installation = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if require_idle_runtime {
            let active_session_count = state.runtime_status().active_session_count;
            if let Some(deferred) = commercial_upgrade_deferred(active_session_count) {
                return deferred;
            }
        }
        state.installation_identity.clone()
    };
    let verified = HttpUpgradeBackend::official()
        .and_then(|backend| backend.current_release(&installation))
        .and_then(|release| {
            let public_key = option_env!("DITCH_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64")
                .ok_or_else(|| {
                    ditch_upgrade::UpgradeError::InvalidResponse(
                        "this source build is not configured for official Commercial artifacts"
                            .to_owned(),
                    )
                })
                .and_then(|value| {
                    STANDARD.decode(value).map_err(|_| {
                        ditch_upgrade::UpgradeError::InvalidResponse(
                            "release verification key is invalid".to_owned(),
                        )
                    })
                })?;
            let team_id = option_env!("DITCH_APPLE_TEAM_ID").ok_or_else(|| {
                ditch_upgrade::UpgradeError::InvalidResponse(
                    "official Apple Team ID is not configured".to_owned(),
                )
            })?;
            let community_build = option_env!("DITCH_BUILD_NUMBER")
                .or(option_env!("DITCH_COMMUNITY_BUILD_SEQUENCE"))
                .unwrap_or("0")
                .parse::<u64>()
                .map_err(|_| {
                    ditch_upgrade::UpgradeError::InvalidResponse(
                        "Community build sequence is invalid".to_owned(),
                    )
                })?;
            let installed_release_sequence = option_env!("DITCH_RELEASE_SEQUENCE")
                .unwrap_or("0")
                .parse::<u64>()
                .map_err(|_| {
                    ditch_upgrade::UpgradeError::InvalidResponse(
                        "installed release sequence is invalid".to_owned(),
                    )
                })?;
            ReleaseVerifier::new(&public_key, team_id)?.verify_manifest(
                &release,
                Utc::now(),
                community_build,
                installed_release_sequence,
            )?;
            Ok(release)
        });
    match verified {
        Ok(release) => ServerResponse::CommercialRelease(release),
        Err(error) => commercial_release_error(error),
    }
}

fn commercial_upgrade_deferred(active_session_count: usize) -> Option<ServerResponse> {
    (active_session_count > 0).then(|| {
        protocol_error(
            "commercial_upgrade_deferred",
            format!(
                "Commercial installation is waiting for {active_session_count} active agent{} to finish. Ditch will not interrupt running work.",
                if active_session_count == 1 { "" } else { "s" }
            ),
        )
    })
}

fn mark_attention_read(
    state: Arc<Mutex<RuntimeState>>,
    attention_ids: Option<&[uuid::Uuid]>,
) -> ServerResponse {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let now = Utc::now();
    let mut changed = Vec::new();
    for attention in &mut state.attention {
        let selected = attention_ids
            .map(|ids| ids.contains(&attention.id))
            .unwrap_or(true);
        if selected && attention.read_at.is_none() {
            attention.read_at = Some(now);
            changed.push(attention.clone());
        }
    }
    if changed.is_empty() {
        return ServerResponse::Accepted;
    }
    if let Err(error) = state.store.mark_attention_read(&changed) {
        for attention in &mut state.attention {
            if changed.iter().any(|item| item.id == attention.id) {
                attention.read_at = None;
            }
        }
        return protocol_error("attention_read_failed", error.to_string());
    }
    state.broadcast(ServerEvent::AttentionRead {
        attention_ids: changed.iter().map(|attention| attention.id).collect(),
    });
    ServerResponse::Accepted
}

fn project_by_id(state: &Arc<Mutex<RuntimeState>>, project_id: ProjectId) -> Option<Project> {
    state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .projects
        .values()
        .find(|project| project.id == project_id)
        .cloned()
}

fn forward_project_command(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    request: ClientRequest,
) -> ServerResponse {
    let Some(alias) = ssh_remote::remote_alias(project) else {
        return protocol_error("wrong_execution_target", "project is local");
    };
    let connections = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .remote_connections
        .clone();
    match connections.request(alias, request) {
        Ok(response) => response,
        Err(error) => protocol_error("remote_unavailable", error.to_string()),
    }
}

fn project_file_request_for_target(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    request: ClientRequest,
) -> ServerResponse {
    let Some(project) = project_by_id(&state, project_id) else {
        return protocol_error("project_not_found", "project was not found");
    };
    if project.is_remote() {
        return match request {
            ClientRequest::ListProjectDirectory { .. } | ClientRequest::ReadProjectFile { .. } => {
                forward_project_command(&state, &project, request)
            }
            ClientRequest::WriteProjectFile { .. } => protocol_error(
                "remote_source_editing_not_supported",
                "remote project files are read-only in the desktop file browser",
            ),
            _ => protocol_error("unsupported_request", "not a project file request"),
        };
    }
    match request {
        ClientRequest::ListProjectDirectory { relative_path, .. } => {
            list_project_directory(state, project_id, &relative_path)
        }
        ClientRequest::ReadProjectFile { relative_path, .. } => {
            read_project_file(state, project_id, &relative_path)
        }
        ClientRequest::WriteProjectFile {
            relative_path,
            expected_revision,
            content,
            ..
        } => write_project_file(
            state,
            project_id,
            &relative_path,
            expected_revision.as_deref(),
            &content,
        ),
        _ => protocol_error("unsupported_request", "not a project file request"),
    }
}

fn open_project_terminal_for_target(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    let Some(project) = project_by_id(&state, project_id) else {
        return protocol_error("project_not_found", "project was not found");
    };
    if !project.is_remote() {
        return open_project_terminal(state, project_id, columns, rows);
    }
    let Some(alias) = ssh_remote::remote_alias(&project).map(str::to_owned) else {
        return protocol_error(
            "wrong_execution_target",
            "remote project has no SSH host alias",
        );
    };
    ensure_remote_event_subscription(&state, &alias);
    let connections = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .remote_connections
        .clone();
    match connections.request(
        &alias,
        ClientRequest::OpenProjectTerminal {
            project_id,
            columns,
            rows,
        },
    ) {
        Ok(ServerResponse::ProjectTerminal(terminal)) => {
            state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_terminal_hosts
                .insert(terminal.id, alias);
            ServerResponse::ProjectTerminal(terminal)
        }
        Ok(response) => response,
        Err(error) => protocol_error("remote_unavailable", error.to_string()),
    }
}

fn write_project_terminal_for_target(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: Uuid,
    data: Vec<u8>,
) -> ServerResponse {
    let remote = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_terminal_hosts
            .get(&terminal_id)
            .cloned()
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    if let Some((alias, connections)) = remote {
        return connections
            .request(
                &alias,
                ClientRequest::WriteProjectTerminal { terminal_id, data },
            )
            .unwrap_or_else(|error| protocol_error("remote_unavailable", error.to_string()));
    }
    write_project_terminal(state, terminal_id, data)
}

fn resize_project_terminal_for_target(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: Uuid,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    let remote = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_terminal_hosts
            .get(&terminal_id)
            .cloned()
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    if let Some((alias, connections)) = remote {
        return connections
            .request(
                &alias,
                ClientRequest::ResizeProjectTerminal {
                    terminal_id,
                    columns,
                    rows,
                },
            )
            .unwrap_or_else(|error| protocol_error("remote_unavailable", error.to_string()));
    }
    resize_project_terminal(state, terminal_id, columns, rows)
}

fn close_project_terminal_for_target(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: Uuid,
) -> ServerResponse {
    let remote = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_terminal_hosts
            .remove(&terminal_id)
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    if let Some((alias, connections)) = remote {
        return connections
            .request(&alias, ClientRequest::CloseProjectTerminal { terminal_id })
            .unwrap_or_else(|error| protocol_error("remote_unavailable", error.to_string()));
    }
    close_project_terminal(state, terminal_id)
}

fn setup_terminal_request_for_target(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: Uuid,
    request: ClientRequest,
) -> ServerResponse {
    let remote = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_terminal_hosts
            .get(&terminal_id)
            .cloned()
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    if let Some((alias, connections)) = remote {
        let closing = matches!(request, ClientRequest::CloseSetupTerminal { .. });
        let response = connections
            .request(&alias, request)
            .unwrap_or_else(|error| protocol_error("remote_unavailable", error.to_string()));
        if closing && matches!(response, ServerResponse::Accepted) {
            state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .remote_terminal_hosts
                .remove(&terminal_id);
        }
        return response;
    }
    match request {
        ClientRequest::WriteSetupTerminal { data, .. } => {
            write_setup_terminal(state, terminal_id, data)
        }
        ClientRequest::ResizeSetupTerminal { columns, rows, .. } => {
            resize_setup_terminal(state, terminal_id, columns, rows)
        }
        ClientRequest::TakeSetupTerminalOutput { .. } => {
            take_setup_terminal_output(state, terminal_id)
        }
        ClientRequest::CloseSetupTerminal { .. } => close_setup_terminal(state, terminal_id),
        _ => protocol_error("unsupported_request", "not a setup terminal request"),
    }
}

fn open_codex_authentication(
    state: Arc<Mutex<RuntimeState>>,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    let binary = active_codex_binary(&state).or_else(|| {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".local/bin/codex"))
            .filter(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
    });
    let Some(binary) = binary else {
        return protocol_error("codex_missing", "Codex is not installed on this machine");
    };
    let mut command = CommandBuilder::new(binary);
    command.arg("login");
    command.env("TERM", "xterm-256color");
    open_setup_command(state, command, "codex_authentication", columns, rows)
}

fn open_codex_sandbox_setup(
    state: Arc<Mutex<RuntimeState>>,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    if std::env::consts::OS != "linux" {
        return protocol_error(
            "codex_sandbox_setup_unsupported",
            "Codex sandbox setup is only required on Linux",
        );
    }
    const SCRIPT: &str = r#"set -eu
. /etc/os-release 2>/dev/null || true
run_as_root() {
  if [ "$(id -u)" = 0 ]; then
    "$@"
  else
    command -v sudo >/dev/null 2>&1 || { echo 'This host requires an administrator to install the Codex sandbox prerequisites.' >&2; exit 74; }
    sudo "$@"
  fi
}
if [ "$(id -u)" != 0 ]; then
  command -v sudo >/dev/null 2>&1 || { echo 'This host requires an administrator to install the Codex sandbox prerequisites.' >&2; exit 74; }
  sudo -v
fi
case "${ID:-}" in
  ubuntu|debian)
    run_as_root apt-get update
    run_as_root apt-get install -y bubblewrap
    if [ "${ID:-}" = ubuntu ] && [ "${VERSION_ID:-}" = 24.04 ] && [ -r /proc/sys/kernel/apparmor_restrict_unprivileged_userns ] && [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns)" = 1 ]; then
      run_as_root apt-get install -y apparmor-profiles apparmor-utils
      source_profile=/usr/share/apparmor/extra-profiles/bwrap-userns-restrict
      [ -r "$source_profile" ] || { echo 'Ubuntu bubblewrap AppArmor profile is unavailable.' >&2; exit 73; }
      run_as_root install -m 0644 "$source_profile" /etc/apparmor.d/bwrap-userns-restrict
      run_as_root apparmor_parser -r /etc/apparmor.d/bwrap-userns-restrict
    fi
    ;;
  fedora)
    run_as_root dnf install -y bubblewrap
    ;;
  *)
    echo 'Automatic Codex sandbox setup is unavailable for this Linux distribution.' >&2
    exit 72
    ;;
esac
codex_binary=$(command -v codex 2>/dev/null || true)
[ -n "$codex_binary" ] || codex_binary="$HOME/.local/bin/codex"
"$codex_binary" sandbox -P :workspace -C "$HOME" /bin/true
printf '\nCodex sandbox is ready.\n'
"#;
    let mut command = CommandBuilder::new("/bin/sh");
    command.args(["-c", SCRIPT]);
    command.env("TERM", "xterm-256color");
    open_setup_command(state, command, "codex_sandbox_setup", columns, rows)
}

fn open_setup_command(
    state: Arc<Mutex<RuntimeState>>,
    command: CommandBuilder,
    purpose: &str,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    let pair = match native_pty_system().openpty(terminal_size(columns, rows)) {
        Ok(pair) => pair,
        Err(error) => return protocol_error("terminal_open_failed", error.to_string()),
    };
    let child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(error) => return protocol_error("codex_auth_failed", error.to_string()),
    };
    drop(pair.slave);
    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => return protocol_error("terminal_writer_failed", error.to_string()),
    };
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => return protocol_error("terminal_reader_failed", error.to_string()),
    };
    let descriptor = SetupTerminal {
        id: Uuid::new_v4(),
        purpose: purpose.into(),
    };
    let output = Arc::new(Mutex::new(Vec::new()));
    let exited = Arc::new(AtomicBool::new(false));
    let reader_output = Arc::clone(&output);
    let reader_exited = Arc::clone(&exited);
    thread::spawn(move || {
        let mut reader = reader;
        let mut bytes = [0_u8; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let mut output = reader_output
                        .lock()
                        .expect("setup terminal output lock poisoned");
                    output.extend_from_slice(&bytes[..count]);
                    if output.len() > 1024 * 1024 {
                        let excess = output.len() - 1024 * 1024;
                        output.drain(..excess);
                    }
                }
            }
        }
        reader_exited.store(true, Ordering::Release);
    });
    state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .setup_terminals
        .insert(
            descriptor.id,
            SetupTerminalRecord {
                master: Arc::new(Mutex::new(pair.master)),
                writer: Arc::new(Mutex::new(writer)),
                child,
                output,
                exited,
            },
        );
    ServerResponse::SetupTerminal(descriptor)
}

fn write_setup_terminal(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: Uuid,
    data: Vec<u8>,
) -> ServerResponse {
    let writer = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .setup_terminals
        .get(&terminal_id)
        .map(|record| Arc::clone(&record.writer));
    let Some(writer) = writer else {
        return protocol_error("terminal_not_found", "setup terminal was not found");
    };
    match writer
        .lock()
        .expect("setup terminal writer lock poisoned")
        .write_all(&data)
    {
        Ok(()) => ServerResponse::Accepted,
        Err(error) => protocol_error("terminal_write_failed", error.to_string()),
    }
}

fn resize_setup_terminal(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: Uuid,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    let master = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .setup_terminals
        .get(&terminal_id)
        .map(|record| Arc::clone(&record.master));
    let Some(master) = master else {
        return protocol_error("terminal_not_found", "setup terminal was not found");
    };
    match master
        .lock()
        .expect("setup terminal master lock poisoned")
        .resize(terminal_size(columns, rows))
    {
        Ok(()) => ServerResponse::Accepted,
        Err(error) => protocol_error("terminal_resize_failed", error.to_string()),
    }
}

fn take_setup_terminal_output(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: Uuid,
) -> ServerResponse {
    let values = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .setup_terminals
        .get(&terminal_id)
        .map(|record| (Arc::clone(&record.output), Arc::clone(&record.exited)));
    let Some((output, exited)) = values else {
        return protocol_error("terminal_not_found", "setup terminal was not found");
    };
    let data = std::mem::take(&mut *output.lock().expect("setup terminal output lock poisoned"));
    ServerResponse::SetupTerminalOutput(SetupTerminalOutput {
        terminal_id,
        data,
        exited: exited.load(Ordering::Acquire),
    })
}

fn close_setup_terminal(state: Arc<Mutex<RuntimeState>>, terminal_id: Uuid) -> ServerResponse {
    let terminal = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .setup_terminals
        .remove(&terminal_id);
    if let Some(mut terminal) = terminal {
        let _ = terminal.child.kill();
    }
    ServerResponse::Accepted
}

fn delete_project_for_target(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
) -> ServerResponse {
    let Some(project) = project_by_id(&state, project_id) else {
        return protocol_error("project_not_found", "project was not found");
    };
    if project.is_remote() {
        let response = forward_project_command(
            &state,
            &project,
            ClientRequest::DeleteProject { project_id },
        );
        if !matches!(response, ServerResponse::Accepted) {
            return response;
        }
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if let Err(error) = state.store.delete_project(project_id) {
            return protocol_error("project_delete_failed", error.to_string());
        }
        let agent_ids = state
            .agents
            .values()
            .filter_map(|record| (record.run.project_id == project_id).then_some(record.run.id))
            .collect::<HashSet<_>>();
        state.agents.retain(|id, _| !agent_ids.contains(id));
        state
            .remote_agent_hosts
            .retain(|id, _| !agent_ids.contains(id));
        state.attention.retain(|item| {
            item.project_id != Some(project_id)
                && item.agent_id.is_none_or(|id| !agent_ids.contains(&id))
        });
        state
            .projects
            .retain(|_, candidate| candidate.id != project_id);
        state.broadcast(ServerEvent::ProjectDeleted { project_id });
        return ServerResponse::Accepted;
    }
    delete_project(state, project_id)
}

fn create_remote_project(
    state: Arc<Mutex<RuntimeState>>,
    alias: String,
    name: String,
    remote_root: String,
    git_policy: ProjectGitPolicy,
) -> ServerResponse {
    let connections = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .remote_connections
        .clone();
    let machine_id = match connections.request(&alias, ClientRequest::HostIdentityStatus) {
        Ok(ServerResponse::HostIdentity(status)) => Some(status.installation_id),
        Ok(ServerResponse::Error(error)) => return ServerResponse::Error(error),
        Ok(_) => None,
        Err(error) => return protocol_error("remote_unavailable", error.to_string()),
    };
    let Some(machine_id) = machine_id else {
        return protocol_error(
            "remote_identity_failed",
            "The remote daemon did not provide a stable machine identity.",
        );
    };
    let project = match ssh_remote::create_remote_project(
        &connections,
        &alias,
        machine_id,
        name,
        remote_root,
        git_policy,
    ) {
        Ok(project) => project,
        Err(error) => return protocol_error("remote_project_failed", error.to_string()),
    };
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if let Err(error) = state.store.upsert_project(&project) {
        return protocol_error("project_store_failed", error.to_string());
    }
    state.projects.insert(project.root_key(), project.clone());
    state.broadcast(ServerEvent::ProjectChanged(project.clone()));
    ssh_remote::observe("remote_project_added", &alias, None);
    ServerResponse::ProjectCreated(project)
}

fn cache_remote_agent(
    state: &Arc<Mutex<RuntimeState>>,
    alias: &str,
    project: &Project,
    run: &AgentRun,
) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state.remote_agent_hosts.insert(run.id, alias.to_owned());
    state.agents.insert(
        run.id,
        AgentRecord {
            run: run.clone(),
            project_root: project.root.clone(),
            allow_non_git: project.git_policy == ProjectGitPolicy::AllowOutsideGit,
            messages: Vec::new(),
            terminal_failure: None,
        },
    );
    state.broadcast(ServerEvent::AgentChanged(run.clone()));
}

fn forward_remote_start(
    state: Arc<Mutex<RuntimeState>>,
    project: Project,
    _project_name: String,
    _project_root: String,
    prompt: String,
    _mode: CodexLaunchMode,
    execution_profile: AgentExecutionProfile,
) -> ServerResponse {
    let Some(alias) = ssh_remote::remote_alias(&project).map(str::to_owned) else {
        return protocol_error("wrong_execution_target", "project is not remote");
    };
    let connections = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .remote_connections
        .clone();
    ensure_remote_event_subscription(&state, &alias);
    let request = ClientRequest::StartRemoteCodexAppServerSession {
        project_id: project.id,
        project_name: project.name.clone(),
        project_root: project.root.to_string_lossy().into_owned(),
        prompt,
        execution_profile,
    };
    match connections.request(&alias, request) {
        Ok(ServerResponse::AgentStarted(run)) => {
            cache_remote_agent(&state, &alias, &project, &run);
            ServerResponse::AgentStarted(run)
        }
        Ok(response) => response,
        Err(error) => protocol_error("remote_unavailable", error.to_string()),
    }
}

fn forward_remote_resume(
    state: Arc<Mutex<RuntimeState>>,
    project: Project,
    _project_name: String,
    _project_root: String,
    thread_id: String,
    prompt: String,
    execution_profile: AgentExecutionProfile,
) -> ServerResponse {
    let Some(alias) = ssh_remote::remote_alias(&project).map(str::to_owned) else {
        return protocol_error("wrong_execution_target", "project is not remote");
    };
    let connections = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .remote_connections
        .clone();
    ensure_remote_event_subscription(&state, &alias);
    let request = ClientRequest::ResumeRemoteCodexAppServerSession {
        project_id: project.id,
        project_name: project.name.clone(),
        project_root: project.root.to_string_lossy().into_owned(),
        thread_id,
        prompt,
        execution_profile,
    };
    match connections.request(&alias, request) {
        Ok(ServerResponse::AgentStarted(run)) => {
            cache_remote_agent(&state, &alias, &project, &run);
            ServerResponse::AgentStarted(run)
        }
        Ok(response) => response,
        Err(error) => protocol_error("remote_unavailable", error.to_string()),
    }
}

fn forward_or_prompt_agent(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    prompt: String,
    execution_profile: AgentExecutionProfile,
) -> ServerResponse {
    let remote = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_agent_hosts
            .get(&agent_id)
            .cloned()
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    let Some((alias, connections)) = remote else {
        return prompt_agent(state, agent_id, prompt, execution_profile);
    };
    ensure_remote_event_subscription(&state, &alias);
    match connections.request(
        &alias,
        ClientRequest::PromptRemoteCodexAppServerAgent {
            agent_id,
            prompt,
            execution_profile,
        },
    ) {
        Ok(response) => response,
        Err(error) => protocol_error("remote_unavailable", error.to_string()),
    }
}

fn forward_or_stop_agent(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    force: bool,
) -> ServerResponse {
    let remote = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_agent_hosts
            .get(&agent_id)
            .cloned()
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    let Some((alias, connections)) = remote else {
        return if force {
            force_kill_agent(state, agent_id)
        } else {
            stop_agent(state, agent_id)
        };
    };
    let request = if force {
        ClientRequest::ForceKillAgent { agent_id }
    } else {
        ClientRequest::StopAgent { agent_id }
    };
    match connections.request(&alias, request) {
        Ok(response) => response,
        Err(error) => protocol_error("remote_unavailable", error.to_string()),
    }
}

fn forward_or_agent_action(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    request: ClientRequest,
) -> ServerResponse {
    let remote = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_agent_hosts
            .get(&agent_id)
            .cloned()
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    if let Some((alias, connections)) = remote {
        return connections
            .request(&alias, request)
            .unwrap_or_else(|error| protocol_error("remote_unavailable", error.to_string()));
    }
    match request {
        ClientRequest::DeleteAgent { .. } => delete_agent(state, agent_id),
        ClientRequest::RenameAgent { title, .. } => rename_agent(state, agent_id, title),
        _ => protocol_error("unsupported_request", "not an agent action"),
    }
}

fn mark_attention_read_for_targets(
    state: Arc<Mutex<RuntimeState>>,
    requested: Option<Vec<Uuid>>,
) -> ServerResponse {
    let (by_host, local_ids, connections) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let selected = requested
            .clone()
            .unwrap_or_else(|| state.attention.iter().map(|item| item.id).collect());
        let mut by_host: HashMap<String, Vec<Uuid>> = HashMap::new();
        let mut local = Vec::new();
        for id in selected {
            if let Some(host) = state.remote_attention_hosts.get(&id) {
                by_host.entry(host.clone()).or_default().push(id);
            } else {
                local.push(id);
            }
        }
        (by_host, local, state.remote_connections.clone())
    };
    for (alias, ids) in by_host {
        let request = if requested.is_none() {
            ClientRequest::MarkAllAttentionRead
        } else {
            ClientRequest::MarkAttentionRead { attention_ids: ids }
        };
        match connections.request(&alias, request) {
            Ok(ServerResponse::Accepted) => {}
            Ok(ServerResponse::Error(error)) => return ServerResponse::Error(error),
            Ok(_) => {
                return protocol_error("remote_protocol_error", "unexpected attention response");
            }
            Err(error) => return protocol_error("remote_unavailable", error.to_string()),
        }
    }
    if local_ids.is_empty() {
        ServerResponse::Accepted
    } else {
        mark_attention_read(state, Some(&local_ids))
    }
}

fn check_remote_project(state: Arc<Mutex<RuntimeState>>, project_id: ProjectId) -> ServerResponse {
    let Some(project) = project_by_id(&state, project_id) else {
        return protocol_error("project_not_found", "project was not found");
    };
    let Some(alias) = ssh_remote::remote_alias(&project) else {
        return protocol_error("wrong_execution_target", "project is local");
    };
    let connections = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .remote_connections
        .clone();
    match connections.request(alias, ClientRequest::Snapshot) {
        Ok(ServerResponse::Snapshot(snapshot))
            if snapshot
                .projects
                .iter()
                .any(|remote| remote.id == project_id) =>
        {
            ServerResponse::Accepted
        }
        Ok(ServerResponse::Snapshot(_)) => protocol_error(
            "remote_project_missing",
            "The remote daemon no longer has this project registration.",
        ),
        Ok(ServerResponse::Error(error)) => ServerResponse::Error(error),
        Ok(_) => protocol_error(
            "remote_protocol_error",
            "remote daemon returned an unexpected response",
        ),
        Err(error) => protocol_error("remote_unavailable", error.to_string()),
    }
}

fn list_filesystem_directory(absolute_path: &str) -> ServerResponse {
    let path = Path::new(absolute_path);
    if !path.is_absolute() || absolute_path.len() > 4096 || absolute_path.contains('\0') {
        return protocol_error(
            "invalid_remote_path",
            "path must be an absolute directory path",
        );
    }
    let path = match path.canonicalize() {
        Ok(path) if path.is_dir() => path,
        Ok(_) => return protocol_error("not_a_directory", "path is not a directory"),
        Err(error) => return protocol_error("directory_read_failed", error.to_string()),
    };
    let mut entries = match fs::read_dir(&path) {
        Ok(entries) => entries
            .flatten()
            .take(20_000)
            .filter_map(|entry| {
                let path = entry.path();
                let link_metadata = fs::symlink_metadata(&path).ok()?;
                let metadata = fs::metadata(&path).ok()?;
                if !metadata.is_dir() {
                    return None;
                }
                Some(ditch_protocol::RemoteDirectoryEntry {
                    name: entry
                        .file_name()
                        .to_string_lossy()
                        .chars()
                        .take(1024)
                        .collect(),
                    absolute_path: path.to_string_lossy().chars().take(4096).collect(),
                    is_directory: true,
                    is_symlink: link_metadata.file_type().is_symlink(),
                })
            })
            .collect::<Vec<_>>(),
        Err(error) => return protocol_error("directory_read_failed", error.to_string()),
    };
    entries.sort_by_key(|entry| entry.name.to_lowercase());
    ServerResponse::RemoteDirectory(ditch_protocol::RemoteDirectory {
        ssh_host_alias: String::new(),
        absolute_path: path.to_string_lossy().into_owned(),
        parent_path: path
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned()),
        entries,
    })
}

fn shutdown_runtime(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    let socket_path = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let children = std::mem::take(&mut state.children);
        for child in children.values() {
            signal_process_group(child.process_group_id, libc::SIGKILL);
            let _ = child
                .child
                .lock()
                .expect("child lock should not be poisoned")
                .kill();
        }
        let mut changed = Vec::new();
        for record in state.agents.values_mut() {
            if matches!(
                record.run.state,
                AgentState::Starting
                    | AgentState::Working
                    | AgentState::Stopping
                    | AgentState::AwaitingApproval
                    | AgentState::Blocked
            ) {
                record.run.state = AgentState::Interrupted;
                record.run.last_visible_action = Some("Runtime stopped by user".to_owned());
                record.run.updated_at = Utc::now();
                changed.push(record.run.id);
            }
        }
        for agent_id in changed {
            state.persist_agent(agent_id);
        }
        state.paths.socket_path.clone()
    };

    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(100));
        let _ = fs::remove_file(socket_path);
        std::process::exit(0);
    });

    ServerResponse::Accepted
}

fn list_agent_models(state: Arc<Mutex<RuntimeState>>, provider: AgentProvider) -> ServerResponse {
    if provider != AgentProvider::Codex {
        return ServerResponse::AgentModels(Vec::new());
    }
    let binary = active_codex_binary(&state);
    let Some(binary) = binary else {
        return protocol_error(
            "codex_not_found",
            "No working Codex CLI installation was found for this user",
        );
    };
    match discover_codex_models(&binary) {
        Ok(models) => ServerResponse::AgentModels(models),
        Err(error) => protocol_error("codex_models_failed", error.to_string()),
    }
}

fn terminal_size(columns: u16, rows: u16) -> PtySize {
    PtySize {
        cols: columns.clamp(2, 500),
        rows: rows.clamp(2, 300),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn open_project_terminal(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ditch_core::ProjectId,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    let (project_root, existing) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let project = state
            .projects
            .values()
            .find(|project| project.id == project_id);
        let Some(project) = project else {
            return protocol_error("project_not_found", "The selected project no longer exists");
        };
        let existing = state
            .terminal_by_project
            .get(&project_id)
            .and_then(|id| state.terminals.get(id))
            .map(|record| record.descriptor.clone());
        (project.root.clone(), existing)
    };
    if let Some(existing) = existing {
        return ServerResponse::ProjectTerminal(existing);
    }

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(terminal_size(columns, rows)) {
        Ok(pair) => pair,
        Err(error) => return protocol_error("terminal_open_failed", error.to_string()),
    };
    let mut command = CommandBuilder::new(&shell);
    command.arg("-l");
    command.cwd(&project_root);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    let mut child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(error) => return protocol_error("terminal_spawn_failed", error.to_string()),
    };
    drop(pair.slave);
    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => return protocol_error("terminal_writer_failed", error.to_string()),
    };
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => return protocol_error("terminal_reader_failed", error.to_string()),
    };
    let terminal = ProjectTerminal {
        id: uuid::Uuid::new_v4(),
        project_id,
        shell: shell.clone(),
    };
    let terminal_id = terminal.id;
    let output_state = Arc::clone(&state);
    thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(bytes) => {
                    let mut state = output_state
                        .lock()
                        .expect("runtime state lock should not be poisoned");
                    state.broadcast(ServerEvent::ProjectTerminalOutput {
                        terminal_id,
                        data: buffer[..bytes].to_vec(),
                    });
                }
            }
        }
        let mut state = output_state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state.broadcast(ServerEvent::ProjectTerminalExited { terminal_id });
    });

    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    // A request can race with another UI connection. Keep the first terminal
    // for the project and terminate this just-created duplicate safely.
    if let Some(existing_id) = state.terminal_by_project.get(&project_id).copied()
        && let Some(existing) = state.terminals.get(&existing_id)
    {
        let _ = child.kill();
        return ServerResponse::ProjectTerminal(existing.descriptor.clone());
    }
    state.terminal_by_project.insert(project_id, terminal_id);
    state.terminals.insert(
        terminal_id,
        ProjectTerminalRecord {
            descriptor: terminal.clone(),
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child,
        },
    );
    ServerResponse::ProjectTerminal(terminal)
}

fn write_project_terminal(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: uuid::Uuid,
    data: Vec<u8>,
) -> ServerResponse {
    let writer = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .terminals
            .get(&terminal_id)
            .map(|record| Arc::clone(&record.writer))
    };
    let Some(writer) = writer else {
        return protocol_error(
            "terminal_not_found",
            "The terminal session is no longer active",
        );
    };
    match writer
        .lock()
        .expect("terminal writer lock should not be poisoned")
        .write_all(&data)
    {
        Ok(()) => ServerResponse::Accepted,
        Err(error) => protocol_error("terminal_write_failed", error.to_string()),
    }
}

fn resize_project_terminal(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: uuid::Uuid,
    columns: u16,
    rows: u16,
) -> ServerResponse {
    let master = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .terminals
            .get(&terminal_id)
            .map(|record| Arc::clone(&record.master))
    };
    let Some(master) = master else {
        return protocol_error(
            "terminal_not_found",
            "The terminal session is no longer active",
        );
    };
    match master
        .lock()
        .expect("terminal master lock should not be poisoned")
        .resize(terminal_size(columns, rows))
    {
        Ok(()) => ServerResponse::Accepted,
        Err(error) => protocol_error("terminal_resize_failed", error.to_string()),
    }
}

fn close_project_terminal(
    state: Arc<Mutex<RuntimeState>>,
    terminal_id: uuid::Uuid,
) -> ServerResponse {
    let mut terminal = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(terminal) = state.terminals.remove(&terminal_id) else {
            return ServerResponse::Accepted;
        };
        state
            .terminal_by_project
            .remove(&terminal.descriptor.project_id);
        terminal
    };
    let _ = terminal.child.kill();
    ServerResponse::Accepted
}

fn discover_codex_models(binary: &str) -> io::Result<Vec<AgentModel>> {
    let mut command = Command::new(binary);
    command.args(["app-server", "--stdio"]);
    if let Some(path) = effective_path_for_binary(Path::new(binary)) {
        command.env("PATH", path);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing app-server stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing app-server stdout"))?;
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"the_ditch","title":"The Ditch","version":env!("CARGO_PKG_VERSION")}}})
    )?;
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"method":"initialized","params":{}})
    )?;
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"id":2,"method":"model/list","params":{"limit":100,"includeHidden":false}})
    )?;
    stdin.flush()?;
    let mut models = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let value: Value = serde_json::from_str(&line?)?;
        if value.get("id").and_then(Value::as_i64) != Some(2) {
            continue;
        }
        if let Some(error) = value.get("error") {
            let _ = child.kill();
            return Err(io::Error::other(error.to_string()));
        }
        for item in value
            .pointer("/result/data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                continue;
            };
            let efforts = item
                .get("supportedReasoningEfforts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("reasoningEffort").and_then(Value::as_str))
                .map(str::to_owned)
                .collect();
            models.push(AgentModel {
                id: id.to_owned(),
                display_name: item
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned(),
                is_default: item
                    .get("isDefault")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                default_reasoning_effort: item
                    .get("defaultReasoningEffort")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                supported_reasoning_efforts: efforts,
                context_window_tokens: item
                    .get("contextWindowTokens")
                    .or_else(|| item.get("contextWindow"))
                    .or_else(|| item.get("context_window_tokens"))
                    .and_then(|value| {
                        value
                            .as_u64()
                            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                    }),
            });
        }
        break;
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(models)
}

include!("remote_app_server_runtime.rs");

fn start_codex_session(
    state: Arc<Mutex<RuntimeState>>,
    project_name: String,
    project_root: String,
    prompt: String,
    mode: CodexLaunchMode,
    execution_profile: ditch_core::AgentExecutionProfile,
) -> ServerResponse {
    let project = project_for_launch(&state, project_name, project_root);
    if let Err(error) = ensure_project_metadata(&project) {
        return protocol_error("project_metadata_failed", error.to_string());
    }

    let now = Utc::now();
    let mut run = AgentRun {
        id: AgentId::new(),
        provider: AgentProvider::Codex,
        state: AgentState::Starting,
        can_stop: false,
        launch_mode: mode.clone(),
        execution_profile: execution_profile.clone(),
        project_id: project.id,
        task_id: None,
        pane_id: None,
        native_session_id: None,
        codex_title: None,
        user_title: None,
        origin_codex_home: std::env::var("CODEX_HOME").ok(),
        current_prompt: Some(prompt.clone()),
        last_visible_action: Some("Starting Codex".to_owned()),
        state_confidence: 0.8,
        state_evidence: "Ditch Runtime accepted the session and is launching Codex.".to_owned(),
        started_at: now,
        updated_at: now,
        finished_at: None,
        exit_code: None,
        resume_block_reason: None,
    };

    if let Err(error) = verify_project_git_policy(&project) {
        return error;
    }
    let allow_non_git = project.git_policy == ProjectGitPolicy::AllowOutsideGit;
    let user_message = AgentChatMessage {
        agent_id: run.id,
        role: AgentChatRole::User,
        text: prompt.clone(),
        created_at: Utc::now(),
    };
    let binary = active_codex_binary(&state);
    let Some(binary) = binary else {
        return persist_launch_failure(
            &state,
            project,
            run,
            user_message,
            allow_non_git,
            "No working Codex CLI installation was found. Choose an existing installation in Ditch settings.".to_owned(),
        );
    };
    let child = match spawn_codex_child(
        &binary,
        &project.root,
        &prompt,
        &mode,
        None,
        allow_non_git,
        &execution_profile,
    ) {
        Ok(child) => Arc::new(Mutex::new(child)),
        Err(error) => {
            return persist_launch_failure(
                &state,
                project,
                run,
                user_message,
                allow_non_git,
                error.to_string(),
            );
        }
    };
    let process_group_id = child
        .lock()
        .expect("child lock should not be poisoned")
        .id() as i32;

    let run_id = uuid::Uuid::new_v4();
    run.state = AgentState::Working;
    run.can_stop = true;
    run.updated_at = Utc::now();
    run.state_evidence = "Codex process is running under Ditch Runtime.".to_owned();

    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if let Err(error) = state.store.upsert_project(&project) {
            let _ = child
                .lock()
                .expect("child lock should not be poisoned")
                .kill();
            return protocol_error("project_store_failed", error.to_string());
        }
        let home = state
            .codex_home
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned());
        if let Err(error) = state
            .store
            .persist_new_agent(&run, &user_message, home.as_deref())
        {
            let _ = child
                .lock()
                .expect("child lock should not be poisoned")
                .kill();
            return protocol_error("agent_store_failed", error.to_string());
        }
        state.projects.insert(project.root_key(), project.clone());
        state.agents.insert(
            run.id,
            AgentRecord {
                run: run.clone(),
                project_root: project.root.clone(),
                allow_non_git,
                messages: vec![user_message.clone()],
                terminal_failure: None,
            },
        );
        state.children.insert(
            run.id,
            ActiveAgentChild {
                run_id,
                process_group_id,
                child: Arc::clone(&child),
                app_server: false,
            },
        );
        state.broadcast(ServerEvent::ProjectChanged(project));
        state.broadcast(ServerEvent::AgentChanged(run.clone()));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), run.id, run_id, child);
    ServerResponse::AgentStarted(run)
}

fn persist_launch_failure(
    state: &Arc<Mutex<RuntimeState>>,
    project: Project,
    mut run: AgentRun,
    user_message: AgentChatMessage,
    allow_non_git: bool,
    error: String,
) -> ServerResponse {
    run.state = AgentState::Failed;
    run.updated_at = Utc::now();
    run.last_visible_action = Some("Codex could not be started".to_owned());
    run.state_evidence = error.clone();
    run.finished_at = Some(run.updated_at);
    run.resume_block_reason = run
        .native_session_id
        .is_none()
        .then_some(AgentResumeBlockReason::NoCodexThread);
    let system_message = AgentChatMessage {
        agent_id: run.id,
        role: AgentChatRole::System,
        text: error.clone(),
        created_at: Utc::now(),
    };
    let attention = RuntimeAttention {
        id: uuid::Uuid::new_v4(),
        kind: AttentionKind::Failed,
        agent_id: Some(run.id),
        project_id: Some(project.id),
        project_name: Some(project.name.clone()),
        agent_name: Some(agent_display_name(&run)),
        title: "Codex failed to start".to_owned(),
        body: notification_summary(&error),
        created_at: Utc::now(),
        read_at: None,
    };
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let home = state
        .codex_home
        .as_ref()
        .map(|value| value.to_string_lossy().into_owned());
    let persisted = state.store.upsert_project(&project).and_then(|_| {
        state.store.persist_failed_agent(
            &run,
            &user_message,
            &system_message,
            &attention,
            home.as_deref(),
        )
    });
    if let Err(store_error) = persisted {
        return protocol_error("agent_store_failed", store_error.to_string());
    }
    state.projects.insert(project.root_key(), project.clone());
    state.agents.insert(
        run.id,
        AgentRecord {
            run: run.clone(),
            project_root: project.root.clone(),
            allow_non_git,
            messages: vec![user_message.clone(), system_message.clone()],
            terminal_failure: Some(error.clone()),
        },
    );
    state.attention.push(attention.clone());
    state.broadcast(ServerEvent::ProjectChanged(project));
    state.broadcast(ServerEvent::AgentChanged(run));
    state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    state.broadcast(ServerEvent::AgentMessageAppended(system_message));
    state.broadcast(ServerEvent::AttentionRaised(attention));
    protocol_error("codex_start_failed", error)
}

fn project_for_launch(
    state: &Arc<Mutex<RuntimeState>>,
    project_name: String,
    project_root: String,
) -> Project {
    let root = canonical_project_root(Path::new(&project_root));
    let root_key = root.to_string_lossy();
    state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .projects
        .get(root_key.as_ref())
        .cloned()
        .or_else(|| load_project_config(&root).ok())
        .unwrap_or_else(|| Project::new(project_name, root))
}

#[allow(clippy::result_large_err)]
fn verify_project_git_policy(project: &Project) -> Result<(), ServerResponse> {
    if project.git_policy == ProjectGitPolicy::AllowOutsideGit {
        return Ok(());
    }
    let is_repository = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&project.root)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if is_repository {
        Ok(())
    } else {
        Err(protocol_error(
            "git_repository_required",
            "This project is not inside a Git repository. Initialize Git or explicitly allow Codex outside Git.",
        ))
    }
}

fn resume_codex_session(
    state: Arc<Mutex<RuntimeState>>,
    project_name: String,
    project_root: String,
    thread_id: String,
    prompt: String,
    execution_profile: ditch_core::AgentExecutionProfile,
) -> ServerResponse {
    let project = project_for_launch(&state, project_name, project_root);
    if let Err(message) = validate_thread_project(&state, &thread_id, &project.root) {
        return protocol_error("codex_thread_project_mismatch", message);
    }
    if let Err(error) = ensure_project_metadata(&project) {
        return protocol_error("project_metadata_failed", error.to_string());
    }

    let now = Utc::now();
    let mut run = AgentRun {
        id: AgentId::new(),
        provider: AgentProvider::Codex,
        state: AgentState::Starting,
        can_stop: false,
        launch_mode: CodexLaunchMode::Exec,
        execution_profile: execution_profile.clone(),
        project_id: project.id,
        task_id: None,
        pane_id: None,
        native_session_id: Some(thread_id.clone()),
        codex_title: None,
        user_title: None,
        origin_codex_home: std::env::var("CODEX_HOME").ok(),
        current_prompt: Some(prompt.clone()),
        last_visible_action: Some("Resuming Codex".to_owned()),
        state_confidence: 0.8,
        state_evidence: "Ditch Runtime accepted the session and is resuming Codex.".to_owned(),
        started_at: now,
        updated_at: now,
        finished_at: None,
        exit_code: None,
        resume_block_reason: None,
    };

    if let Err(error) = verify_project_git_policy(&project) {
        return error;
    }
    let allow_non_git = project.git_policy == ProjectGitPolicy::AllowOutsideGit;
    let user_message = AgentChatMessage {
        agent_id: run.id,
        role: AgentChatRole::User,
        text: prompt.clone(),
        created_at: Utc::now(),
    };
    let binary = active_codex_binary(&state);
    let Some(binary) = binary else {
        return persist_launch_failure(
            &state,
            project,
            run,
            user_message,
            allow_non_git,
            "No working Codex CLI installation was found. Choose an existing installation in Ditch settings.".to_owned(),
        );
    };
    let child = match spawn_codex_child(
        &binary,
        &project.root,
        &prompt,
        &CodexLaunchMode::Exec,
        Some(&thread_id),
        allow_non_git,
        &execution_profile,
    ) {
        Ok(child) => Arc::new(Mutex::new(child)),
        Err(error) => {
            return persist_launch_failure(
                &state,
                project,
                run,
                user_message,
                allow_non_git,
                error.to_string(),
            );
        }
    };
    let process_group_id = child
        .lock()
        .expect("child lock should not be poisoned")
        .id() as i32;

    let run_id = uuid::Uuid::new_v4();
    run.state = AgentState::Working;
    run.can_stop = true;
    run.updated_at = Utc::now();
    run.state_evidence = "Codex resume process is running under Ditch Runtime.".to_owned();

    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if let Err(error) = state.store.upsert_project(&project) {
            let _ = child
                .lock()
                .expect("child lock should not be poisoned")
                .kill();
            return protocol_error("project_store_failed", error.to_string());
        }
        let home = state
            .codex_home
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned());
        if let Err(error) = state
            .store
            .persist_new_agent(&run, &user_message, home.as_deref())
        {
            let _ = child
                .lock()
                .expect("child lock should not be poisoned")
                .kill();
            return protocol_error("agent_store_failed", error.to_string());
        }
        state.projects.insert(project.root_key(), project.clone());
        state.agents.insert(
            run.id,
            AgentRecord {
                run: run.clone(),
                project_root: project.root.clone(),
                allow_non_git,
                messages: vec![user_message.clone()],
                terminal_failure: None,
            },
        );
        state.children.insert(
            run.id,
            ActiveAgentChild {
                run_id,
                process_group_id,
                child: Arc::clone(&child),
                app_server: false,
            },
        );
        state.broadcast(ServerEvent::ProjectChanged(project));
        state.broadcast(ServerEvent::AgentChanged(run.clone()));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), run.id, run_id, child);
    ServerResponse::AgentStarted(run)
}

fn validate_thread_project(
    state: &Arc<Mutex<RuntimeState>>,
    thread_id: &str,
    requested_root: &Path,
) -> Result<(), String> {
    let state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let original = state
        .agents
        .values()
        .find(|record| record.run.native_session_id.as_deref() == Some(thread_id));
    match original {
        Some(record) if record.project_root != requested_root => Err(format!(
            "Codex thread {thread_id} belongs to {}, not {}. Start a new Codex session for the selected project.",
            record.project_root.display(),
            requested_root.display()
        )),
        Some(record) if record.run.origin_codex_home.as_deref() != state.codex_home.as_ref().map(|value| value.to_string_lossy()).as_deref() => Err(
            "This Codex thread was created under a different CODEX_HOME. Its history is readable, but it cannot be resumed with the current Codex account.".to_owned()
        ),
        _ => Ok(()),
    }
}

fn prompt_agent(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    prompt: String,
    execution_profile: ditch_core::AgentExecutionProfile,
) -> ServerResponse {
    let (project_root, thread_id, allow_non_git) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(record) = state.agents.get(&agent_id) else {
            return protocol_error("agent_not_found", "agent session was not found");
        };
        if state.children.contains_key(&agent_id)
            || matches!(
                record.run.state,
                AgentState::Starting | AgentState::Working | AgentState::Stopping
            )
        {
            return protocol_error("agent_busy", "agent session is already working");
        }
        if record.run.native_session_id.is_none()
            && matches!(
                record.run.state,
                AgentState::Failed | AgentState::Interrupted | AgentState::Stale
            )
        {
            return protocol_error(
                "agent_not_resumable",
                "This session cannot accept another prompt because Codex never created a thread. Start a new agent instead.",
            );
        }
        let current_home = state
            .codex_home
            .as_ref()
            .map(|value| value.to_string_lossy());
        if record.run.origin_codex_home.as_deref() != current_home.as_deref() {
            return protocol_error(
                "codex_home_mismatch",
                "This session belongs to a different CODEX_HOME. Start a new agent with the current Codex account.",
            );
        }
        (
            record.project_root.clone(),
            record.run.native_session_id.clone(),
            record.allow_non_git,
        )
    };

    let binary = active_codex_binary(&state);
    let Some(binary) = binary else {
        let message = "No working Codex CLI installation was found. Choose an existing installation in Ditch settings.".to_owned();
        record_terminal_failure(&state, agent_id, message.clone(), None);
        finish_agent(&state, agent_id, None, Some(1));
        return protocol_error("codex_not_found", message);
    };

    let child = match spawn_codex_child(
        &binary,
        &project_root,
        &prompt,
        &CodexLaunchMode::Exec,
        thread_id.as_deref(),
        allow_non_git,
        &execution_profile,
    ) {
        Ok(child) => Arc::new(Mutex::new(child)),
        Err(error) => {
            let message = error.to_string();
            record_terminal_failure(&state, agent_id, message.clone(), None);
            finish_agent(&state, agent_id, None, Some(1));
            return protocol_error("codex_start_failed", message);
        }
    };

    let user_message = AgentChatMessage {
        agent_id,
        role: AgentChatRole::User,
        text: prompt.clone(),
        created_at: Utc::now(),
    };
    let run_id = uuid::Uuid::new_v4();
    let process_group_id = child
        .lock()
        .expect("child lock should not be poisoned")
        .id() as i32;

    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(record) = state.agents.get_mut(&agent_id) else {
            return protocol_error("agent_not_found", "agent session was not found");
        };
        record.run.state = AgentState::Working;
        record.run.can_stop = true;
        record.run.execution_profile = execution_profile;
        record.run.current_prompt = Some(prompt);
        record.run.last_visible_action = Some("Prompt sent to Codex".to_owned());
        record.run.updated_at = Utc::now();
        record.run.finished_at = None;
        record.run.exit_code = None;
        record.run.resume_block_reason = None;
        record.terminal_failure = None;
        record.messages.push(user_message.clone());
        let run = record.run.clone();
        state.persist_message(&user_message);
        state.persist_agent(agent_id);
        state.children.insert(
            agent_id,
            ActiveAgentChild {
                run_id,
                process_group_id,
                child: Arc::clone(&child),
                app_server: false,
            },
        );
        state.broadcast(ServerEvent::AgentChanged(run));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), agent_id, run_id, child);
    ServerResponse::Accepted
}

fn stop_agent(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let active = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let active = state.children.get(&agent_id).cloned();
        if active.as_ref().is_some_and(|child| child.app_server) {
            state.app_server_turns.remove(&agent_id);
            state
                .app_server_permission_agents
                .retain(|_, owner| *owner != agent_id);
            state
                .pending_permissions
                .retain(|_, request| request.agent_id != Some(agent_id));
        }
        let Some(record) = state.agents.get_mut(&agent_id) else {
            return protocol_error("agent_not_found", "agent session was not found");
        };
        record.run.state = if active.is_some() {
            AgentState::Stopping
        } else {
            AgentState::Interrupted
        };
        record.run.can_stop = false;
        record.run.last_visible_action = Some(if active.is_some() {
            "Stopping Codex".to_owned()
        } else {
            "Stopped by user".to_owned()
        });
        record.run.updated_at = Utc::now();
        if active.is_none() {
            record.run.finished_at = Some(record.run.updated_at);
            record.run.resume_block_reason = record
                .run
                .native_session_id
                .is_none()
                .then_some(AgentResumeBlockReason::NoCodexThread);
        }
        let run = record.run.clone();
        state.persist_agent(agent_id);
        state.broadcast(ServerEvent::AgentChanged(run));
        active
    };

    let Some(active) = active else {
        return ServerResponse::Accepted;
    };

    signal_process(active.process_group_id, libc::SIGINT);
    let mut group_stopped = wait_for_process_group_exit(&active, STOP_INTERRUPT_GRACE);
    if !group_stopped {
        signal_process_group(active.process_group_id, libc::SIGTERM);
        group_stopped = wait_for_process_group_exit(&active, STOP_TERMINATE_GRACE);
    }
    if !group_stopped {
        signal_process_group(active.process_group_id, libc::SIGKILL);
        let _ = active
            .child
            .lock()
            .expect("child lock should not be poisoned")
            .kill();
        group_stopped = wait_for_process_group_exit(&active, STOP_KILL_GRACE);
    }

    if !group_stopped {
        return protocol_error(
            "agent_stop_failed",
            "Codex did not release its process group; this session is not safe to resume yet.",
        );
    }

    // The exit watcher drains stdout before finalizing, which preserves a
    // thread.started event that was already in flight when Stop was clicked.
    if !wait_for_run_finalization(&state, agent_id, active.run_id, Duration::from_secs(1)) {
        finalize_stopped_run(&state, agent_id, active.run_id);
    }
    ServerResponse::Accepted
}

fn force_kill_agent(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let active = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if !state.agents.contains_key(&agent_id) {
            return protocol_error("agent_not_found", "agent session was not found");
        }
        state.children.get(&agent_id).cloned()
    };
    let Some(active) = active else {
        return protocol_error("agent_not_running", "agent session is not running");
    };
    // The PID is never supplied remotely. It comes only from the Child owned by
    // this exact agent/run token and targets its dedicated process group.
    signal_process_group(active.process_group_id, libc::SIGKILL);
    let _ = active
        .child
        .lock()
        .expect("child lock should not be poisoned")
        .kill();
    if !wait_for_process_group_exit(&active, STOP_KILL_GRACE) {
        return protocol_error(
            "agent_stop_failed",
            "The owned Codex process group did not exit.",
        );
    }
    if !wait_for_run_finalization(&state, agent_id, active.run_id, Duration::from_secs(1)) {
        finalize_stopped_run(&state, agent_id, active.run_id);
    }
    ServerResponse::Accepted
}

fn signal_process_group(process_group_id: i32, signal: i32) {
    if process_group_id > 0 {
        // SAFETY: kill with a negative PID targets the process group created
        // for this Codex launch. No pointers or shared memory are involved.
        unsafe {
            libc::kill(-process_group_id, signal);
        }
    }
}

fn signal_process(process_id: i32, signal: i32) {
    if process_id > 0 {
        // SAFETY: the PID is read directly from the Child created for this run.
        unsafe {
            libc::kill(process_id, signal);
        }
    }
}

fn process_group_exists(process_group_id: i32) -> bool {
    if process_group_id <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs existence/permission checking only.
    let result = unsafe { libc::kill(-process_group_id, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn wait_for_process_group_exit(active: &ActiveAgentChild, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let _ = active
            .child
            .lock()
            .expect("child lock should not be poisoned")
            .try_wait();
        if !process_group_exists(active.process_group_id) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_run_finalization(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: uuid::Uuid,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let current = state
            .lock()
            .expect("runtime state lock should not be poisoned")
            .children
            .get(&agent_id)
            .is_some_and(|active| active.run_id == run_id);
        if !current {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn finalize_stopped_run(state: &Arc<Mutex<RuntimeState>>, agent_id: AgentId, run_id: uuid::Uuid) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if !is_current_run(&state, agent_id, run_id) {
        return;
    }
    state.children.remove(&agent_id);
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return;
    };
    record.run.state = AgentState::Interrupted;
    record.run.can_stop = false;
    record.run.last_visible_action = Some("Stopped by user".to_owned());
    record.run.updated_at = Utc::now();
    record.run.finished_at = Some(record.run.updated_at);
    record.run.resume_block_reason = record
        .run
        .native_session_id
        .is_none()
        .then_some(AgentResumeBlockReason::NoCodexThread);
    let run = record.run.clone();
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentChanged(run));
}

fn delete_agent(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(record) = state.agents.get(&agent_id) else {
        return protocol_error("agent_not_found", "agent session was not found");
    };
    if matches!(
        record.run.state,
        AgentState::Starting | AgentState::Working | AgentState::Stopping
    ) || state.children.contains_key(&agent_id)
    {
        return protocol_error(
            "agent_active",
            "Stop this agent before deleting it permanently.",
        );
    }
    if let Err(error) = state.store.delete_agent(agent_id) {
        return protocol_error("agent_delete_failed", error.to_string());
    }
    state.agents.remove(&agent_id);
    state
        .attention
        .retain(|item| item.agent_id != Some(agent_id));
    state.broadcast(ServerEvent::AgentDeleted { agent_id });
    ServerResponse::Accepted
}

fn delete_project(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ditch_core::ProjectId,
) -> ServerResponse {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(project_key) = state
        .projects
        .iter()
        .find_map(|(key, project)| (project.id == project_id).then(|| key.clone()))
    else {
        return protocol_error("project_not_found", "project was not found");
    };
    let has_active_agent = state.agents.values().any(|record| {
        record.run.project_id == project_id
            && (record.run.can_stop
                || state.children.contains_key(&record.run.id)
                || matches!(
                    record.run.state,
                    AgentState::Starting
                        | AgentState::Working
                        | AgentState::Stopping
                        | AgentState::AwaitingApproval
                        | AgentState::Blocked
                ))
    });
    if has_active_agent {
        return protocol_error(
            "project_active",
            "Stop this project's active agents before deleting it.",
        );
    }
    if let Err(error) = state.store.delete_project(project_id) {
        return protocol_error("project_delete_failed", error.to_string());
    }

    if let Some(terminal_id) = state.terminal_by_project.remove(&project_id)
        && let Some(mut terminal) = state.terminals.remove(&terminal_id)
    {
        let _ = terminal.child.kill();
    }
    let agent_ids = state
        .agents
        .values()
        .filter(|record| record.run.project_id == project_id)
        .map(|record| record.run.id)
        .collect::<Vec<_>>();
    for agent_id in &agent_ids {
        state.agents.remove(agent_id);
        state.children.remove(agent_id);
    }
    state.attention.retain(|item| {
        item.project_id != Some(project_id)
            && item
                .agent_id
                .is_none_or(|agent_id| !agent_ids.contains(&agent_id))
    });
    state.projects.remove(&project_key);
    state.broadcast(ServerEvent::ProjectDeleted { project_id });
    ServerResponse::Accepted
}

fn rename_agent(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    title: Option<String>,
) -> ServerResponse {
    let title = title
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return protocol_error("agent_not_found", "agent session was not found");
    };
    record.run.user_title = title;
    record.run.updated_at = Utc::now();
    let run = record.run.clone();
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentChanged(run));
    ServerResponse::Accepted
}

fn attach_codex_io(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: uuid::Uuid,
    child: Arc<Mutex<Child>>,
) {
    let stdout = child
        .lock()
        .expect("child lock should not be poisoned")
        .stdout
        .take();
    let stdout_thread = if let Some(stdout) = stdout {
        let state = Arc::clone(&state);
        Some(thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => handle_codex_stdout_line(&state, agent_id, Some(run_id), &line),
                    Err(error) => {
                        record_terminal_failure(
                            &state,
                            agent_id,
                            format!("Failed to read Codex output: {error}"),
                            Some(run_id),
                        );
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    let stderr = child
        .lock()
        .expect("child lock should not be poisoned")
        .stderr
        .take();
    let stderr_thread = if let Some(stderr) = stderr {
        let state = Arc::clone(&state);
        Some(thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let text = line.trim();
                        if !text.is_empty() {
                            log_codex_diagnostic(&state, agent_id, "stderr", text);
                            if is_terminal_codex_stderr(text) {
                                record_terminal_failure(
                                    &state,
                                    agent_id,
                                    text.to_owned(),
                                    Some(run_id),
                                );
                            }
                        }
                    }
                    Err(error) => {
                        record_terminal_failure(
                            &state,
                            agent_id,
                            format!("Failed to read Codex diagnostics: {error}"),
                            Some(run_id),
                        );
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    thread::spawn(move || {
        let code = loop {
            let status = child
                .lock()
                .expect("child lock should not be poisoned")
                .try_wait();
            match status {
                Ok(Some(status)) => break status.code(),
                Ok(None) => thread::sleep(std::time::Duration::from_millis(25)),
                Err(_) => break None,
            }
        };
        if let Some(reader) = stdout_thread {
            let _ = reader.join();
        }
        if let Some(reader) = stderr_thread {
            let _ = reader.join();
        }
        finish_agent(&state, agent_id, Some(run_id), code);
    });
}

fn handle_codex_stdout_line(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: Option<uuid::Uuid>,
    line: &str,
) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        let text = line.trim();
        if looks_like_diagnostic(text) {
            log_codex_diagnostic(state, agent_id, "stdout", text);
        }
        return;
    };
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
        return;
    };

    match event_type {
        "turn.started" => start_agent_turn(state, agent_id, run_id),
        "thread.started" => {
            if let Some(thread_id) = value.get("thread_id").and_then(Value::as_str) {
                let codex_title =
                    codex_session_title(&value).or_else(|| codex_title_from_state(thread_id));
                let mut state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                if run_id.is_some_and(|run_id| !is_current_run(&state, agent_id, run_id)) {
                    return;
                }
                let Some(record) = state.agents.get_mut(&agent_id) else {
                    return;
                };
                record.run.native_session_id = Some(thread_id.to_owned());
                if let Some(title) = codex_title {
                    record.run.codex_title = Some(title);
                }
                record.run.updated_at = Utc::now();
                record.run.resume_block_reason = None;
                let run = record.run.clone();
                state.persist_agent(agent_id);
                state.broadcast(ServerEvent::AgentChanged(run));
            }
        }
        "thread.updated" | "thread.renamed" | "session.updated" => {
            if let Some(title) = codex_session_title(&value) {
                let mut state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                if run_id.is_some_and(|run_id| !is_current_run(&state, agent_id, run_id)) {
                    return;
                }
                let Some(record) = state.agents.get_mut(&agent_id) else {
                    return;
                };
                record.run.codex_title = Some(title);
                record.run.updated_at = Utc::now();
                let run = record.run.clone();
                state.persist_agent(agent_id);
                state.broadcast(ServerEvent::AgentChanged(run));
            }
        }
        "item.started" => {
            let action = value
                .get("item")
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                .map(|kind| match kind {
                    "command_execution" => "Running a command",
                    "agent_message" => "Writing a response",
                    _ => "Processing",
                })
                .unwrap_or("Processing");
            update_agent_action(state, agent_id, action, run_id);
        }
        "item.completed" => {
            let Some(item) = value.get("item") else {
                return;
            };
            match item.get("type").and_then(Value::as_str) {
                Some("agent_message") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        append_message(
                            state,
                            agent_id,
                            AgentChatRole::Assistant,
                            text.trim().to_owned(),
                            run_id,
                        );
                    }
                }
                Some("command_execution") => {
                    if let Some(command) = item.get("command").and_then(Value::as_str) {
                        append_message(
                            state,
                            agent_id,
                            AgentChatRole::Tool,
                            command.trim().to_owned(),
                            run_id,
                        );
                    }
                }
                Some("error") => {
                    if let Some(message) = extract_diagnostic_message(&value) {
                        record_terminal_failure(state, agent_id, message, run_id);
                    }
                }
                _ => {}
            }
        }
        "turn.completed" => {
            let codex_title = {
                let state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                state
                    .agents
                    .get(&agent_id)
                    .and_then(|record| record.run.native_session_id.as_deref())
                    .and_then(codex_title_from_state)
            };
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if run_id.is_some_and(|run_id| !is_current_run(&state, agent_id, run_id)) {
                return;
            }
            let Some(record) = state.agents.get_mut(&agent_id) else {
                return;
            };
            if record.run.state == AgentState::Stopping {
                return;
            }
            record.run.state = AgentState::Completed;
            record.terminal_failure = None;
            record.run.last_visible_action = Some("Turn completed".to_owned());
            if let Some(title) = codex_title {
                record.run.codex_title = Some(title);
            }
            record.run.updated_at = Utc::now();
            let run = record.run.clone();
            state.persist_agent(agent_id);
            state.broadcast(ServerEvent::AgentChanged(run));
        }
        "turn.failed" | "error" => {
            let message = extract_diagnostic_message(&value)
                .unwrap_or_else(|| format!("Codex reported {event_type}"));
            if is_transient_reconnect(&message) {
                append_message(state, agent_id, AgentChatRole::System, message, run_id);
            } else {
                record_terminal_failure(state, agent_id, message, run_id);
            }
        }
        _ => {
            if let Some(message) = extract_explicit_error(&value) {
                record_terminal_failure(state, agent_id, message, run_id);
            }
        }
    }
}

fn codex_session_title(value: &Value) -> Option<String> {
    [
        value.get("title"),
        value.get("name"),
        value.pointer("/thread/title"),
        value.pointer("/thread/name"),
        value.pointer("/session/title"),
        value.pointer("/session/name"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|title| !title.is_empty())
    .map(str::to_owned)
}

fn codex_title_from_state(thread_id: &str) -> Option<String> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))?;
    let connection = rusqlite::Connection::open_with_flags(
        codex_home.join("state_5.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    connection
        .query_row(
            "SELECT COALESCE(NULLIF(name, ''), NULLIF(title, '')) FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
}

fn looks_like_diagnostic(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.starts_with("error")
        || lower.starts_with("warning")
        || lower.contains("failed")
        || lower.contains("usage limit")
}

fn is_terminal_codex_stderr(text: &str) -> bool {
    text.starts_with("Error:")
        || text.starts_with("Failed to create session:")
        || text.starts_with("failed to initialize thread persistence:")
}

fn is_transient_reconnect(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("reconnecting...") && lower.contains("stream disconnected before completion")
}

fn extract_diagnostic_message(value: &Value) -> Option<String> {
    let error = value
        .get("error")
        .or_else(|| value.pointer("/item/error"))
        .or_else(|| value.pointer("/payload/error"));
    let message = error
        .and_then(|error| match error {
            Value::String(message) => Some(message.clone()),
            Value::Object(_) => error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        })
        .or_else(|| {
            value
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            value
                .get("item")
                .and_then(|item| item.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| {
            value
                .get("payload")
                .and_then(|payload| payload.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let code = error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .or_else(|| value.get("code").and_then(Value::as_str));
    match (message, code) {
        (Some(message), Some(code)) if !message.contains(code) => {
            Some(format!("{message} (code: {code})"))
        }
        (message, _) => message,
    }
}

fn extract_explicit_error(value: &Value) -> Option<String> {
    if value.get("error").is_some()
        || value.pointer("/item/error").is_some()
        || value.pointer("/payload/error").is_some()
    {
        extract_diagnostic_message(value)
    } else {
        None
    }
}

fn is_current_run(state: &RuntimeState, agent_id: AgentId, run_id: uuid::Uuid) -> bool {
    state
        .children
        .get(&agent_id)
        .is_some_and(|active| active.run_id == run_id)
}

fn record_terminal_failure(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    message: String,
    run_id: Option<uuid::Uuid>,
) {
    let message = message.trim().to_owned();
    if message.is_empty() {
        return;
    }
    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if run_id.is_some_and(|run_id| !is_current_run(&state, agent_id, run_id)) {
            return;
        }
        let Some(record) = state.agents.get_mut(&agent_id) else {
            return;
        };
        if record.run.state == AgentState::Stopping {
            return;
        }
        record.terminal_failure = Some(message.clone());
        record.run.state = AgentState::Failed;
    }
    append_message(
        state,
        agent_id,
        AgentChatRole::System,
        message.clone(),
        run_id,
    );
    if is_workspace_permission_denial(&message) {
        raise_workspace_permission_attention(state, agent_id, message);
    }
}

fn is_workspace_permission_denial(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    (lower.contains("read-only sandbox")
        || lower.contains("read-only permission profile")
        || lower.contains("writing is blocked")
        || lower.contains("workspace write access"))
        && (lower.contains("reject") || lower.contains("block") || lower.contains("disabled"))
}

fn raise_workspace_permission_attention(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    body: String,
) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if state.attention.iter().any(|item| {
        item.agent_id == Some(agent_id) && item.title == "Codex cannot write this project"
    }) {
        return;
    }
    let project_id = state
        .agents
        .get(&agent_id)
        .map(|record| record.run.project_id);
    let project_name = project_id.and_then(|project_id| {
        state
            .projects
            .values()
            .find(|project| project.id == project_id)
            .map(|project| project.name.clone())
    });
    let agent_name = state
        .agents
        .get(&agent_id)
        .map(|record| agent_display_name(&record.run));
    let attention = RuntimeAttention {
        id: uuid::Uuid::new_v4(),
        kind: AttentionKind::Blocked,
        agent_id: Some(agent_id),
        project_id,
        project_name,
        agent_name,
        title: "Codex cannot write this project".to_owned(),
        body: notification_summary(&body),
        created_at: Utc::now(),
        read_at: None,
    };
    if let Err(error) = state.store.upsert_attention(&attention) {
        eprintln!("{RUNTIME_IDENTITY} failed to persist attention: {error}");
    }
    state.attention.push(attention.clone());
    state.broadcast(ServerEvent::AttentionRaised(attention));
}

fn update_agent_action(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    action: &str,
    run_id: Option<uuid::Uuid>,
) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if run_id.is_some_and(|run_id| !is_current_run(&state, agent_id, run_id)) {
        return;
    }
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return;
    };
    if record.run.state == AgentState::Stopping {
        return;
    }
    record.run.last_visible_action = Some(action.to_owned());
    record.run.updated_at = Utc::now();
    let run = record.run.clone();
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentChanged(run));
}

fn start_agent_turn(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: Option<uuid::Uuid>,
) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if run_id.is_some_and(|run_id| !is_current_run(&state, agent_id, run_id)) {
        return;
    }
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return;
    };
    if record.run.state == AgentState::Stopping {
        return;
    }
    record.terminal_failure = None;
    record.run.state = AgentState::Working;
    record.run.last_visible_action = Some("Codex is working".to_owned());
    record.run.updated_at = Utc::now();
    let run = record.run.clone();
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentChanged(run));
}

fn log_codex_diagnostic(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    stream: &str,
    text: &str,
) {
    let path = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .paths
        .logs_dir
        .join("runtime.log");
    let line = format!("{} Codex {} {stream}: {text}\n", Utc::now(), agent_id.0);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

fn append_message(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    role: AgentChatRole,
    text: String,
    run_id: Option<uuid::Uuid>,
) {
    if text.trim().is_empty() {
        return;
    }

    let message = AgentChatMessage {
        agent_id,
        role,
        text,
        created_at: Utc::now(),
    };

    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if run_id.is_some_and(|run_id| !is_current_run(&state, agent_id, run_id)) {
        return;
    }
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return;
    };
    record.messages.push(message.clone());
    record.run.updated_at = Utc::now();
    state.persist_message(&message);
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentMessageAppended(message));
}

fn finish_agent(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: Option<uuid::Uuid>,
    code: Option<i32>,
) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if let Some(run_id) = run_id
        && !is_current_run(&state, agent_id, run_id)
    {
        return;
    }
    let app_server = state
        .children
        .get(&agent_id)
        .is_some_and(|active| active.app_server);
    state.children.remove(&agent_id);
    if app_server {
        state.app_server_turns.remove(&agent_id);
        state
            .app_server_permission_agents
            .retain(|_, owner| *owner != agent_id);
        state
            .pending_permissions
            .retain(|_, request| request.agent_id != Some(agent_id));
    }
    let (run, attention) = {
        let has_blocked_attention = state.attention.iter().any(|attention| {
            attention.agent_id == Some(agent_id)
                && attention.kind == AttentionKind::Blocked
                && attention.title == "Codex cannot write this project"
        });
        let project_name = state
            .agents
            .get(&agent_id)
            .and_then(|record| {
                state
                    .projects
                    .values()
                    .find(|project| project.id == record.run.project_id)
            })
            .map(|project| project.name.clone());
        let Some(record) = state.agents.get_mut(&agent_id) else {
            return;
        };
        let stopping = record.run.state == AgentState::Stopping;
        let interrupted = record.run.state == AgentState::Interrupted;
        let app_server_finished = app_server
            && matches!(
                record.run.state,
                AgentState::Completed | AgentState::Failed | AgentState::Interrupted
            );
        record.run.can_stop = false;
        if stopping {
            record.run.state = AgentState::Interrupted;
            record.run.last_visible_action = Some("Stopped by user".to_owned());
        } else if !interrupted && !app_server_finished {
            record.run.state = if record.terminal_failure.is_some() {
                AgentState::Failed
            } else {
                match code {
                    Some(0) => AgentState::Completed,
                    _ => AgentState::Failed,
                }
            };
            record.run.last_visible_action = Some(match code {
                Some(code) => format!("Codex exited with code {code}"),
                None => "Codex exited without a code".to_owned(),
            });
        }
        record.run.updated_at = Utc::now();
        record.run.finished_at = Some(record.run.updated_at);
        if !app_server_finished {
            record.run.exit_code = code;
        }
        record.run.resume_block_reason = record
            .run
            .native_session_id
            .is_none()
            .then_some(AgentResumeBlockReason::NoCodexThread);
        let attention = match record.run.state {
            AgentState::Failed if has_blocked_attention => None,
            AgentState::Failed => {
                let body = record
                    .terminal_failure
                    .clone()
                    .or_else(|| {
                        record
                            .messages
                            .iter()
                            .rev()
                            .find(|message| message.role == AgentChatRole::System)
                            .map(|message| message.text.clone())
                    })
                    .unwrap_or_else(|| {
                        record
                            .run
                            .last_visible_action
                            .clone()
                            .unwrap_or_else(|| "The agent failed".to_owned())
                    });
                Some(RuntimeAttention {
                    id: uuid::Uuid::new_v4(),
                    kind: AttentionKind::Failed,
                    agent_id: Some(agent_id),
                    project_id: Some(record.run.project_id),
                    project_name: project_name.clone(),
                    agent_name: Some(agent_display_name(&record.run)),
                    title: "Agent failed".to_owned(),
                    body: notification_summary(&body),
                    created_at: Utc::now(),
                    read_at: None,
                })
            }
            AgentState::Completed => Some(RuntimeAttention {
                id: uuid::Uuid::new_v4(),
                kind: AttentionKind::Completed,
                agent_id: Some(agent_id),
                project_id: Some(record.run.project_id),
                project_name,
                agent_name: Some(agent_display_name(&record.run)),
                title: "Agent finished".to_owned(),
                body: "Task completed successfully.".to_owned(),
                created_at: Utc::now(),
                read_at: None,
            }),
            _ => None,
        };
        (record.run.clone(), attention)
    };
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentChanged(run));
    if let Some(attention) = attention {
        if let Err(error) = state.store.upsert_attention(&attention) {
            eprintln!("{RUNTIME_IDENTITY} failed to persist attention: {error}");
        }
        state.attention.push(attention.clone());
        state.broadcast(ServerEvent::AttentionRaised(attention));
    }
}

fn agent_display_name(run: &AgentRun) -> String {
    run.user_title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .or_else(|| {
            run.codex_title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
        })
        .unwrap_or("Codex")
        .trim()
        .to_owned()
}

fn notification_summary(value: &str) -> String {
    const MAX_CHARS: usize = 220;
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= MAX_CHARS {
        return compact;
    }
    let mut summary = compact.chars().take(MAX_CHARS - 1).collect::<String>();
    summary.push('…');
    summary
}

fn spawn_codex_child(
    binary: &str,
    cwd: &Path,
    prompt: &str,
    mode: &CodexLaunchMode,
    resume_thread: Option<&str>,
    allow_non_git: bool,
    execution_profile: &AgentExecutionProfile,
) -> io::Result<Child> {
    let mut command = Command::new(binary);
    // A dedicated process group lets Stop terminate Codex and every helper it
    // launches. Killing only the direct CLI process can orphan the app-server
    // process that owns the thread-store writer lock.
    command.process_group(0);
    command.args(codex_child_args(
        cwd,
        mode,
        resume_thread,
        allow_non_git,
        execution_profile,
    ));
    command.current_dir(cwd);
    command.env("TERM", "xterm-256color");
    if let Some(path) = effective_path_for_binary(Path::new(binary)) {
        command.env("PATH", path);
    }
    if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
        command.env("CODEX_HOME", codex_home);
    }
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
    }
    Ok(child)
}

fn codex_child_args(
    cwd: &Path,
    mode: &CodexLaunchMode,
    resume_thread: Option<&str>,
    allow_non_git: bool,
    execution_profile: &AgentExecutionProfile,
) -> Vec<String> {
    let mut args = Vec::new();
    match execution_profile.approval {
        AgentApprovalPreset::Ask => args.extend([
            "--sandbox".to_owned(),
            "workspace-write".to_owned(),
            "--ask-for-approval".to_owned(),
            "on-request".to_owned(),
        ]),
        AgentApprovalPreset::ApproveForMe => args.extend([
            "--sandbox".to_owned(),
            "workspace-write".to_owned(),
            "--ask-for-approval".to_owned(),
            "never".to_owned(),
        ]),
        AgentApprovalPreset::FullAccess => {
            args.push("--dangerously-bypass-approvals-and-sandbox".to_owned())
        }
    }
    if execution_profile.approval != AgentApprovalPreset::FullAccess {
        args.extend([
            "--config".to_owned(),
            "sandbox_workspace_write.network_access=true".to_owned(),
        ]);
    }
    args.extend([
        "--config".to_owned(),
        "web_search=\"live\"".to_owned(),
        "--config".to_owned(),
        "tools.web_search=true".to_owned(),
    ]);
    if let Some(model) = execution_profile.model.as_deref() {
        args.extend(["--model".to_owned(), model.to_owned()]);
    }
    if let Some(effort) = execution_profile.reasoning_effort.as_deref() {
        args.extend([
            "--config".to_owned(),
            format!("model_reasoning_effort=\"{effort}\""),
        ]);
    }
    if resume_thread.is_none() {
        args.extend(["--cd".to_owned(), cwd.to_string_lossy().into_owned()]);
    }
    args.push("exec".to_owned());
    if allow_non_git {
        args.push("--skip-git-repo-check".to_owned());
    }
    if let Some(thread_id) = resume_thread {
        args.extend([
            "resume".to_owned(),
            "--json".to_owned(),
            thread_id.to_owned(),
            "-".to_owned(),
        ]);
    } else {
        match mode {
            CodexLaunchMode::Exec | CodexLaunchMode::InteractiveTui => args.extend([
                "--json".to_owned(),
                "--color".to_owned(),
                "never".to_owned(),
                "-".to_owned(),
            ]),
        }
    }
    args
}

fn canonical_project_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

const MAX_EDITABLE_FILE_BYTES: u64 = 1024 * 1024;

#[allow(clippy::result_large_err)]
fn project_for_file_request(
    state: &Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
) -> Result<Project, ServerResponse> {
    state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .projects
        .values()
        .find(|project| project.id == project_id)
        .cloned()
        .ok_or_else(|| protocol_error("project_not_found", "project was not found"))
}

#[allow(clippy::result_large_err)]
fn resolve_project_path(project: &Project, relative_path: &str) -> Result<PathBuf, ServerResponse> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(protocol_error(
            "invalid_project_path",
            "path must stay inside the selected project",
        ));
    }
    let root = project
        .root
        .canonicalize()
        .map_err(|error| protocol_error("project_path_failed", format!("project root: {error}")))?;
    let candidate = root.join(relative);
    let resolved = candidate.canonicalize().map_err(|error| {
        protocol_error("project_path_failed", format!("{relative_path}: {error}"))
    })?;
    if !resolved.starts_with(&root) {
        return Err(protocol_error(
            "project_path_outside_root",
            "path resolves outside the selected project",
        ));
    }
    Ok(resolved)
}

fn list_project_directory(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    relative_path: &str,
) -> ServerResponse {
    let project = match project_for_file_request(&state, project_id) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let directory = match resolve_project_path(&project, relative_path) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => return protocol_error("not_a_directory", "path is not a directory"),
        Err(response) => return response,
    };
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => return protocol_error("directory_read_failed", error.to_string()),
    };
    let excluded = [
        ".git",
        ".ditch",
        ".dart_tool",
        "build",
        "target",
        "node_modules",
    ];
    let mut values = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if excluded.contains(&name.as_str()) {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let kind = if metadata.file_type().is_symlink() {
            ProjectFileKind::Symlink
        } else if metadata.is_dir() {
            ProjectFileKind::Directory
        } else if metadata.is_file() {
            ProjectFileKind::File
        } else {
            continue;
        };
        let child_path = if relative_path.is_empty() {
            name.clone()
        } else {
            format!("{relative_path}/{name}")
        };
        values.push(ProjectFileEntry {
            name,
            relative_path: child_path,
            kind,
            size: metadata.len(),
        });
    }
    values.sort_by(|left, right| {
        let left_rank = !matches!(left.kind, ProjectFileKind::Directory);
        let right_rank = !matches!(right.kind, ProjectFileKind::Directory);
        left_rank
            .cmp(&right_rank)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    ServerResponse::ProjectDirectory(ProjectDirectory {
        project_id,
        relative_path: relative_path.to_owned(),
        entries: values,
    })
}

fn read_project_file(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    relative_path: &str,
) -> ServerResponse {
    let project = match project_for_file_request(&state, project_id) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let path = match resolve_project_path(&project, relative_path) {
        Ok(path) if path.is_file() => path,
        Ok(_) => return protocol_error("not_a_file", "path is not a file"),
        Err(response) => return response,
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return protocol_error("file_read_failed", error.to_string()),
    };
    if bytes.len() as u64 > MAX_EDITABLE_FILE_BYTES {
        return protocol_error("file_too_large", "files larger than 1 MB are not editable");
    }
    let content = match String::from_utf8(bytes.clone()) {
        Ok(content) if !content.contains('\0') => content,
        _ => return protocol_error("binary_file", "only UTF-8 text files are editable"),
    };
    ServerResponse::ProjectFile(ProjectFile {
        project_id,
        relative_path: relative_path.to_owned(),
        revision: file_revision(&bytes),
        size: bytes.len() as u64,
        content,
    })
}

fn write_project_file(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    relative_path: &str,
    expected_revision: Option<&str>,
    content: &str,
) -> ServerResponse {
    if content.len() as u64 > MAX_EDITABLE_FILE_BYTES {
        return protocol_error("file_too_large", "files larger than 1 MB are not editable");
    }
    let project = match project_for_file_request(&state, project_id) {
        Ok(project) => project,
        Err(response) => return response,
    };
    let path = match resolve_project_path(&project, relative_path) {
        Ok(path) if path.is_file() => path,
        Ok(_) => return protocol_error("not_a_file", "path is not a file"),
        Err(response) => return response,
    };
    let original = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return protocol_error("file_read_failed", error.to_string()),
    };
    if expected_revision.is_some_and(|revision| file_revision(&original) != revision) {
        return protocol_error(
            "file_changed",
            "the file changed on disk; reload it before saving",
        );
    }
    let permissions = match fs::metadata(&path) {
        Ok(metadata) => metadata.permissions(),
        Err(error) => return protocol_error("file_write_failed", error.to_string()),
    };
    let Some(parent) = path.parent() else {
        return protocol_error("file_write_failed", "file has no parent directory");
    };
    let temporary = parent.join(format!(".ditch-save-{}.tmp", uuid::Uuid::new_v4()));
    let write_result = fs::write(&temporary, content.as_bytes())
        .and_then(|_| fs::set_permissions(&temporary, permissions))
        .and_then(|_| fs::rename(&temporary, &path));
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return protocol_error("file_write_failed", error.to_string());
    }
    ServerResponse::ProjectFileSaved(ProjectFileSaved {
        project_id,
        relative_path: relative_path.to_owned(),
        revision: file_revision(content.as_bytes()),
        size: content.len() as u64,
    })
}

fn file_revision(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn resolve_codex_binary(preferred: Option<&str>) -> Option<String> {
    discover_codex_installations(preferred)
        .into_iter()
        .find(|installation| installation.selected)
        .or_else(|| discover_codex_installations(None).into_iter().next())
        .map(|installation| installation.path)
}

fn check_codex_readiness(binary: Option<&str>) -> CodexReadiness {
    let Some(binary) = binary else {
        return CodexReadiness {
            path: None,
            version: None,
            compatible: false,
            authenticated: false,
            update_supported: false,
            doctor_supported: false,
            issues: vec![
                "Codex CLI was not found. Install Codex for this macOS user, then check again."
                    .to_owned(),
            ],
            diagnostics: None,
        };
    };
    let executable = Path::new(binary);
    let version = executable_version(executable);
    let path = effective_path_for_binary(executable);
    let root_help = command_text(
        executable,
        &["--help"],
        Duration::from_secs(5),
        path.clone(),
    );
    let exec_help = command_text(
        executable,
        &["exec", "--help"],
        Duration::from_secs(5),
        path.clone(),
    );
    let resume_help = command_text(
        executable,
        &["exec", "resume", "--help"],
        Duration::from_secs(5),
        path.clone(),
    );
    let update_supported = root_help
        .as_deref()
        .is_some_and(|help| help.contains("update"));
    let doctor_supported = root_help
        .as_deref()
        .is_some_and(|help| help.contains("doctor"));
    let mut issues = Vec::new();
    if version.is_none() {
        issues.push("The selected executable did not report a Codex version.".to_owned());
    }
    let required_global_flags = [
        "--ask-for-approval",
        "--sandbox",
        "--cd",
        "--model",
        "--config",
        "--dangerously-bypass-approvals-and-sandbox",
    ];
    match root_help.as_deref() {
        Some(help) => {
            let missing = required_global_flags
                .iter()
                .filter(|flag| !help.contains(**flag))
                .copied()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                issues.push(format!(
                    "This Codex CLI is missing required global options: {}.",
                    missing.join(", ")
                ));
            }
        }
        None => issues.push("The selected Codex CLI does not provide command help.".to_owned()),
    }
    let required_exec_flags = ["--json", "--color", "--skip-git-repo-check"];
    match exec_help.as_deref() {
        Some(help) => {
            let missing = required_exec_flags
                .iter()
                .filter(|flag| !help.contains(**flag))
                .copied()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                issues.push(format!(
                    "This Codex CLI is missing required exec options: {}.",
                    missing.join(", ")
                ));
            }
        }
        None => issues.push("The selected Codex CLI does not provide `codex exec`.".to_owned()),
    }
    if resume_help.is_none() {
        issues.push("The selected Codex CLI does not support resumable exec sessions.".to_owned());
    }
    let launch_probes: [(&str, &[&str]); 3] = [
        (
            "automatic workspace launch",
            &[
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "--config",
                "sandbox_workspace_write.network_access=true",
                "--config",
                "web_search=\"live\"",
                "--config",
                "tools.web_search=true",
                "exec",
                "--help",
            ],
        ),
        (
            "approval-based resumed launch",
            &[
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "exec",
                "resume",
                "--help",
            ],
        ),
        (
            "full-access launch",
            &[
                "--dangerously-bypass-approvals-and-sandbox",
                "exec",
                "--help",
            ],
        ),
    ];
    for (label, args) in launch_probes {
        if let Err(detail) = codex_argument_probe(executable, args, path.clone()) {
            issues.push(format!("Codex rejected Ditch's {label}: {detail}"));
        }
    }
    if !root_help
        .as_deref()
        .is_some_and(|help| help.contains("app-server"))
    {
        issues.push(
            "The selected Codex CLI cannot provide the model catalog required by Ditch.".to_owned(),
        );
    }

    let authenticated = command_output_with_timeout(
        executable,
        &["login", "status"],
        Duration::from_secs(10),
        path.clone(),
    )
    .is_some_and(|output| output.status.success());
    if !authenticated {
        issues.push("Codex is not signed in for this user.".to_owned());
    }

    let diagnostics = (doctor_supported && authenticated).then(|| {
        command_text(
            executable,
            &["doctor", "--json"],
            Duration::from_secs(30),
            path,
        )
        .unwrap_or_else(|| "Codex diagnostics could not be completed.".to_owned())
    });
    let compatible = version.is_some()
        && exec_help.is_some()
        && resume_help.is_some()
        && !issues.iter().any(|issue| !issue.contains("not signed in"));

    CodexReadiness {
        path: Some(binary.to_owned()),
        version,
        compatible,
        authenticated,
        update_supported,
        doctor_supported,
        issues,
        diagnostics,
    }
}

fn codex_argument_probe(
    executable: &Path,
    args: &[&str],
    path: Option<OsString>,
) -> Result<(), String> {
    let output = command_output_with_timeout(executable, args, Duration::from_secs(5), path)
        .ok_or_else(|| format!("`codex {}` did not complete", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`codex {}` failed: {}",
            args.join(" "),
            command_output_detail(&output)
        ))
    }
}

fn command_text(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
    path: Option<OsString>,
) -> Option<String> {
    let output = command_output_with_timeout(executable, args, timeout, path)?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    match (stdout.is_empty(), stderr.is_empty()) {
        (false, false) => Some(format!("{stdout}\n{stderr}")),
        (false, true) => Some(stdout),
        (true, false) => Some(stderr),
        (true, true) => Some(String::new()),
    }
}

fn command_output_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("Codex exited with {}", output.status)
    }
}

fn active_codex_binary(state: &Arc<Mutex<RuntimeState>>) -> Option<String> {
    let preferred = state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .codex_binary
        .clone();
    if let Some(path) = preferred.as_deref()
        && is_executable_file(Path::new(path))
        && executable_version(Path::new(path)).is_some()
    {
        return preferred;
    }

    let replacement = resolve_codex_binary(preferred.as_deref());
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state.codex_binary = replacement.clone();
    if let Some(path) = replacement.as_deref()
        && let Err(error) = state.store.set_setting(CODEX_BINARY_SETTING, path)
    {
        eprintln!("{RUNTIME_IDENTITY} failed to persist Codex selection: {error}");
    }
    replacement
}

fn discover_codex_installations(preferred: Option<&str>) -> Vec<CodexInstallation> {
    let mut candidates = Vec::new();
    if let Some(path) = preferred {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = login_shell_binary("codex") {
        candidates.push(path);
    }
    if let Some(path) = find_binary("codex") {
        candidates.push(PathBuf::from(path));
    }

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.extend([
            home.join(".volta/bin/codex"),
            home.join(".asdf/shims/codex"),
            home.join(".local/share/mise/shims/codex"),
            home.join(".local/bin/codex"),
            home.join(".npm-global/bin/codex"),
            home.join(".bun/bin/codex"),
            home.join(".nix-profile/bin/codex"),
            home.join(".yarn/bin/codex"),
            home.join("Library/pnpm/codex"),
            home.join(".local/share/pnpm/codex"),
        ]);
        collect_nested_binary(
            &home.join(".nvm/versions/node"),
            "bin/codex",
            &mut candidates,
        );
        collect_nested_binary(
            &home.join(".local/share/fnm/node-versions"),
            "installation/bin/codex",
            &mut candidates,
        );
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
        PathBuf::from("/opt/local/bin/codex"),
        PathBuf::from("/usr/bin/codex"),
    ]);

    let mut seen = BTreeSet::new();
    let mut installations = Vec::new();
    for candidate in candidates {
        if !is_executable_file(&candidate) {
            continue;
        }
        let identity = fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if !seen.insert(identity) {
            continue;
        }
        let Some(version) = executable_version(&candidate) else {
            continue;
        };
        let path = candidate.to_string_lossy().into_owned();
        installations.push(CodexInstallation {
            selected: preferred == Some(path.as_str()),
            path,
            version,
        });
    }
    installations
}

fn collect_nested_binary(root: &Path, suffix: &str, candidates: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        candidates.push(entry.path().join(suffix));
    }
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn executable_version(path: &Path) -> Option<String> {
    let output = command_output_with_timeout(
        path,
        &["--version"],
        Duration::from_secs(2),
        effective_path_for_binary(path),
    )?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!version.is_empty()).then_some(version)
}

fn login_shell_binary(name: &str) -> Option<PathBuf> {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| is_executable_file(path))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    let command = format!("command -v {name}");
    let output =
        command_output_with_timeout(&shell, &["-lic", &command], Duration::from_secs(3), None)?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('/'))
        .map(PathBuf::from)
        .find(|path| is_executable_file(path))
}

fn command_output_with_timeout(
    executable: &Path,
    args: &[&str],
    timeout: Duration,
    path: Option<OsString>,
) -> Option<std::process::Output> {
    let mut command = Command::new(executable);
    command.args(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stdout) = stdout {
            let _ = stdout.read_to_end(&mut bytes);
        }
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut bytes);
        }
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => return None,
        }
    };
    Some(std::process::Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}

fn effective_path_for_binary(binary: &Path) -> Option<OsString> {
    let mut paths = Vec::new();
    if let Some(parent) = binary.parent() {
        paths.push(parent.to_path_buf());
    }
    if let Some(login_path) = LOGIN_SHELL_PATH.get_or_init(login_shell_path).as_ref() {
        paths.extend(std::env::split_paths(login_path));
    }
    if let Some(current_path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&current_path));
    }
    let mut seen = BTreeSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    std::env::join_paths(paths).ok()
}

fn login_shell_path() -> Option<OsString> {
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| is_executable_file(path))
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
    let output = command_output_with_timeout(
        &shell,
        &["-lic", "printf '%s\\n' \"$PATH\""],
        Duration::from_secs(3),
        None,
    )?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.contains('/') && line.contains(':'))
        .filter(|line| !line.is_empty())
        .map(OsString::from)
}

fn find_binary(name: &str) -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let static_candidates = [
        home.as_ref()
            .map(|home| home.join(".nvm/current/bin").join(name)),
        home.as_ref()
            .map(|home| home.join(".npm-global/bin").join(name)),
        home.as_ref().map(|home| home.join(".local/bin").join(name)),
        Some(PathBuf::from("/opt/homebrew/bin").join(name)),
        Some(PathBuf::from("/usr/local/bin").join(name)),
        Some(PathBuf::from("/usr/bin").join(name)),
    ];

    if let Some(candidate) = static_candidates
        .into_iter()
        .flatten()
        .find(|candidate| candidate.is_file())
    {
        return Some(candidate.to_string_lossy().into_owned());
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|path| path.join(name))
        .find(|candidate| candidate.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

fn protocol_error(code: impl Into<String>, message: impl Into<String>) -> ServerResponse {
    ServerResponse::Error(ProtocolError {
        code: code.into(),
        message: message.into(),
    })
}

fn commercial_release_error(error: UpgradeError) -> ServerResponse {
    let code = match &error {
        UpgradeError::Network(_) => "commercial_release_network_failed",
        UpgradeError::Relay { code, .. } if code == "release_unavailable" => {
            "commercial_release_unavailable"
        }
        UpgradeError::Relay { code, .. } if code == "device_not_licensed" => {
            "commercial_device_not_licensed"
        }
        UpgradeError::Relay { .. } | UpgradeError::Rejected => "commercial_release_rejected",
        UpgradeError::Expired => "commercial_release_authorization_expired",
        UpgradeError::Downgrade => "commercial_release_downgrade_refused",
        UpgradeError::Incompatible => "commercial_release_incompatible",
        UpgradeError::ManifestSignature => "commercial_release_signature_invalid",
        UpgradeError::Digest | UpgradeError::Size => "commercial_release_artifact_invalid",
        UpgradeError::ApplicationIdentity => "commercial_release_identity_invalid",
        UpgradeError::InvalidResponse(message)
            if message.contains("not configured for official Commercial artifacts")
                || message.contains("Apple Team ID is not configured") =>
        {
            "commercial_release_verification_not_configured"
        }
        UpgradeError::InvalidResponse(_) => "commercial_release_invalid",
        UpgradeError::Io(_) => "commercial_release_staging_failed",
    };
    protocol_error(code, error.to_string())
}

trait ProjectRootKey {
    fn root_key(&self) -> String;
}

impl ProjectRootKey for Project {
    fn root_key(&self) -> String {
        match &self.execution_target {
            ProjectExecutionTarget::Local => self.root.to_string_lossy().into_owned(),
            ProjectExecutionTarget::Remote {
                remote_machine_id, ..
            } => {
                format!(
                    "remote://{remote_machine_id}{}",
                    self.root.to_string_lossy()
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_folder_browser_lists_directories_and_not_files() {
        let root = std::env::temp_dir().join(format!("ditch-browser-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("project")).unwrap();
        fs::write(root.join("notes.txt"), "not a project folder").unwrap();

        let response = list_filesystem_directory(&root.to_string_lossy());
        let ServerResponse::RemoteDirectory(directory) = response else {
            panic!("expected a directory response");
        };
        assert_eq!(
            directory
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["project"]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_terminal_events_are_forwarded_only_for_the_owning_host() {
        let mut runtime = test_runtime();
        let terminal_id = Uuid::new_v4();
        runtime
            .remote_terminal_hosts
            .insert(terminal_id, "dev-box".to_owned());
        let (tx, rx) = mpsc::channel();
        runtime.subscribers.push(tx);
        let state = Arc::new(Mutex::new(runtime));

        forward_remote_event(
            &state,
            "other-box",
            ServerEvent::ProjectTerminalOutput {
                terminal_id,
                data: b"wrong host".to_vec(),
            },
        );
        assert!(rx.try_recv().is_err());

        forward_remote_event(
            &state,
            "dev-box",
            ServerEvent::ProjectTerminalOutput {
                terminal_id,
                data: b"remote shell".to_vec(),
            },
        );
        let line = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(line.contains("ProjectTerminalOutput"));
        assert!(line.contains(&terminal_id.to_string()));
        assert!(line.contains("114,101,109,111,116,101,32,115,104,101,108,108"));
    }

    #[test]
    fn codex_readiness_checks_capabilities_and_authentication() {
        let root = std::env::temp_dir().join(format!("ditch-codex-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join("codex");
        fs::write(
            &binary,
            r#"#!/bin/sh
case "$1 $2 $3" in
  "--version  ") echo "codex-cli 1.2.3" ;;
  "--help  ") echo "exec app-server doctor update login --ask-for-approval --sandbox --cd --model --config --dangerously-bypass-approvals-and-sandbox" ;;
  "exec --help ") echo "--json --color --skip-git-repo-check" ;;
  "exec resume --help") echo "resume" ;;
  "--sandbox workspace-write --ask-for-approval") echo "accepted" ;;
  "--dangerously-bypass-approvals-and-sandbox exec --help") echo "accepted" ;;
  "login status ") echo "Logged in" ;;
  "doctor --json ") echo '{}' ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&binary, permissions).unwrap();

        let readiness = check_codex_readiness(Some(&binary.to_string_lossy()));

        assert!(readiness.compatible);
        assert!(readiness.authenticated);
        assert!(readiness.update_supported);
        assert!(readiness.doctor_supported);
        assert!(readiness.issues.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_readiness_rejects_a_genuinely_incompatible_cli() {
        let root = std::env::temp_dir().join(format!("ditch-codex-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join("codex");
        fs::write(
            &binary,
            r#"#!/bin/sh
case "$1 $2 $3" in
  "--version  ") echo "codex-cli 0.1.0" ;;
  "--help  ") echo "exec app-server login" ;;
  "exec --help ") echo "--json --color --skip-git-repo-check" ;;
  "exec resume --help") echo "resume" ;;
  "login status ") echo "Logged in" ;;
  *) echo "unexpected argument" >&2; exit 2 ;;
esac
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&binary, permissions).unwrap();

        let readiness = check_codex_readiness(Some(&binary.to_string_lossy()));

        assert!(!readiness.compatible);
        assert!(
            readiness
                .issues
                .iter()
                .any(|issue| issue.contains("missing required global options"))
        );
        assert!(
            readiness
                .issues
                .iter()
                .any(|issue| issue.contains("unexpected argument"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn test_runtime() -> RuntimeState {
        let root = std::env::temp_dir().join(format!("ditchd-test-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths {
            data_dir: root.clone(),
            database_path: root.join("ditch.sqlite3"),
            socket_path: root.join("ditchd.sock"),
            logs_dir: root.join("logs"),
            scrollback_dir: root.join("scrollback"),
        };
        ensure_app_dirs(&paths).expect("test app directories should be created");
        RuntimeState::new(paths, false).expect("test runtime should initialize")
    }

    #[test]
    fn local_runtime_rejects_the_internal_ssh_app_server_launch() {
        let state = Arc::new(Mutex::new(test_runtime()));
        let response = handle_request(
            ClientRequest::StartRemoteCodexAppServerSession {
                project_id: ProjectId::new(),
                project_name: "Remote".to_owned(),
                project_root: "/srv/remote".to_owned(),
                prompt: "test".to_owned(),
                execution_profile: AgentExecutionProfile::default(),
            },
            state,
        );
        let ServerResponse::Error(error) = response else {
            panic!("local runtime must reject the SSH-only launch");
        };
        assert_eq!(error.code, "ssh_app_server_only");
    }

    #[test]
    fn attention_subscribers_only_receive_attention_events() {
        let mut state = test_runtime();
        let (all_tx, all_rx) = mpsc::channel();
        let (attention_tx, attention_rx) = mpsc::channel();
        state.subscribers.push(all_tx);
        state.attention_subscribers.push(attention_tx);

        state.broadcast(ServerEvent::RuntimeStatusChanged(state.runtime_status()));
        assert!(
            all_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("general subscriber should receive status")
                .contains("RuntimeStatusChanged")
        );
        assert!(matches!(
            attention_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let attention_id = uuid::Uuid::new_v4();
        state.broadcast(ServerEvent::AttentionDismissed { attention_id });
        assert!(
            all_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("general subscriber should receive attention")
                .contains(&attention_id.to_string())
        );
        assert!(
            attention_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("attention subscriber should receive attention")
                .contains(&attention_id.to_string())
        );
    }

    #[test]
    fn attention_subscription_starts_with_a_compact_attention_snapshot() {
        let state = Arc::new(Mutex::new(test_runtime()));
        let (server, client) = UnixStream::pair().expect("fixture stream should open");
        let subscription_state = Arc::clone(&state);
        let subscription = thread::spawn(move || subscribe_attention(server, subscription_state));

        client
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .expect("fixture timeout should apply");
        let mut reader = BufReader::new(client);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("initial attention event should be readable");
        let event: Value = serde_json::from_str(&line).expect("event should be valid JSON");
        assert!(
            event
                .pointer("/body/event/AttentionSnapshotReplaced")
                .is_some()
        );
        assert!(!line.contains("\"projects\""));
        assert!(!line.contains("\"messages\""));
        assert!(line.len() < 512);

        state
            .lock()
            .expect("runtime state lock should not be poisoned")
            .attention_subscribers
            .clear();
        subscription
            .join()
            .expect("subscription thread should stop")
            .expect("subscription should close cleanly");
    }

    #[test]
    fn marking_attention_read_updates_status_and_persists() {
        let mut runtime = test_runtime();
        let attention = RuntimeAttention {
            id: uuid::Uuid::new_v4(),
            kind: AttentionKind::Completed,
            agent_id: None,
            project_id: None,
            project_name: None,
            agent_name: None,
            title: "Finished".to_owned(),
            body: "The agent completed".to_owned(),
            created_at: Utc::now(),
            read_at: None,
        };
        let other_attention = RuntimeAttention {
            id: uuid::Uuid::new_v4(),
            title: "Another result".to_owned(),
            ..attention.clone()
        };
        runtime.store.upsert_attention(&attention).unwrap();
        runtime.store.upsert_attention(&other_attention).unwrap();
        runtime.attention.push(attention.clone());
        runtime.attention.push(other_attention.clone());
        assert_eq!(runtime.runtime_status().unread_attention_count, 2);
        let state = Arc::new(Mutex::new(runtime));

        let response = handle_request(
            ClientRequest::MarkAttentionRead {
                attention_ids: vec![attention.id],
            },
            Arc::clone(&state),
        );

        assert!(matches!(response, ServerResponse::Accepted));
        {
            let state = state.lock().expect("fixture state should lock");
            assert_eq!(state.runtime_status().unread_attention_count, 1);
            assert!(state.attention[0].read_at.is_some());
            assert!(state.attention[1].read_at.is_none());
            let persisted = state.store.load().expect("persisted state should load");
            assert!(
                persisted
                    .attention
                    .iter()
                    .find(|item| item.id == attention.id)
                    .unwrap()
                    .read_at
                    .is_some()
            );
            assert!(
                persisted
                    .attention
                    .iter()
                    .find(|item| item.id == other_attention.id)
                    .unwrap()
                    .read_at
                    .is_none()
            );
        }

        let response = handle_request(ClientRequest::MarkAllAttentionRead, Arc::clone(&state));
        assert!(matches!(response, ServerResponse::Accepted));
        assert_eq!(
            state
                .lock()
                .expect("fixture state should lock")
                .runtime_status()
                .unread_attention_count,
            0
        );
    }

    #[test]
    fn live_runtime_socket_is_never_removed() {
        let path = std::env::temp_dir().join(format!(
            "ditchd-live-{}-{}.sock",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let listener = UnixListener::bind(&path).expect("fixture socket should bind");

        let error = remove_stale_socket(&path).expect_err("live socket must be preserved");

        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        assert!(path.exists());
        drop(listener);
        fs::remove_file(path).expect("fixture socket should be removed");
    }

    #[test]
    fn new_codex_session_disables_color_output() {
        let args = codex_child_args(
            Path::new("/tmp/project"),
            &CodexLaunchMode::Exec,
            None,
            false,
            &AgentExecutionProfile::default(),
        );

        assert_eq!(
            args,
            vec![
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "--config",
                "sandbox_workspace_write.network_access=true",
                "--config",
                "web_search=\"live\"",
                "--config",
                "tools.web_search=true",
                "--cd",
                "/tmp/project",
                "exec",
                "--json",
                "--color",
                "never",
                "-"
            ]
        );
    }

    #[test]
    fn resumed_codex_session_does_not_pass_color_flag() {
        let args = codex_child_args(
            Path::new("/tmp/project"),
            &CodexLaunchMode::Exec,
            Some("thread-123"),
            false,
            &AgentExecutionProfile::default(),
        );

        assert_eq!(
            args,
            vec![
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "never",
                "--config",
                "sandbox_workspace_write.network_access=true",
                "--config",
                "web_search=\"live\"",
                "--config",
                "tools.web_search=true",
                "exec",
                "resume",
                "--json",
                "thread-123",
                "-"
            ]
        );
        assert!(!args.iter().any(|arg| arg == "--color"));
    }

    #[test]
    fn explicit_non_git_policy_adds_skip_check_to_new_and_resumed_sessions() {
        let fresh = codex_child_args(
            Path::new("/tmp/project"),
            &CodexLaunchMode::Exec,
            None,
            true,
            &AgentExecutionProfile::default(),
        );
        let resumed = codex_child_args(
            Path::new("/tmp/project"),
            &CodexLaunchMode::Exec,
            Some("thread-123"),
            true,
            &AgentExecutionProfile::default(),
        );

        assert_eq!(
            fresh[fresh.iter().position(|arg| arg == "exec").unwrap() + 1],
            "--skip-git-repo-check"
        );
        assert_eq!(
            resumed[resumed.iter().position(|arg| arg == "exec").unwrap() + 1],
            "--skip-git-repo-check"
        );
    }

    #[test]
    fn codex_execution_profile_maps_approval_and_model_flags() {
        let ask = AgentExecutionProfile {
            model: Some("gpt-test".to_owned()),
            reasoning_effort: Some("high".to_owned()),
            approval: AgentApprovalPreset::Ask,
        };
        let ask_args = codex_child_args(
            Path::new("/tmp/project"),
            &CodexLaunchMode::Exec,
            None,
            false,
            &ask,
        );
        assert!(
            ask_args
                .windows(2)
                .any(|args| args == ["--model", "gpt-test"])
        );
        assert!(
            ask_args
                .windows(2)
                .any(|args| args == ["--sandbox", "workspace-write"])
        );
        assert!(
            ask_args
                .windows(2)
                .any(|args| args == ["--ask-for-approval", "on-request"])
        );
        assert!(
            ask_args.windows(2).any(|args| {
                args == ["--config", "sandbox_workspace_write.network_access=true"]
            })
        );
        for setting in ["web_search=\"live\"", "tools.web_search=true"] {
            assert!(
                ask_args
                    .windows(2)
                    .any(|args| args == ["--config", setting])
            );
        }
        let ask_exec = ask_args.iter().position(|arg| arg == "exec").unwrap();
        for global in ["--ask-for-approval", "--sandbox", "--model", "--config"] {
            assert!(ask_args.iter().position(|arg| arg == global).unwrap() < ask_exec);
        }

        let full_access = AgentExecutionProfile {
            approval: AgentApprovalPreset::FullAccess,
            ..Default::default()
        };
        let full_access_args = codex_child_args(
            Path::new("/tmp/project"),
            &CodexLaunchMode::Exec,
            Some("thread-123"),
            false,
            &full_access,
        );
        assert!(
            full_access_args
                .iter()
                .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
        );
        for setting in ["web_search=\"live\"", "tools.web_search=true"] {
            assert!(
                full_access_args
                    .windows(2)
                    .any(|args| args == ["--config", setting])
            );
        }
        assert!(
            full_access_args
                .iter()
                .position(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
                .unwrap()
                < full_access_args
                    .iter()
                    .position(|arg| arg == "exec")
                    .unwrap()
        );
        assert!(!full_access_args.iter().any(|arg| arg == "--approve-for-me"));
    }

    #[test]
    fn workspace_permission_denials_are_recognized() {
        assert!(is_workspace_permission_denial(
            "patch rejected: writing is blocked by read-only sandbox; rejected by user approval settings"
        ));
        assert!(!is_workspace_permission_denial("Codex exited with code 1"));
    }

    #[test]
    fn reconnect_retries_are_not_terminal_failures() {
        assert!(is_transient_reconnect(
            "Reconnecting... 2/5 (stream disconnected before completion: Broken pipe)"
        ));
        assert!(!is_transient_reconnect("stream disconnected permanently"));
    }

    #[test]
    fn active_writer_stderr_is_promoted_to_a_visible_terminal_failure() {
        assert!(is_terminal_codex_stderr(
            "Error: thread/resume: thread 123 already has an active writer"
        ));
        assert!(!is_terminal_codex_stderr(
            "2026-08-20 ERROR codex_core::tools::router: apply_patch failed"
        ));
    }

    #[test]
    fn stopping_run_still_accepts_an_in_flight_thread_id() {
        let mut runtime = test_runtime();
        let project = Project::new("Fixture", "/tmp/fixture-stopping-thread");
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let now = Utc::now();
        runtime.agents.insert(
            agent_id,
            AgentRecord {
                run: AgentRun {
                    id: agent_id,
                    provider: AgentProvider::Codex,
                    state: AgentState::Stopping,
                    can_stop: false,
                    launch_mode: CodexLaunchMode::Exec,
                    execution_profile: AgentExecutionProfile::default(),
                    project_id: project.id,
                    task_id: None,
                    pane_id: None,
                    native_session_id: None,
                    codex_title: None,
                    user_title: None,
                    origin_codex_home: None,
                    current_prompt: Some("start".to_owned()),
                    last_visible_action: Some("Stopping Codex".to_owned()),
                    state_confidence: 1.0,
                    state_evidence: "fixture".to_owned(),
                    started_at: now,
                    updated_at: now,
                    finished_at: None,
                    exit_code: None,
                    resume_block_reason: None,
                },
                project_root: project.root,
                allow_non_git: false,
                messages: Vec::new(),
                terminal_failure: None,
            },
        );
        let mut command = Command::new("sleep");
        command.arg("30");
        command.process_group(0);
        let child = Arc::new(Mutex::new(command.spawn().unwrap()));
        let process_group_id = child.lock().unwrap().id() as i32;
        let run_id = uuid::Uuid::new_v4();
        runtime.children.insert(
            agent_id,
            ActiveAgentChild {
                run_id,
                process_group_id,
                child: Arc::clone(&child),
                app_server: false,
            },
        );
        let state = Arc::new(Mutex::new(runtime));

        handle_codex_stdout_line(
            &state,
            agent_id,
            Some(run_id),
            r#"{"type":"thread.started","thread_id":"thread-arrived-during-stop"}"#,
        );

        let state_guard = state.lock().unwrap();
        assert_eq!(
            state_guard.agents[&agent_id]
                .run
                .native_session_id
                .as_deref(),
            Some("thread-arrived-during-stop")
        );
        assert_eq!(
            state_guard.agents[&agent_id].run.state,
            AgentState::Stopping
        );
        drop(state_guard);
        signal_process_group(process_group_id, libc::SIGKILL);
        let _ = child.lock().unwrap().wait();
    }

    #[test]
    fn stop_agent_waits_until_the_codex_process_group_exits() {
        let mut runtime = test_runtime();
        let project = Project::new("Fixture", "/tmp/fixture-stop");
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let now = Utc::now();
        runtime.agents.insert(
            agent_id,
            AgentRecord {
                run: AgentRun {
                    id: agent_id,
                    provider: AgentProvider::Codex,
                    state: AgentState::Working,
                    can_stop: false,
                    launch_mode: CodexLaunchMode::Exec,
                    execution_profile: AgentExecutionProfile::default(),
                    project_id: project.id,
                    task_id: None,
                    pane_id: None,
                    native_session_id: Some("thread-stop".to_owned()),
                    codex_title: Some("Stop test".to_owned()),
                    user_title: None,
                    origin_codex_home: None,
                    current_prompt: Some("keep working".to_owned()),
                    last_visible_action: None,
                    state_confidence: 1.0,
                    state_evidence: "fixture".to_owned(),
                    started_at: now,
                    updated_at: now,
                    finished_at: None,
                    exit_code: None,
                    resume_block_reason: None,
                },
                project_root: project.root,
                allow_non_git: false,
                messages: Vec::new(),
                terminal_failure: None,
            },
        );
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 & wait"]);
        command.process_group(0);
        let child = Arc::new(Mutex::new(
            command.spawn().expect("fixture child should start"),
        ));
        let process_group_id = child.lock().unwrap().id() as i32;
        let run_id = uuid::Uuid::new_v4();
        runtime.agents.get_mut(&agent_id).unwrap().run.can_stop = true;
        runtime.children.insert(
            agent_id,
            ActiveAgentChild {
                run_id,
                process_group_id,
                child: Arc::clone(&child),
                app_server: false,
            },
        );
        let state = Arc::new(Mutex::new(runtime));
        attach_codex_io(Arc::clone(&state), agent_id, run_id, child);

        let started = std::time::Instant::now();
        let response = stop_agent(Arc::clone(&state), agent_id);

        assert!(matches!(response, ServerResponse::Accepted));
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        let state = state.lock().expect("fixture state should lock");
        assert_eq!(state.agents[&agent_id].run.state, AgentState::Interrupted);
        assert_eq!(
            state.agents[&agent_id].run.last_visible_action.as_deref(),
            Some("Stopped by user")
        );
        assert!(!state.children.contains_key(&agent_id));
        assert!(state.attention.is_empty());
        assert!(!process_group_exists(process_group_id));
    }

    #[test]
    fn delete_project_removes_runtime_state_but_preserves_user_files() {
        let mut runtime = test_runtime();
        let project_root =
            std::env::temp_dir().join(format!("ditchd-delete-project-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&project_root).unwrap();
        fs::write(project_root.join("keep-me.txt"), "user data").unwrap();
        let project = Project::new("Fixture", &project_root);
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let state = Arc::new(Mutex::new(runtime));

        let response = delete_project(Arc::clone(&state), project.id);

        assert!(matches!(response, ServerResponse::Accepted));
        let state = state.lock().unwrap();
        assert!(state.projects.is_empty());
        assert!(state.store.load().unwrap().projects.is_empty());
        assert_eq!(
            fs::read_to_string(project_root.join("keep-me.txt")).unwrap(),
            "user data"
        );
        drop(state);
        fs::remove_dir_all(project_root).unwrap();
    }

    #[test]
    fn failed_agent_creates_attention_with_stderr_detail() {
        let mut runtime = test_runtime();
        let project = Project::new("Fixture", "/tmp/fixture");
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let now = Utc::now();
        runtime.agents.insert(
            agent_id,
            AgentRecord {
                run: AgentRun {
                    id: agent_id,
                    provider: AgentProvider::Codex,
                    state: AgentState::Working,
                    can_stop: false,
                    launch_mode: CodexLaunchMode::Exec,
                    execution_profile: AgentExecutionProfile::default(),
                    project_id: project.id,
                    task_id: None,
                    pane_id: None,
                    native_session_id: None,
                    codex_title: None,
                    user_title: None,
                    origin_codex_home: None,
                    current_prompt: Some("test".to_owned()),
                    last_visible_action: None,
                    state_confidence: 1.0,
                    state_evidence: "fixture".to_owned(),
                    started_at: now,
                    updated_at: now,
                    finished_at: None,
                    exit_code: None,
                    resume_block_reason: None,
                },
                project_root: project.root,
                allow_non_git: false,
                messages: vec![AgentChatMessage {
                    agent_id,
                    role: AgentChatRole::System,
                    text: "fixture failure".to_owned(),
                    created_at: now,
                }],
                terminal_failure: None,
            },
        );
        let state = Arc::new(Mutex::new(runtime));

        finish_agent(&state, agent_id, None, Some(1));

        let response = prompt_agent(
            Arc::clone(&state),
            agent_id,
            "retry".to_owned(),
            AgentExecutionProfile::default(),
        );
        assert!(matches!(
            response,
            ServerResponse::Error(ProtocolError { ref code, .. }) if code == "agent_not_resumable"
        ));

        let state = state.lock().expect("fixture state should lock");
        assert_eq!(state.agents[&agent_id].run.state, AgentState::Failed);
        assert_eq!(state.agents[&agent_id].run.exit_code, Some(1));
        assert!(state.agents[&agent_id].run.finished_at.is_some());
        assert_eq!(
            state.agents[&agent_id].run.resume_block_reason,
            Some(AgentResumeBlockReason::NoCodexThread)
        );
        assert_eq!(state.agents.len(), 1);
        assert_eq!(state.attention.len(), 1);
        assert_eq!(state.attention[0].body, "fixture failure");
        assert_eq!(state.attention[0].project_id, Some(project.id));
        let persisted = state.store.load().expect("persisted state should load");
        assert_eq!(persisted.agents.len(), 1);
        assert_eq!(persisted.agents[0].run.exit_code, Some(1));
        assert_eq!(
            persisted.agents[0].run.resume_block_reason,
            Some(AgentResumeBlockReason::NoCodexThread)
        );
    }

    #[test]
    fn completed_agent_creates_project_scoped_attention() {
        let mut runtime = test_runtime();
        let project = Project::new("Fixture", "/tmp/fixture-completed");
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let now = Utc::now();
        runtime.agents.insert(
            agent_id,
            AgentRecord {
                run: AgentRun {
                    id: agent_id,
                    provider: AgentProvider::Codex,
                    state: AgentState::Working,
                    can_stop: false,
                    launch_mode: CodexLaunchMode::Exec,
                    execution_profile: AgentExecutionProfile::default(),
                    project_id: project.id,
                    task_id: None,
                    pane_id: None,
                    native_session_id: Some("thread-completed".to_owned()),
                    codex_title: Some("Notification test".to_owned()),
                    user_title: None,
                    origin_codex_home: None,
                    current_prompt: Some("finish soon".to_owned()),
                    last_visible_action: None,
                    state_confidence: 1.0,
                    state_evidence: "fixture".to_owned(),
                    started_at: now,
                    updated_at: now,
                    finished_at: None,
                    exit_code: None,
                    resume_block_reason: None,
                },
                project_root: project.root,
                allow_non_git: false,
                messages: Vec::new(),
                terminal_failure: None,
            },
        );
        let state = Arc::new(Mutex::new(runtime));

        finish_agent(&state, agent_id, None, Some(0));

        let state = state.lock().expect("fixture state should lock");
        assert_eq!(state.agents[&agent_id].run.state, AgentState::Completed);
        assert_eq!(state.attention.len(), 1);
        assert_eq!(state.attention[0].kind, AttentionKind::Completed);
        assert_eq!(state.attention[0].agent_id, Some(agent_id));
        assert_eq!(state.attention[0].project_id, Some(project.id));
        assert_eq!(state.attention[0].project_name.as_deref(), Some("Fixture"));
        assert_eq!(
            state.attention[0].agent_name.as_deref(),
            Some("Notification test")
        );
        let persisted = state.store.load().expect("persisted state should load");
        assert_eq!(persisted.attention.len(), 1);
        assert_eq!(persisted.attention[0].kind, AttentionKind::Completed);
    }

    #[test]
    fn assistant_and_tool_text_cannot_create_false_permission_failures() {
        let mut runtime = test_runtime();
        let project = Project::new("Safe project", "/tmp/safe-content");
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let now = Utc::now();
        runtime.agents.insert(
            agent_id,
            AgentRecord {
                run: AgentRun {
                    id: agent_id,
                    provider: AgentProvider::Codex,
                    state: AgentState::Working,
                    can_stop: false,
                    launch_mode: CodexLaunchMode::Exec,
                    execution_profile: AgentExecutionProfile::default(),
                    project_id: project.id,
                    task_id: None,
                    pane_id: None,
                    native_session_id: Some("thread-safe".to_owned()),
                    codex_title: Some("Safety test".to_owned()),
                    user_title: None,
                    origin_codex_home: None,
                    current_prompt: Some("explain the detector".to_owned()),
                    last_visible_action: None,
                    state_confidence: 1.0,
                    state_evidence: "fixture".to_owned(),
                    started_at: now,
                    updated_at: now,
                    finished_at: None,
                    exit_code: None,
                    resume_block_reason: None,
                },
                project_root: project.root,
                allow_non_git: false,
                messages: Vec::new(),
                terminal_failure: None,
            },
        );
        let state = Arc::new(Mutex::new(runtime));

        handle_codex_stdout_line(
            &state,
            agent_id,
            None,
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"patch source containing writing is blocked by a read-only sandbox and block"}}"#,
        );
        handle_codex_stdout_line(
            &state,
            agent_id,
            None,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"This answer discusses workspace_permission_denial and writing is blocked, but reports no failure."}}"#,
        );
        handle_codex_stdout_line(&state, agent_id, None, r#"{"type":"turn.completed"}"#);
        finish_agent(&state, agent_id, None, Some(0));

        let state = state.lock().expect("fixture state should lock");
        assert_eq!(state.agents[&agent_id].run.state, AgentState::Completed);
        assert!(state.agents[&agent_id].terminal_failure.is_none());
        assert_eq!(state.attention.len(), 1);
        assert_eq!(state.attention[0].kind, AttentionKind::Completed);
        assert_eq!(
            state
                .agents
                .get(&agent_id)
                .unwrap()
                .messages
                .iter()
                .filter(|message| message.role == AgentChatRole::System)
                .count(),
            0
        );
    }

    #[test]
    fn structured_turn_failure_is_visible_in_chat_and_attention() {
        let mut runtime = test_runtime();
        let project = Project::new("Fixture", "/tmp/fixture");
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let now = Utc::now();
        runtime.agents.insert(
            agent_id,
            AgentRecord {
                run: AgentRun {
                    id: agent_id,
                    provider: AgentProvider::Codex,
                    state: AgentState::Working,
                    can_stop: false,
                    launch_mode: CodexLaunchMode::Exec,
                    execution_profile: AgentExecutionProfile::default(),
                    project_id: project.id,
                    task_id: None,
                    pane_id: None,
                    native_session_id: Some("thread-quota".to_owned()),
                    codex_title: None,
                    user_title: None,
                    origin_codex_home: None,
                    current_prompt: Some("test".to_owned()),
                    last_visible_action: None,
                    state_confidence: 1.0,
                    state_evidence: "fixture".to_owned(),
                    started_at: now,
                    updated_at: now,
                    finished_at: None,
                    exit_code: None,
                    resume_block_reason: None,
                },
                project_root: project.root,
                allow_non_git: false,
                messages: Vec::new(),
                terminal_failure: None,
            },
        );
        let state = Arc::new(Mutex::new(runtime));

        handle_codex_stdout_line(
            &state,
            agent_id,
            None,
            r#"{"type":"task_complete","payload":{"error":{"message":"You've hit your usage limit.","code":"usage_limit_exceeded"}}}"#,
        );
        finish_agent(&state, agent_id, None, Some(1));

        let state = state.lock().expect("fixture state should lock");
        assert_eq!(state.agents[&agent_id].messages.len(), 1);
        assert_eq!(
            state.agents[&agent_id].messages[0].text,
            "You've hit your usage limit. (code: usage_limit_exceeded)"
        );
        assert_eq!(
            state.attention[0].body,
            "You've hit your usage limit. (code: usage_limit_exceeded)"
        );
    }

    #[test]
    fn codex_thread_cannot_resume_in_another_project() {
        let mut runtime = test_runtime();
        let project = Project::new("Original", "/tmp/original");
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let now = Utc::now();
        runtime.agents.insert(
            agent_id,
            AgentRecord {
                run: AgentRun {
                    id: agent_id,
                    provider: AgentProvider::Codex,
                    state: AgentState::Failed,
                    can_stop: false,
                    launch_mode: CodexLaunchMode::Exec,
                    execution_profile: AgentExecutionProfile::default(),
                    project_id: project.id,
                    task_id: None,
                    pane_id: None,
                    native_session_id: Some("thread-123".to_owned()),
                    codex_title: None,
                    user_title: None,
                    origin_codex_home: None,
                    current_prompt: None,
                    last_visible_action: None,
                    state_confidence: 1.0,
                    state_evidence: "fixture".to_owned(),
                    started_at: now,
                    updated_at: now,
                    finished_at: None,
                    exit_code: None,
                    resume_block_reason: None,
                },
                project_root: project.root,
                allow_non_git: false,
                messages: Vec::new(),
                terminal_failure: None,
            },
        );
        let state = Arc::new(Mutex::new(runtime));

        let error = validate_thread_project(&state, "thread-123", Path::new("/tmp/other"))
            .expect_err("cross-project resume must fail");

        assert!(error.contains("belongs to /tmp/original, not /tmp/other"));
    }

    #[test]
    fn project_files_are_scoped_readable_and_revision_safe() {
        let mut runtime = test_runtime();
        let root = runtime.paths.data_dir.join("editable-project");
        fs::create_dir_all(root.join("lib")).expect("fixture directory should exist");
        fs::write(root.join("README.md"), "before\n").expect("fixture file should exist");
        fs::write(root.join("lib/main.dart"), "void main() {}\n")
            .expect("nested fixture should exist");
        fs::create_dir_all(root.join(".git")).expect("excluded fixture should exist");
        let project = Project::new("Editable", &root);
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let state = Arc::new(Mutex::new(runtime));

        let ServerResponse::ProjectDirectory(directory) =
            list_project_directory(Arc::clone(&state), project.id, "")
        else {
            panic!("root directory should list");
        };
        assert_eq!(
            directory
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["lib", "README.md"]
        );

        let ServerResponse::ProjectFile(file) =
            read_project_file(Arc::clone(&state), project.id, "README.md")
        else {
            panic!("text file should open");
        };
        assert_eq!(file.content, "before\n");
        let stale_revision = file.revision.clone();
        let ServerResponse::ProjectFileSaved(saved) = write_project_file(
            Arc::clone(&state),
            project.id,
            "README.md",
            Some(&file.revision),
            "after\n",
        ) else {
            panic!("matching revision should save");
        };
        assert_ne!(saved.revision, stale_revision);
        assert_eq!(
            fs::read_to_string(root.join("README.md")).unwrap(),
            "after\n"
        );

        assert!(matches!(
            write_project_file(
                Arc::clone(&state),
                project.id,
                "README.md",
                Some(&stale_revision),
                "overwrite\n",
            ),
            ServerResponse::Error(ProtocolError { ref code, .. }) if code == "file_changed"
        ));
        assert!(matches!(
            read_project_file(state, project.id, "../outside.txt"),
            ServerResponse::Error(ProtocolError { ref code, .. }) if code == "invalid_project_path"
        ));
    }

    #[test]
    fn commercial_upgrade_is_deferred_without_stopping_active_agents() {
        assert!(commercial_upgrade_deferred(0).is_none());
        assert!(matches!(
            commercial_upgrade_deferred(2),
            Some(ServerResponse::Error(ProtocolError { ref code, ref message }))
                if code == "commercial_upgrade_deferred"
                    && message.contains("2 active agents")
                    && message.contains("not interrupt")
        ));
    }
}
