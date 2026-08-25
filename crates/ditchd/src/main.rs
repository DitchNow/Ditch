use chrono::{DateTime, Utc};
use ditch_core::{
    AgentApprovalPreset, AgentExecutionProfile, AgentId, AgentProvider, AgentResumeBlockReason,
    AgentRun, AgentState, AppPaths, AttentionKind, ChangeIntent, CodexLaunchMode, IntegrationState,
    ManagedWorktree, Project, ProjectGitPolicy, ProjectId, WorktreeStatus,
};
use ditch_protocol::{
    AgentChatMessage, AgentChatRole, AgentModel, ClientRequest, CodexInstallation, CodexReadiness,
    Envelope, HealthResponse, InitialProjectSnapshotCreated, ProjectAgentReadiness,
    ProjectAgentReadinessState, ProjectDirectory, ProjectFile, ProjectFileEntry, ProjectFileKind,
    ProjectFileSaved, ProjectTerminal, ProtocolError, RuntimeAttention, RuntimeStatus, ServerEvent,
    ServerResponse, Snapshot,
};
use ditch_store::{
    DitchStore, discover_legacy_projects, ensure_app_dirs, ensure_project_metadata,
    load_project_config, save_project_config,
};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

mod git_orchestration;
use git_orchestration::{GitCoordinator, apply_overlap_projection};

const RUNTIME_IDENTITY: &str = "The Ditch Runtime";
const CODEX_BINARY_SETTING: &str = "codex_binary";
const STOP_INTERRUPT_GRACE: Duration = Duration::from_millis(1500);
const STOP_TERMINATE_GRACE: Duration = Duration::from_millis(500);
const STOP_KILL_GRACE: Duration = Duration::from_millis(1500);
static LOGIN_SHELL_PATH: OnceLock<Option<OsString>> = OnceLock::new();

#[allow(dead_code)]
fn main() {
    if let Err(error) = run_runtime() {
        eprintln!("{RUNTIME_IDENTITY} failed: {error}");
        std::process::exit(1);
    }
}

fn run_runtime() -> io::Result<()> {
    let paths = AppPaths::for_current_user();
    validate_codex_home()
        .and_then(|_| ensure_app_dirs(&paths).map_err(io::Error::other))
        .and_then(|_| serve(paths))
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
    worktrees: HashMap<AgentId, ManagedWorktree>,
    git: Arc<GitCoordinator>,
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
}

struct ProjectTerminalRecord {
    descriptor: ProjectTerminal,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn portable_pty::Child + Send>,
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
}

impl RuntimeState {
    fn new(paths: AppPaths) -> Result<Self, ditch_store::StoreError> {
        let mut store = DitchStore::open(&paths)?;
        store.reconcile_active_agents()?;
        let durable = store.load()?;
        let projects = durable
            .projects
            .into_iter()
            .map(|project| (project.root_key(), project))
            .collect::<HashMap<_, _>>();
        let durable_worktrees = durable.worktrees;
        let durable_project_git_operations = durable.project_git_operations;
        let mut agents: HashMap<AgentId, AgentRecord> = durable
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
        for worktree in &durable_worktrees {
            if agents.contains_key(&worktree.session_id) {
                continue;
            }
            let Some(project) = projects
                .values()
                .find(|project| project.id == worktree.project_id)
            else {
                continue;
            };
            let prompt = worktree
                .intent
                .as_ref()
                .map(|intent| intent.summary.clone())
                .unwrap_or_else(|| "Recovered Ditch worktree".to_owned());
            let run = AgentRun {
                id: worktree.session_id,
                provider: AgentProvider::Codex,
                state: AgentState::Stale,
                can_stop: false,
                launch_mode: CodexLaunchMode::Exec,
                execution_profile: AgentExecutionProfile::default(),
                project_id: project.id,
                task_id: None,
                pane_id: None,
                native_session_id: None,
                codex_title: None,
                user_title: Some("Recovered agent work".into()),
                origin_codex_home: std::env::var("CODEX_HOME").ok(),
                current_prompt: Some(prompt.clone()),
                last_visible_action: Some(
                    "Recovered after an interrupted worktree operation".into(),
                ),
                state_confidence: 1.0,
                state_evidence: "The durable worktree journal survived without its session row."
                    .into(),
                started_at: worktree.created_at,
                updated_at: Utc::now(),
                finished_at: Some(Utc::now()),
                exit_code: None,
                resume_block_reason: Some(AgentResumeBlockReason::NoCodexThread),
            };
            let message = AgentChatMessage {
                agent_id: run.id,
                role: AgentChatRole::System,
                text: "Ditch recovered this isolated workspace after an interrupted launch. Its files remain preserved.".into(),
                created_at: Utc::now(),
            };
            let home = std::env::var("CODEX_HOME").ok();
            if store
                .persist_new_agent(&run, &message, home.as_deref())
                .is_ok()
            {
                agents.insert(
                    run.id,
                    AgentRecord {
                        run,
                        project_root: project.root.clone(),
                        allow_non_git: false,
                        messages: vec![message],
                        terminal_failure: None,
                    },
                );
            }
        }
        let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from);
        let selected_codex = store.setting(CODEX_BINARY_SETTING)?;
        let codex_binary = resolve_codex_binary(selected_codex.as_deref());
        if selected_codex.as_deref() != codex_binary.as_deref()
            && let Some(binary) = codex_binary.as_deref()
        {
            store.set_setting(CODEX_BINARY_SETTING, binary)?;
        }
        let mut runtime = Self {
            paths,
            store,
            projects,
            agents,
            worktrees: durable_worktrees
                .into_iter()
                .map(|worktree| (worktree.session_id, worktree))
                .collect(),
            git: Arc::new(GitCoordinator::default()),
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
        };
        runtime.reconcile_project_git_operations(durable_project_git_operations);
        runtime.reconcile_worktrees();
        Ok(runtime)
    }

    fn runtime_status(&self) -> RuntimeStatus {
        RuntimeStatus {
            identity: RUNTIME_IDENTITY.to_owned(),
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
            capabilities: vec![
                "persistent_sessions_v1".to_owned(),
                "attention_stream_v1".to_owned(),
                "transcript_pagination_v1".to_owned(),
                "project_files_v1".to_owned(),
                "persistent_attention_read_v1".to_owned(),
                "always_on_web_access_v1".to_owned(),
                "git_worktree_orchestration_v1".to_owned(),
                "initial_project_snapshot_v1".to_owned(),
            ],
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
            worktrees: self.worktrees.values().cloned().collect(),
        }
    }

    fn reconcile_worktrees(&mut self) {
        let ids = self.worktrees.keys().copied().collect::<Vec<_>>();
        for agent_id in ids {
            let Some(mut worktree) = self.worktrees.get(&agent_id).cloned() else {
                continue;
            };
            let Some(project) = self
                .projects
                .values()
                .find(|item| item.id == worktree.project_id)
                .cloned()
            else {
                continue;
            };
            if let Err(error) = self.git.reconcile_operation(&project, &mut worktree) {
                worktree.status = WorktreeStatus::RecoveryNeeded;
                worktree.last_error = Some(error.to_string());
            }
            if worktree.status == WorktreeStatus::Integrated
                && worktree.path.exists()
                && let Err(error) = self.git.archive_integrated(&project, &mut worktree)
            {
                worktree.status = WorktreeStatus::CleanupPending;
                worktree.last_error = Some(error.to_string());
            }
            let _ = self.store.upsert_worktree(&worktree);
            self.worktrees.insert(agent_id, worktree);
        }
        let mut projection = self.worktrees.values().cloned().collect::<Vec<_>>();
        apply_overlap_projection(&mut projection);
        for item in projection {
            let _ = self.store.upsert_worktree(&item);
            self.worktrees.insert(item.session_id, item);
        }
    }

    fn reconcile_project_git_operations(
        &mut self,
        operations: Vec<ditch_core::ProjectGitOperation>,
    ) {
        for mut operation in operations {
            let Some(project) = self
                .projects
                .values()
                .find(|project| project.id == operation.project_id)
                .cloned()
            else {
                continue;
            };
            if let Err(error) = self
                .git
                .reconcile_initial_snapshot(&project, &mut operation)
            {
                operation.state = ditch_core::ProjectGitOperationState::RecoveryNeeded;
                operation.last_error = Some(error.to_string());
                operation.updated_at = Utc::now();
            }
            let _ = self.store.upsert_project_git_operation(&operation);
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

fn serve(paths: AppPaths) -> io::Result<()> {
    remove_stale_socket(&paths.socket_path)?;
    let listener = UnixListener::bind(&paths.socket_path)?;
    let state = Arc::new(Mutex::new(
        RuntimeState::new(paths).map_err(io::Error::other)?,
    ));

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
            event: ServerEvent::SnapshotReplaced(Box::new(state.snapshot())),
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
            ServerResponse::Snapshot(Box::new(state.snapshot()))
        }
        ClientRequest::ListAgentMessages {
            agent_id,
            before_sequence,
            limit,
        } => {
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
            let initialized_repository =
                project.git_policy == ProjectGitPolicy::InitializeRepository;
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
            let runtime = Arc::clone(&state);
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if let Err(error) = state.store.upsert_project(&project) {
                return protocol_error("project_store_failed", error.to_string());
            }
            state.projects.insert(project.root_key(), project.clone());
            state.broadcast(ServerEvent::ProjectChanged(project.clone()));
            drop(state);
            if initialized_repository {
                let readiness = inspect_project_agent_readiness(Arc::clone(&runtime), project.id);
                if let ServerResponse::ProjectAgentReadiness(readiness) = readiness
                    && readiness.state == ProjectAgentReadinessState::NeedsInitialSnapshot
                    && readiness.included_file_count == 0
                    && let Some(tree_oid) = readiness.snapshot_tree_oid
                    && let ServerResponse::Error(error) =
                        create_initial_project_snapshot(runtime, project.id, &tree_oid)
                {
                    return ServerResponse::Error(error);
                }
            }
            ServerResponse::ProjectCreated(project)
        }
        ClientRequest::SetProjectIntegrationPolicy { project_id, policy } => {
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            let Some(key) = state
                .projects
                .iter()
                .find_map(|(key, project)| (project.id == project_id).then(|| key.clone()))
            else {
                return protocol_error("project_not_found", "The project was not found");
            };
            let mut project = state.projects[&key].clone();
            project.integration_policy = policy;
            if let Err(error) = state.store.upsert_project(&project) {
                return protocol_error("project_update_failed", error.to_string());
            }
            state.projects.insert(key, project.clone());
            state.broadcast(ServerEvent::ProjectChanged(project));
            ServerResponse::Accepted
        }
        ClientRequest::DeleteProject { project_id } => delete_project(state, project_id),
        ClientRequest::InspectProjectAgentReadiness { project_id } => {
            inspect_project_agent_readiness(state, project_id)
        }
        ClientRequest::CreateInitialProjectSnapshot {
            project_id,
            expected_tree_oid,
        } => create_initial_project_snapshot(state, project_id, &expected_tree_oid),
        ClientRequest::StartCodexSession {
            project_name,
            project_root,
            prompt,
            mode,
            execution_profile,
        } => start_codex_session(
            state,
            project_name,
            project_root,
            prompt,
            mode,
            execution_profile,
        ),
        ClientRequest::ResumeCodexSession {
            project_name,
            project_root,
            thread_id,
            prompt,
            execution_profile,
        } => resume_codex_session(
            state,
            project_name,
            project_root,
            thread_id,
            prompt,
            execution_profile,
        ),
        ClientRequest::PromptAgent {
            agent_id,
            prompt,
            execution_profile,
        } => prompt_agent(state, agent_id, prompt, execution_profile),
        ClientRequest::ListAgentModels { provider } => list_agent_models(state, provider),
        ClientRequest::OpenProjectTerminal {
            project_id,
            columns,
            rows,
        } => open_project_terminal(state, project_id, columns, rows),
        ClientRequest::WriteProjectTerminal { terminal_id, data } => {
            write_project_terminal(state, terminal_id, data)
        }
        ClientRequest::ResizeProjectTerminal {
            terminal_id,
            columns,
            rows,
        } => resize_project_terminal(state, terminal_id, columns, rows),
        ClientRequest::CloseProjectTerminal { terminal_id } => {
            close_project_terminal(state, terminal_id)
        }
        ClientRequest::ListProjectDirectory {
            project_id,
            relative_path,
        } => list_project_directory(state, project_id, &relative_path),
        ClientRequest::ReadProjectFile {
            project_id,
            relative_path,
        } => read_project_file(state, project_id, &relative_path),
        ClientRequest::WriteProjectFile {
            project_id,
            relative_path,
            expected_revision,
            content,
        } => write_project_file(
            state,
            project_id,
            &relative_path,
            expected_revision.as_deref(),
            &content,
        ),
        ClientRequest::StopAgent { agent_id } => stop_agent(state, agent_id),
        ClientRequest::DeleteAgent { agent_id } => delete_agent(state, agent_id),
        ClientRequest::RenameAgent { agent_id, title } => rename_agent(state, agent_id, title),
        ClientRequest::RegisterChangeIntent {
            agent_id,
            intent,
            continue_on_overlap,
        } => register_change_intent(state, agent_id, intent, continue_on_overlap),
        ClientRequest::RefreshWorktree { agent_id } => refresh_worktree(state, agent_id),
        ClientRequest::OverrideWorktreeOverlap { agent_id } => {
            override_worktree_overlap(state, agent_id)
        }
        ClientRequest::GetWorktreeReview { agent_id } => get_worktree_review(state, agent_id),
        ClientRequest::PrepareIntegration { agent_id } => prepare_integration(state, agent_id),
        ClientRequest::ApplyIntegration { agent_id } => apply_integration(state, agent_id, false),
        ClientRequest::ApplyIntegrationWithoutValidation { agent_id } => {
            apply_integration(state, agent_id, true)
        }
        ClientRequest::CreateConflictResolution { agent_id } => {
            create_conflict_resolution(state, agent_id)
        }
        ClientRequest::FinalizeConflictResolution { agent_id } => {
            finalize_conflict_resolution(state, agent_id)
        }
        ClientRequest::DiscardWorktree {
            agent_id,
            confirm_dirty,
        } => discard_worktree(state, agent_id, confirm_dirty),
        ClientRequest::DismissAttention { attention_id } => {
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
            mark_attention_read(state, Some(&attention_ids))
        }
        ClientRequest::MarkAllAttentionRead => mark_attention_read(state, None),
        ClientRequest::StartCodex { .. }
        | ClientRequest::ApprovePermission { .. }
        | ClientRequest::DenyPermission { .. }
        | ClientRequest::SubscribeEvents { .. }
        | ClientRequest::SubscribeAttention => protocol_error(
            "unsupported_request",
            "request is not implemented by this runtime",
        ),
    }
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
    if !allow_non_git && execution_profile.approval == AgentApprovalPreset::FullAccess {
        return protocol_error(
            "isolated_full_access_unsupported",
            "Full Access is unavailable for isolated parallel work because it would remove the workspace safety boundary.",
        );
    }
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
    if !allow_non_git && let Err(error) = prepare_project_snapshot_for_launch(&state, &project) {
        return protocol_error(error.code, error.message);
    }
    let managed = if allow_non_git {
        None
    } else {
        match create_journaled_worktree(&state, &project, run.id, &prompt) {
            Ok(created) => Some(created),
            Err(error) => {
                return persist_launch_failure(
                    &state,
                    project,
                    run,
                    user_message,
                    allow_non_git,
                    error.message,
                );
            }
        }
    };
    let execution_root = managed
        .as_ref()
        .map(|created| created.agent_cwd.as_path())
        .unwrap_or(&project.root);
    let codex_prompt = managed_agent_prompt(&prompt, managed.is_some());
    let child = match spawn_codex_child(
        &binary,
        execution_root,
        &codex_prompt,
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
        if let Some(created) = managed.as_ref() {
            if let Err(error) = state.store.upsert_worktree(&created.managed) {
                let _ = child
                    .lock()
                    .expect("child lock should not be poisoned")
                    .kill();
                return protocol_error("worktree_store_failed", error.to_string());
            }
            state.worktrees.insert(run.id, created.managed.clone());
            emit_git_event(
                &mut state,
                project.id,
                Some(run.id),
                "worktree_created",
                "isolated locked workspace created",
            );
        }
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
            },
        );
        state.broadcast(ServerEvent::ProjectChanged(project));
        state.broadcast(ServerEvent::AgentChanged(run.clone()));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), run.id, run_id, child);
    if managed.is_some() {
        monitor_worktree(Arc::clone(&state), run.id, run_id);
    }
    ServerResponse::AgentStarted(Box::new(run))
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
    state.broadcast(ServerEvent::AgentChanged(run.clone()));
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

fn project_by_id(state: &Arc<Mutex<RuntimeState>>, project_id: ProjectId) -> Option<Project> {
    state
        .lock()
        .expect("runtime state lock should not be poisoned")
        .projects
        .values()
        .find(|project| project.id == project_id)
        .cloned()
}

fn inspect_project_agent_readiness(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
) -> ServerResponse {
    let Some(project) = project_by_id(&state, project_id) else {
        return protocol_error("project_not_found", "project was not found");
    };
    if project.git_policy == ProjectGitPolicy::AllowOutsideGit {
        return ServerResponse::ProjectAgentReadiness(ProjectAgentReadiness {
            state: ProjectAgentReadinessState::Ready,
            repository_root: project.root.to_string_lossy().into_owned(),
            target_branch: String::new(),
            included_file_count: 0,
            sample_paths: Vec::new(),
            warnings: Vec::new(),
            snapshot_tree_oid: None,
        });
    }
    let git = {
        Arc::clone(
            &state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .git,
        )
    };
    match git.inspect_initial_snapshot(&project) {
        Ok(preview) => ServerResponse::ProjectAgentReadiness(ProjectAgentReadiness {
            state: if preview.ready {
                ProjectAgentReadinessState::Ready
            } else if preview.unsafe_to_snapshot {
                ProjectAgentReadinessState::UnsafeInitialSnapshot
            } else {
                ProjectAgentReadinessState::NeedsInitialSnapshot
            },
            repository_root: preview.repository_root.to_string_lossy().into_owned(),
            target_branch: preview.target_branch,
            included_file_count: preview.included_paths.len(),
            sample_paths: preview.included_paths.into_iter().take(20).collect(),
            warnings: preview.warnings,
            snapshot_tree_oid: preview.tree_oid,
        }),
        Err(error) => protocol_error(error.code, error.message),
    }
}

fn create_initial_project_snapshot(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    expected_tree_oid: &str,
) -> ServerResponse {
    let Some(project) = project_by_id(&state, project_id) else {
        return protocol_error("project_not_found", "project was not found");
    };
    if project.git_policy == ProjectGitPolicy::AllowOutsideGit {
        return protocol_error(
            "initial_snapshot_not_applicable",
            "This project is configured to run outside Git",
        );
    }
    let git = {
        Arc::clone(
            &state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .git,
        )
    };
    let result = git.create_initial_snapshot(&project, expected_tree_oid, |operation| {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .store
            .upsert_project_git_operation(operation)
            .map_err(|error| git_orchestration::GitError {
                code: "project_git_journal_failed",
                message: error.to_string(),
            })?;
        emit_git_event(
            &mut state,
            project_id,
            None,
            "initial_snapshot_state_changed",
            &format!("initial project snapshot: {:?}", operation.state),
        );
        Ok(())
    });
    match result {
        Ok(commit_oid) => {
            ServerResponse::InitialProjectSnapshotCreated(InitialProjectSnapshotCreated {
                commit_oid,
            })
        }
        Err(error) => protocol_error(error.code, error.message),
    }
}

fn prepare_project_snapshot_for_launch(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
) -> Result<(), git_orchestration::GitError> {
    let git = {
        Arc::clone(
            &state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .git,
        )
    };
    let preview = git.inspect_initial_snapshot(project)?;
    if preview.ready {
        return Ok(());
    }
    if preview.unsafe_to_snapshot || !preview.warnings.is_empty() {
        return Err(git_orchestration::GitError {
            code: "project_snapshot_review_required",
            message: if preview.warnings.is_empty() {
                "Some project files need review before Ditch can prepare them for agents.".into()
            } else {
                preview.warnings.join(" ")
            },
        });
    }
    let tree_oid = preview.tree_oid.ok_or(git_orchestration::GitError {
        code: "project_snapshot_missing",
        message: "Ditch could not prepare the current project files.".into(),
    })?;
    git.create_initial_snapshot(project, &tree_oid, |operation| {
        let mut runtime = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        runtime
            .store
            .upsert_project_git_operation(operation)
            .map_err(|error| git_orchestration::GitError {
                code: "project_git_journal_failed",
                message: error.to_string(),
            })?;
        emit_git_event(
            &mut runtime,
            project.id,
            None,
            "project_snapshot_state_changed",
            &format!("project preparation: {:?}", operation.state),
        );
        Ok(())
    })?;
    let verified = git.inspect_initial_snapshot(project)?;
    if !verified.ready {
        return Err(git_orchestration::GitError {
            code: "project_changed_during_preparation",
            message: "Project files changed while Ditch was preparing them. Try starting the agent again.".into(),
        });
    }
    Ok(())
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
    if !allow_non_git && execution_profile.approval == AgentApprovalPreset::FullAccess {
        return protocol_error(
            "isolated_full_access_unsupported",
            "Full Access is unavailable for isolated parallel work because it would remove the workspace safety boundary.",
        );
    }
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
    if !allow_non_git && let Err(error) = prepare_project_snapshot_for_launch(&state, &project) {
        return protocol_error(error.code, error.message);
    }
    let managed = if allow_non_git {
        None
    } else {
        match create_journaled_worktree(&state, &project, run.id, &prompt) {
            Ok(created) => Some(created),
            Err(error) => {
                return persist_launch_failure(
                    &state,
                    project,
                    run,
                    user_message,
                    allow_non_git,
                    error.message,
                );
            }
        }
    };
    let execution_root = managed
        .as_ref()
        .map(|created| created.agent_cwd.as_path())
        .unwrap_or(&project.root);
    let codex_prompt = managed_agent_prompt(&prompt, managed.is_some());
    let child = match spawn_codex_child(
        &binary,
        execution_root,
        &codex_prompt,
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
        if let Some(created) = managed.as_ref() {
            if let Err(error) = state.store.upsert_worktree(&created.managed) {
                let _ = child
                    .lock()
                    .expect("child lock should not be poisoned")
                    .kill();
                return protocol_error("worktree_store_failed", error.to_string());
            }
            state.worktrees.insert(run.id, created.managed.clone());
            emit_git_event(
                &mut state,
                project.id,
                Some(run.id),
                "worktree_created",
                "isolated locked workspace created",
            );
        }
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
            },
        );
        state.broadcast(ServerEvent::ProjectChanged(project));
        state.broadcast(ServerEvent::AgentChanged(run.clone()));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), run.id, run_id, child);
    if managed.is_some() {
        monitor_worktree(Arc::clone(&state), run.id, run_id);
    }
    ServerResponse::AgentStarted(Box::new(run))
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
        let execution_root = state
            .worktrees
            .get(&agent_id)
            .map(|worktree| {
                if worktree.agent_cwd.as_os_str().is_empty() {
                    worktree.path.clone()
                } else {
                    worktree.agent_cwd.clone()
                }
            })
            .unwrap_or_else(|| record.project_root.clone());
        if state.worktrees.contains_key(&agent_id)
            && execution_profile.approval == AgentApprovalPreset::FullAccess
        {
            return protocol_error(
                "isolated_full_access_unsupported",
                "Full Access is unavailable for isolated parallel work because it would remove the workspace safety boundary.",
            );
        }
        (
            execution_root,
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
            },
        );
        state.broadcast(ServerEvent::AgentChanged(run));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
        if let Some(worktree) = state.worktrees.get_mut(&agent_id) {
            worktree.status = if worktree.dirty {
                WorktreeStatus::Dirty
            } else {
                WorktreeStatus::Active
            };
            worktree.integration_state = IntegrationState::NotRequested;
            worktree.validation_state = ditch_core::ValidationState::NotRun;
            worktree.conflict_state = ditch_core::ConflictState::None;
            worktree.last_error = None;
            worktree.updated_at = Utc::now();
            let changed = worktree.clone();
            let _ = state.store.upsert_worktree(&changed);
            state.broadcast(ServerEvent::WorktreeChanged(Box::new(changed)));
        }
    }

    attach_codex_io(Arc::clone(&state), agent_id, run_id, child);
    ServerResponse::Accepted
}

fn register_change_intent(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    intent: ChangeIntent,
    continue_on_overlap: bool,
) -> ServerResponse {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(worktree) = state.worktrees.get_mut(&agent_id) else {
        return protocol_error(
            "worktree_not_found",
            "This agent does not own a managed worktree",
        );
    };
    worktree.intent = Some(intent);
    worktree.overlap_override = continue_on_overlap;
    worktree.updated_at = Utc::now();
    let project_id = worktree.project_id;
    let mut projection = state.worktrees.values().cloned().collect::<Vec<_>>();
    apply_overlap_projection(&mut projection);
    let has_overlap = projection
        .iter()
        .find(|item| item.session_id == agent_id)
        .is_some_and(|item| !item.overlapping_session_ids.is_empty());
    for item in projection {
        let _ = state.store.upsert_worktree(&item);
        state.worktrees.insert(item.session_id, item.clone());
        state.broadcast(ServerEvent::WorktreeChanged(Box::new(item)));
    }
    emit_git_event(
        &mut state,
        project_id,
        Some(agent_id),
        "change_intent_registered",
        if has_overlap {
            "overlap detected"
        } else {
            "no overlap detected"
        },
    );
    ServerResponse::Accepted
}

fn refresh_worktree(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    match refresh_worktree_inner(&state, agent_id) {
        Ok(()) => ServerResponse::Accepted,
        Err((code, message)) => protocol_error(code, message),
    }
}

fn override_worktree_overlap(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(worktree) = state.worktrees.get_mut(&agent_id) else {
        return protocol_error(
            "worktree_not_found",
            "This agent does not own a managed worktree",
        );
    };
    worktree.overlap_override = true;
    worktree.updated_at = Utc::now();
    persist_overlap_projection(&mut state);
    let project_id = state
        .worktrees
        .get(&agent_id)
        .map(|worktree| worktree.project_id);
    if let Some(project_id) = project_id {
        emit_git_event(
            &mut state,
            project_id,
            Some(agent_id),
            "path_overlap_overridden",
            "user chose to continue separately; automatic integration remains guarded",
        );
    }
    ServerResponse::Accepted
}

fn get_worktree_review(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let (git, project, worktree) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(worktree) = state.worktrees.get(&agent_id).cloned() else {
            return protocol_error(
                "worktree_not_found",
                "This agent does not own a managed worktree",
            );
        };
        let Some(project) = state
            .projects
            .values()
            .find(|item| item.id == worktree.project_id)
            .cloned()
        else {
            return protocol_error("project_not_found", "The worktree project was not found");
        };
        (Arc::clone(&state.git), project, worktree)
    };
    match git.review(&project, &worktree) {
        Ok(review) => ServerResponse::WorktreeReview(review),
        Err(error) => protocol_error(error.code, error.message),
    }
}

fn refresh_worktree_inner(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
) -> Result<(), (&'static str, String)> {
    let (git, project, mut worktree) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let worktree = state.worktrees.get(&agent_id).cloned().ok_or((
            "worktree_not_found",
            "This agent does not own a managed worktree".to_owned(),
        ))?;
        let project = state
            .projects
            .values()
            .find(|item| item.id == worktree.project_id)
            .cloned()
            .ok_or((
                "project_not_found",
                "The worktree project was not found".to_owned(),
            ))?;
        (Arc::clone(&state.git), project, worktree)
    };
    git.refresh(&project, &mut worktree)
        .map_err(|error| (error.code, error.message))?;
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state.worktrees.insert(agent_id, worktree);
    persist_overlap_projection(&mut state);
    Ok(())
}

fn prepare_integration(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let (git, project, mut worktree, validation_root) = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(mut worktree) = state.worktrees.get(&agent_id).cloned() else {
            return protocol_error(
                "worktree_not_found",
                "This agent does not own a managed worktree",
            );
        };
        if state.children.contains_key(&agent_id)
            || state.agents.get(&agent_id).is_some_and(|record| {
                matches!(record.run.state, AgentState::Working | AgentState::Stopping)
            })
        {
            return protocol_error(
                "agent_active",
                "This agent is still working. Ditch will check its result when it finishes.",
            );
        }
        let Some(project) = state
            .projects
            .values()
            .find(|item| item.id == worktree.project_id)
            .cloned()
        else {
            return protocol_error("project_not_found", "The worktree project was not found");
        };
        worktree.operation = Some("prepare_integration".into());
        worktree.status = WorktreeStatus::QueuedForIntegration;
        worktree.integration_state = IntegrationState::Queued;
        worktree.updated_at = Utc::now();
        let _ = state.store.upsert_worktree(&worktree);
        state.worktrees.insert(agent_id, worktree.clone());
        state.broadcast(ServerEvent::WorktreeChanged(Box::new(worktree.clone())));
        emit_git_event(
            &mut state,
            project.id,
            Some(agent_id),
            "integration_queued",
            "candidate queued",
        );
        (
            Arc::clone(&state.git),
            project,
            worktree,
            state.paths.data_dir.join("validation-worktrees"),
        )
    };
    let result = (|| {
        git.checkpoint(&project, &mut worktree)?;
        git.prepare_integration(&project, &mut worktree, &validation_root)
    })();
    worktree.operation = None;
    if let Err(error) = result {
        worktree.status = WorktreeStatus::NeedsReview;
        worktree.integration_state = IntegrationState::Blocked;
        worktree.last_error = Some(error.message.clone());
        persist_one_worktree(&state, worktree);
        return protocol_error(error.code, error.message);
    }
    let conflict = worktree.status == WorktreeStatus::ConflictRisk;
    let ready = worktree.status == WorktreeStatus::ReadyToApply;
    let project_id = worktree.project_id;
    persist_one_worktree(&state, worktree);
    let mut locked = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    emit_git_event(
        &mut locked,
        project_id,
        Some(agent_id),
        if conflict {
            "integration_conflict_detected"
        } else if ready {
            "validation_passed"
        } else {
            "validation_failed"
        },
        if conflict {
            "Git merge simulation reported a conflict"
        } else if ready {
            "combined candidate validation passed"
        } else {
            "combined candidate validation failed"
        },
    );
    drop(locked);
    if ready
        && project.integration_policy == ditch_core::IntegrationPolicy::AutoApplyAfterValidation
    {
        return apply_integration(state, agent_id, false);
    }
    ServerResponse::Accepted
}

fn apply_integration(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    allow_without_validation: bool,
) -> ServerResponse {
    let (git, project, mut worktree) = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(mut worktree) = state.worktrees.get(&agent_id).cloned() else {
            return protocol_error(
                "worktree_not_found",
                "This agent does not own a managed worktree",
            );
        };
        if state.children.contains_key(&agent_id) {
            return protocol_error(
                "agent_active",
                "This agent is still working. Its current result cannot be applied yet.",
            );
        }
        let applyable = worktree.status == WorktreeStatus::ReadyToApply
            || (allow_without_validation
                && worktree.status == WorktreeStatus::NeedsReview
                && worktree.validation_state == ditch_core::ValidationState::NotConfigured);
        if !applyable {
            return protocol_error(
                "integration_not_ready",
                "This result must finish its safety checks before it can be applied.",
            );
        }
        let Some(project) = state
            .projects
            .values()
            .find(|item| item.id == worktree.project_id)
            .cloned()
        else {
            return protocol_error("project_not_found", "The worktree project was not found");
        };
        worktree.operation = Some("apply_integration".into());
        worktree.status = WorktreeStatus::Applying;
        worktree.integration_state = IntegrationState::Applying;
        let _ = state.store.upsert_worktree(&worktree);
        state.worktrees.insert(agent_id, worktree.clone());
        (Arc::clone(&state.git), project, worktree)
    };
    let result = git.apply(&project, &mut worktree, allow_without_validation);
    worktree.operation = None;
    if let Err(error) = result {
        if worktree.status == WorktreeStatus::Applying {
            worktree.status = if worktree.validation_state == ditch_core::ValidationState::Passed {
                WorktreeStatus::ReadyToApply
            } else {
                WorktreeStatus::NeedsReview
            };
            worktree.integration_state = ditch_core::IntegrationState::Blocked;
            worktree.last_error = Some(error.message.clone());
        }
        persist_one_worktree(&state, worktree);
        return protocol_error(error.code, error.message);
    }
    if let Err(error) = git.archive_integrated(&project, &mut worktree) {
        worktree.status = WorktreeStatus::CleanupPending;
        worktree.last_error = Some(error.message);
    }
    let project_id = worktree.project_id;
    persist_one_worktree(&state, worktree);
    let remaining = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        emit_git_event(
            &mut state,
            project_id,
            Some(agent_id),
            "integration_applied",
            "canonical target advanced safely",
        );
        state
            .worktrees
            .values()
            .filter(|item| {
                item.project_id == project_id
                    && item.session_id != agent_id
                    && item.checkpoint_oid.is_some()
                    && !state.children.contains_key(&item.session_id)
                    && state
                        .agents
                        .get(&item.session_id)
                        .is_some_and(|record| record.run.state == AgentState::Completed)
                    && !matches!(
                        item.status,
                        WorktreeStatus::Integrated | WorktreeStatus::Discarded
                    )
            })
            .map(|item| item.session_id)
            .collect::<Vec<_>>()
    };
    for candidate in remaining {
        let _ = prepare_integration(Arc::clone(&state), candidate);
    }
    ServerResponse::Accepted
}

fn create_conflict_resolution(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
) -> ServerResponse {
    let (git, project, mut worktree, resolution_root) = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if state.children.contains_key(&agent_id) {
            return protocol_error(
                "agent_active",
                "Stop this agent before opening a conflict-resolution workspace",
            );
        }
        let Some(mut worktree) = state.worktrees.get(&agent_id).cloned() else {
            return protocol_error("worktree_not_found", "The agent workspace was not found");
        };
        let Some(project) = state
            .projects
            .values()
            .find(|project| project.id == worktree.project_id)
            .cloned()
        else {
            return protocol_error("project_not_found", "The project was not found");
        };
        let resolution_root = state.paths.data_dir.join("resolution-worktrees");
        worktree.resolution_path = Some(
            resolution_root
                .join(project.id.0.simple().to_string())
                .join(agent_id.0.simple().to_string()),
        );
        worktree.resolution_branch = Some(format!("ditch/resolver/{}", agent_id.0.simple()));
        worktree.operation = Some("create_resolution".into());
        worktree.updated_at = Utc::now();
        let _ = state.store.upsert_worktree(&worktree);
        (Arc::clone(&state.git), project, worktree, resolution_root)
    };
    match git.create_conflict_resolution(&project, &mut worktree, &resolution_root) {
        Ok(path) => {
            worktree.operation = None;
            persist_one_worktree(&state, worktree);
            ServerResponse::ConflictResolutionWorkspace {
                path: path.to_string_lossy().into_owned(),
            }
        }
        Err(error) => {
            worktree.operation = None;
            if worktree
                .resolution_path
                .as_ref()
                .is_some_and(|path| !path.exists())
            {
                worktree.resolution_path = None;
                worktree.resolution_branch = None;
                worktree.resolution_target_oid = None;
            }
            persist_one_worktree(&state, worktree);
            protocol_error(error.code, error.message)
        }
    }
}

fn finalize_conflict_resolution(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
) -> ServerResponse {
    let (git, project, mut worktree, validation_root) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(worktree) = state.worktrees.get(&agent_id).cloned() else {
            return protocol_error("worktree_not_found", "The agent workspace was not found");
        };
        let Some(project) = state
            .projects
            .values()
            .find(|project| project.id == worktree.project_id)
            .cloned()
        else {
            return protocol_error("project_not_found", "The project was not found");
        };
        (
            Arc::clone(&state.git),
            project,
            worktree,
            state.paths.data_dir.join("validation-worktrees"),
        )
    };
    if let Err(error) = git.finalize_conflict_resolution(&project, &mut worktree, &validation_root)
    {
        worktree.last_error = Some(error.message.clone());
        persist_one_worktree(&state, worktree);
        return protocol_error(error.code, error.message);
    }
    let ready = worktree.status == WorktreeStatus::ReadyToApply;
    persist_one_worktree(&state, worktree);
    if ready
        && project.integration_policy == ditch_core::IntegrationPolicy::AutoApplyAfterValidation
    {
        return apply_integration(state, agent_id, false);
    }
    ServerResponse::Accepted
}

fn discard_worktree(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    confirm_dirty: bool,
) -> ServerResponse {
    let (git, project, mut worktree) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if state.children.contains_key(&agent_id) {
            return protocol_error(
                "agent_active",
                "Stop this agent before discarding its workspace",
            );
        }
        let Some(worktree) = state.worktrees.get(&agent_id).cloned() else {
            return protocol_error(
                "worktree_not_found",
                "This agent does not own a managed worktree",
            );
        };
        let Some(project) = state
            .projects
            .values()
            .find(|item| item.id == worktree.project_id)
            .cloned()
        else {
            return protocol_error("project_not_found", "The worktree project was not found");
        };
        (Arc::clone(&state.git), project, worktree)
    };
    if let Err(error) = git.discard(&project, &mut worktree, confirm_dirty) {
        persist_one_worktree(&state, worktree);
        return protocol_error(error.code, error.message);
    }
    let project_id = worktree.project_id;
    persist_one_worktree(&state, worktree);
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    emit_git_event(
        &mut state,
        project_id,
        Some(agent_id),
        "worktree_removed",
        "managed workspace removed with Git",
    );
    ServerResponse::Accepted
}

fn persist_one_worktree(state: &Arc<Mutex<RuntimeState>>, worktree: ManagedWorktree) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let _ = state.store.upsert_worktree(&worktree);
    state
        .worktrees
        .insert(worktree.session_id, worktree.clone());
    state.broadcast(ServerEvent::WorktreeChanged(Box::new(worktree)));
}

fn persist_overlap_projection(state: &mut RuntimeState) {
    let mut worktrees = state.worktrees.values().cloned().collect::<Vec<_>>();
    apply_overlap_projection(&mut worktrees);
    for worktree in worktrees {
        let _ = state.store.upsert_worktree(&worktree);
        state
            .worktrees
            .insert(worktree.session_id, worktree.clone());
        state.broadcast(ServerEvent::WorktreeChanged(Box::new(worktree)));
    }
}

fn emit_git_event(
    state: &mut RuntimeState,
    project_id: ProjectId,
    agent_id: Option<AgentId>,
    kind: &str,
    detail: &str,
) {
    let _ = state
        .store
        .append_git_event(project_id, agent_id, kind, detail);
    state.broadcast(ServerEvent::GitLifecycle {
        project_id,
        agent_id,
        kind: kind.to_owned(),
        detail: detail.to_owned(),
    });
}

fn stop_agent(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let active = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let active = state.children.get(&agent_id).cloned();
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
    state.broadcast(ServerEvent::AgentChanged(run.clone()));
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
    if state.worktrees.get(&agent_id).is_some_and(|worktree| {
        !matches!(
            worktree.status,
            WorktreeStatus::Discarded | WorktreeStatus::Integrated
        ) || worktree.path.exists()
    }) {
        return protocol_error(
            "managed_worktree_preserved",
            "Discard this agent's preserved workspace before deleting its history.",
        );
    }
    if let Err(error) = state.store.delete_agent(agent_id) {
        return protocol_error("agent_delete_failed", error.to_string());
    }
    state.agents.remove(&agent_id);
    state.worktrees.remove(&agent_id);
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
    if state.worktrees.values().any(|worktree| {
        worktree.project_id == project_id
            && (!matches!(
                worktree.status,
                WorktreeStatus::Discarded | WorktreeStatus::Integrated
            ) || worktree.path.exists())
    }) {
        return protocol_error(
            "project_worktrees_preserved",
            "Discard or apply this project's preserved agent work before removing the project from Ditch.",
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
        state.worktrees.remove(agent_id);
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

fn monitor_worktree(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId, run_id: uuid::Uuid) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(1500));
            let active = {
                let state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
                state
                    .children
                    .get(&agent_id)
                    .is_some_and(|child| child.run_id == run_id)
            };
            let _ = refresh_worktree_inner(&state, agent_id);
            if !active {
                break;
            }
        }
    });
}

fn managed_agent_prompt(prompt: &str, isolated: bool) -> String {
    if !isolated {
        return prompt.to_owned();
    }
    format!(
        "{prompt}\n\n[Ditch workspace boundary]\nYou are operating inside a Ditch-managed isolated Git worktree. Treat the current directory as the complete project workspace. Do not switch branches, merge or rebase the target branch, create/remove worktrees, modify another Ditch worktree or the canonical project directory, delete/relocate this worktree, or force-reset shared refs. Ditch owns integration. Keep all task work inside the current working directory. Do not ask the user to manage Ditch branches or worktrees. If the prepared project appears incomplete, report that Ditch could not prepare the project instead of prescribing Git commands."
    )
}

fn create_journaled_worktree(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    agent_id: AgentId,
    prompt: &str,
) -> Result<git_orchestration::CreatedWorktree, git_orchestration::GitError> {
    let (git, root) = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .store
            .upsert_project(project)
            .map_err(|error| git_orchestration::GitError {
                code: "project_store_failed",
                message: error.to_string(),
            })?;
        (
            Arc::clone(&state.git),
            state.paths.data_dir.join("worktrees"),
        )
    };
    let result = git.create_journaled(project, agent_id, prompt, &root, |worktree| {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .store
            .upsert_worktree(worktree)
            .map_err(|error| git_orchestration::GitError {
                code: "worktree_store_failed",
                message: error.to_string(),
            })?;
        state.worktrees.insert(agent_id, worktree.clone());
        emit_git_event(
            &mut state,
            project.id,
            Some(agent_id),
            "worktree_create_started",
            "creating isolated locked workspace",
        );
        Ok(())
    });
    if let Err(error) = &result {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if let Some(mut worktree) = state.worktrees.get(&agent_id).cloned() {
            worktree.status = WorktreeStatus::Failed;
            worktree.operation = None;
            worktree.last_error = Some(error.message.clone());
            worktree.updated_at = Utc::now();
            let _ = state.store.upsert_worktree(&worktree);
            state.worktrees.insert(agent_id, worktree.clone());
            state.broadcast(ServerEvent::WorktreeChanged(Box::new(worktree)));
        }
    }
    result
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
    let state_handle = Arc::clone(state);
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    if let Some(run_id) = run_id
        && !is_current_run(&state, agent_id, run_id)
    {
        return;
    }
    state.children.remove(&agent_id);
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
        record.run.can_stop = false;
        if stopping {
            record.run.state = AgentState::Interrupted;
            record.run.last_visible_action = Some("Stopped by user".to_owned());
        } else if !interrupted {
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
        record.run.exit_code = code;
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
    state.broadcast(ServerEvent::AgentChanged(run.clone()));
    if let Some(attention) = attention {
        if let Err(error) = state.store.upsert_attention(&attention) {
            eprintln!("{RUNTIME_IDENTITY} failed to persist attention: {error}");
        }
        state.attention.push(attention.clone());
        state.broadcast(ServerEvent::AttentionRaised(attention));
    }
    let checkpoint = matches!(run.state, AgentState::Completed | AgentState::Interrupted)
        && state.worktrees.contains_key(&agent_id);
    drop(state);
    if checkpoint {
        checkpoint_finished_worktree(&state_handle, agent_id, run.state == AgentState::Completed);
    }
}

fn checkpoint_finished_worktree(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    prepare_when_complete: bool,
) {
    let (git, project, mut worktree) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(worktree) = state.worktrees.get(&agent_id).cloned() else {
            return;
        };
        let Some(project) = state
            .projects
            .values()
            .find(|item| item.id == worktree.project_id)
            .cloned()
        else {
            return;
        };
        (Arc::clone(&state.git), project, worktree)
    };
    let result = git.checkpoint(&project, &mut worktree);
    let mut locked = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    match result {
        Ok(_) => {
            worktree.status = WorktreeStatus::NeedsReview;
            worktree.integration_state = IntegrationState::NotRequested;
            worktree.last_error = None;
            emit_git_event(
                &mut locked,
                project.id,
                Some(agent_id),
                "worktree_checkpointed",
                "agent result preserved without changing its index",
            );
        }
        Err(error) => {
            worktree.status = WorktreeStatus::RecoveryNeeded;
            worktree.last_error = Some(error.message);
            emit_git_event(
                &mut locked,
                project.id,
                Some(agent_id),
                "worktree_recovery_needed",
                "agent result could not be checkpointed automatically",
            );
        }
    }
    let _ = locked.store.upsert_worktree(&worktree);
    locked.worktrees.insert(agent_id, worktree.clone());
    locked.broadcast(ServerEvent::WorktreeChanged(Box::new(worktree)));
    drop(locked);
    if prepare_when_complete {
        let _ = prepare_integration(Arc::clone(state), agent_id);
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

trait ProjectRootKey {
    fn root_key(&self) -> String;
}

impl ProjectRootKey for Project {
    fn root_key(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        RuntimeState::new(paths).expect("test runtime should initialize")
    }

    #[test]
    fn validated_completed_work_applies_automatically_by_default() {
        let mut runtime = test_runtime();
        let cleanup = runtime.paths.data_dir.clone();
        let repo = cleanup.join("auto-apply-project");
        fs::create_dir_all(repo.join(".ditch")).unwrap();
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        fs::write(repo.join("base.txt"), "base\n").unwrap();
        Command::new("git")
            .args(["add", "base.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let committed = Command::new("git")
            .args([
                "-c",
                "user.name=Ditch Test",
                "-c",
                "user.email=ditch-test@localhost",
                "commit",
                "-m",
                "baseline",
            ])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(committed.status.success());
        fs::write(
            repo.join(".ditch/validation.json"),
            r#"{"commands":[["sh","-c","exit 0"]]}"#,
        )
        .unwrap();
        let project = Project::new("Automatic", &repo);
        assert_eq!(
            project.integration_policy,
            ditch_core::IntegrationPolicy::AutoApplyAfterValidation
        );
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let agent_id = AgentId::new();
        let mut worktree = runtime
            .git
            .create(
                &project,
                agent_id,
                "automatic",
                &cleanup.join("managed-worktrees"),
            )
            .unwrap()
            .managed;
        fs::write(worktree.path.join("feature.txt"), "automatic\n").unwrap();
        runtime.git.checkpoint(&project, &mut worktree).unwrap();
        runtime.store.upsert_worktree(&worktree).unwrap();
        runtime.worktrees.insert(agent_id, worktree);
        let state = Arc::new(Mutex::new(runtime));

        let response = prepare_integration(Arc::clone(&state), agent_id);

        assert!(matches!(response, ServerResponse::Accepted));
        assert_eq!(
            fs::read_to_string(repo.join("feature.txt")).unwrap(),
            "automatic\n"
        );
        assert_eq!(
            state.lock().unwrap().worktrees[&agent_id].integration_state,
            IntegrationState::Applied
        );
        fs::remove_dir_all(cleanup).ok();
    }

    #[test]
    fn initialized_empty_project_is_immediately_ready_for_agents() {
        let runtime = test_runtime();
        let cleanup = runtime.paths.data_dir.clone();
        let project_root = cleanup.join("empty-project");
        fs::create_dir_all(&project_root).unwrap();
        let state = Arc::new(Mutex::new(runtime));

        let response = handle_request(
            ClientRequest::CreateProject {
                name: "Empty".into(),
                root: project_root.to_string_lossy().into_owned(),
                git_policy: ProjectGitPolicy::InitializeRepository,
            },
            Arc::clone(&state),
        );
        let ServerResponse::ProjectCreated(project) = response else {
            panic!("project creation should succeed");
        };
        assert!(
            git_orchestration::changed_paths(&project_root)
                .unwrap()
                .iter()
                .all(|path| path == ".ditch" || path.starts_with(".ditch/"))
        );
        assert!(
            Command::new("git")
                .args(["rev-parse", "--verify", "HEAD"])
                .current_dir(&project_root)
                .status()
                .unwrap()
                .success()
        );
        let readiness = inspect_project_agent_readiness(state, project.id);
        assert!(matches!(
            readiness,
            ServerResponse::ProjectAgentReadiness(ProjectAgentReadiness {
                state: ProjectAgentReadinessState::Ready,
                ..
            })
        ));
        fs::remove_dir_all(cleanup).ok();
    }

    #[test]
    fn launch_preparation_saves_files_added_after_an_empty_baseline() {
        let runtime = test_runtime();
        let cleanup = runtime.paths.data_dir.clone();
        let project_root = cleanup.join("later-populated-project");
        fs::create_dir_all(&project_root).unwrap();
        let state = Arc::new(Mutex::new(runtime));
        let response = handle_request(
            ClientRequest::CreateProject {
                name: "Later populated".into(),
                root: project_root.to_string_lossy().into_owned(),
                git_policy: ProjectGitPolicy::InitializeRepository,
            },
            Arc::clone(&state),
        );
        let ServerResponse::ProjectCreated(project) = response else {
            panic!("project creation should succeed");
        };
        let read_head = || {
            let output = Command::new("git")
                .args(["rev-parse", "--verify", "HEAD"])
                .current_dir(&project_root)
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let empty_head = read_head();
        fs::create_dir_all(project_root.join("src")).unwrap();
        fs::write(project_root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            project_root.join("Cargo.toml"),
            "[package]\nname='later-populated'\nversion='0.1.0'\n",
        )
        .unwrap();

        prepare_project_snapshot_for_launch(&state, &project).unwrap();

        let prepared_head = read_head();
        assert_ne!(prepared_head, empty_head);
        assert!(matches!(
            inspect_project_agent_readiness(Arc::clone(&state), project.id),
            ServerResponse::ProjectAgentReadiness(ProjectAgentReadiness {
                state: ProjectAgentReadinessState::Ready,
                ..
            })
        ));
        let created = state
            .lock()
            .unwrap()
            .git
            .create(
                &project,
                AgentId::new(),
                "Dockerize this app",
                &cleanup.join("managed-worktrees"),
            )
            .unwrap();
        assert!(created.agent_cwd.join("Cargo.toml").is_file());
        assert!(created.agent_cwd.join("src/main.rs").is_file());

        fs::remove_dir_all(cleanup).ok();
    }

    #[test]
    fn populated_unborn_project_prepares_without_creating_a_failed_agent() {
        let mut runtime = test_runtime();
        let cleanup = runtime.paths.data_dir.clone();
        let project_root = cleanup.join("populated-project");
        fs::create_dir_all(&project_root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(&project_root)
                .status()
                .unwrap()
                .success()
        );
        fs::write(project_root.join("app.txt"), "hello\n").unwrap();
        let project = Project::new("Populated", &project_root);
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let state = Arc::new(Mutex::new(runtime));

        let readiness = inspect_project_agent_readiness(Arc::clone(&state), project.id);
        let ServerResponse::ProjectAgentReadiness(readiness) = readiness else {
            panic!("readiness should be returned");
        };
        assert_eq!(
            readiness.state,
            ProjectAgentReadinessState::NeedsInitialSnapshot
        );
        assert_eq!(readiness.included_file_count, 1);
        let response = create_initial_project_snapshot(
            Arc::clone(&state),
            project.id,
            readiness.snapshot_tree_oid.as_deref().unwrap(),
        );
        assert!(matches!(
            response,
            ServerResponse::InitialProjectSnapshotCreated(_)
        ));
        assert!(
            state
                .lock()
                .expect("runtime state lock should not be poisoned")
                .agents
                .is_empty()
        );
        fs::remove_dir_all(cleanup).ok();
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
}
