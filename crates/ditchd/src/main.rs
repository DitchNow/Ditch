use chrono::{DateTime, Utc};
use ditch_core::{
    AgentId, AgentProvider, AgentResumeBlockReason, AgentRun, AgentState, AppPaths, AttentionKind,
    CodexLaunchMode, Project, ProjectGitPolicy,
};
use ditch_protocol::{
    AgentChatMessage, AgentChatRole, ClientRequest, Envelope, HealthResponse, ProtocolError,
    RuntimeAttention, RuntimeStatus, ServerEvent, ServerResponse, Snapshot,
};
use ditch_store::{
    DitchStore, discover_legacy_projects, ensure_app_dirs, ensure_project_metadata,
    load_project_config, save_project_config,
};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

const RUNTIME_IDENTITY: &str = "The Ditch Runtime";

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
    children: HashMap<AgentId, Arc<Mutex<Child>>>,
    subscribers: Vec<Sender<String>>,
    next_sequence: u64,
    instance_id: uuid::Uuid,
    attention: Vec<RuntimeAttention>,
    started_at: DateTime<Utc>,
    codex_home: Option<PathBuf>,
    codex_binary: Option<String>,
}

struct AgentRecord {
    run: AgentRun,
    project_root: PathBuf,
    allow_non_git: bool,
    messages: Vec<AgentChatMessage>,
    terminal_failure: Option<String>,
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
        let agents = durable
            .agents
            .into_iter()
            .filter_map(|agent| {
                let project = projects
                    .values()
                    .find(|project| project.id == agent.run.project_id)?;
                Some((
                    agent.run.id,
                    AgentRecord {
                        run: agent.run,
                        project_root: project.root.clone(),
                        allow_non_git: project.git_policy == ProjectGitPolicy::AllowOutsideGit,
                        messages: agent.messages,
                        terminal_failure: agent.terminal_failure,
                    },
                ))
            })
            .collect();
        let codex_home = std::env::var_os("CODEX_HOME").map(PathBuf::from);
        let codex_binary = find_binary("codex");
        Ok(Self {
            paths,
            store,
            projects,
            agents,
            children: HashMap::new(),
            subscribers: Vec::new(),
            next_sequence: 1,
            instance_id: uuid::Uuid::new_v4(),
            attention: durable.attention,
            started_at: Utc::now(),
            codex_home,
            codex_binary,
        })
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
                            | AgentState::AwaitingApproval
                            | AgentState::Blocked
                    )
                })
                .count(),
            attention_count: self.attention.len(),
            instance_id: self.instance_id,
            codex_home: self
                .codex_home
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            codex_binary: self.codex_binary.clone(),
            started_at: Some(self.started_at),
            build_version: env!("CARGO_PKG_VERSION").to_owned(),
            capabilities: vec!["persistent_sessions_v1".to_owned()],
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
            messages: self
                .agents
                .values()
                .flat_map(|record| record.messages.clone())
                .collect(),
        }
    }

    fn broadcast(&mut self, event: ServerEvent) {
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
                    "The Ditch Runtime is already running",
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

fn handle_request(request: ClientRequest, state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    match request {
        ClientRequest::Health => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            ServerResponse::Health(HealthResponse {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                app_paths: state.paths.clone(),
                codex_binary: find_binary("codex"),
                claude_binary: find_binary("claude"),
            })
        }
        ClientRequest::RuntimeStatus => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            ServerResponse::RuntimeStatus(state.runtime_status())
        }
        ClientRequest::Snapshot => {
            let state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            ServerResponse::Snapshot(state.snapshot())
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
        ClientRequest::StartCodexSession {
            project_name,
            project_root,
            prompt,
            mode,
        } => start_codex_session(state, project_name, project_root, prompt, mode),
        ClientRequest::ResumeCodexSession {
            project_name,
            project_root,
            thread_id,
            prompt,
        } => resume_codex_session(state, project_name, project_root, thread_id, prompt),
        ClientRequest::PromptAgent { agent_id, prompt } => prompt_agent(state, agent_id, prompt),
        ClientRequest::StopAgent { agent_id } => stop_agent(state, agent_id),
        ClientRequest::DeleteAgent { agent_id } => delete_agent(state, agent_id),
        ClientRequest::RenameAgent { agent_id, title } => rename_agent(state, agent_id, title),
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
        ClientRequest::StartCodex { .. }
        | ClientRequest::ApprovePermission { .. }
        | ClientRequest::DenyPermission { .. }
        | ClientRequest::SubscribeEvents { .. } => protocol_error(
            "unsupported_request",
            "request is not implemented by this runtime",
        ),
    }
}

fn shutdown_runtime(state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    let socket_path = {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let children = std::mem::take(&mut state.children);
        for child in children.values() {
            let _ = child
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

fn start_codex_session(
    state: Arc<Mutex<RuntimeState>>,
    project_name: String,
    project_root: String,
    prompt: String,
    mode: CodexLaunchMode,
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
        launch_mode: mode.clone(),
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
        state_evidence: "The Ditch Runtime accepted the session and is launching Codex.".to_owned(),
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
    let Some(binary) = find_binary("codex") else {
        return persist_launch_failure(
            &state,
            project,
            run,
            user_message,
            allow_non_git,
            "Codex binary was not found on PATH".to_owned(),
        );
    };
    let child = match spawn_codex_child(&binary, &project.root, &prompt, &mode, None, allow_non_git)
    {
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

    run.state = AgentState::Working;
    run.updated_at = Utc::now();
    run.state_evidence = "Codex process is running under The Ditch Runtime.".to_owned();

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
        state.children.insert(run.id, Arc::clone(&child));
        state.broadcast(ServerEvent::ProjectChanged(project));
        state.broadcast(ServerEvent::AgentChanged(run.clone()));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), run.id, child);
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
        title: "Codex failed to start".to_owned(),
        body: error.clone(),
        created_at: Utc::now(),
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
        launch_mode: CodexLaunchMode::Exec,
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
        state_evidence: "The Ditch Runtime accepted the session and is resuming Codex.".to_owned(),
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
    let Some(binary) = find_binary("codex") else {
        return persist_launch_failure(
            &state,
            project,
            run,
            user_message,
            allow_non_git,
            "Codex binary was not found on PATH".to_owned(),
        );
    };
    let child = match spawn_codex_child(
        &binary,
        &project.root,
        &prompt,
        &CodexLaunchMode::Exec,
        Some(&thread_id),
        allow_non_git,
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

    run.state = AgentState::Working;
    run.updated_at = Utc::now();
    run.state_evidence = "Codex resume process is running under The Ditch Runtime.".to_owned();

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
        state.children.insert(run.id, Arc::clone(&child));
        state.broadcast(ServerEvent::ProjectChanged(project));
        state.broadcast(ServerEvent::AgentChanged(run.clone()));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), run.id, child);
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
) -> ServerResponse {
    let (project_root, thread_id, allow_non_git) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(record) = state.agents.get(&agent_id) else {
            return protocol_error("agent_not_found", "agent session was not found");
        };
        if matches!(record.run.state, AgentState::Starting | AgentState::Working) {
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

    let Some(binary) = find_binary("codex") else {
        let message = "Codex binary was not found on PATH".to_owned();
        record_terminal_failure(&state, agent_id, message.clone());
        finish_agent(&state, agent_id, Some(1));
        return protocol_error("codex_not_found", message);
    };

    let child = match spawn_codex_child(
        &binary,
        &project_root,
        &prompt,
        &CodexLaunchMode::Exec,
        thread_id.as_deref(),
        allow_non_git,
    ) {
        Ok(child) => Arc::new(Mutex::new(child)),
        Err(error) => {
            let message = error.to_string();
            record_terminal_failure(&state, agent_id, message.clone());
            finish_agent(&state, agent_id, Some(1));
            return protocol_error("codex_start_failed", message);
        }
    };

    let user_message = AgentChatMessage {
        agent_id,
        role: AgentChatRole::User,
        text: prompt.clone(),
        created_at: Utc::now(),
    };

    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(record) = state.agents.get_mut(&agent_id) else {
            return protocol_error("agent_not_found", "agent session was not found");
        };
        record.run.state = AgentState::Working;
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
        state.children.insert(agent_id, Arc::clone(&child));
        state.broadcast(ServerEvent::AgentChanged(run));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }

    attach_codex_io(Arc::clone(&state), agent_id, child);
    ServerResponse::Accepted
}

fn stop_agent(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let child = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state.children.get(&agent_id).cloned()
    };

    if let Some(child) = child {
        let _ = child
            .lock()
            .expect("child lock should not be poisoned")
            .kill();
    }

    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state.children.remove(&agent_id);
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return protocol_error("agent_not_found", "agent session was not found");
    };
    record.run.state = AgentState::Interrupted;
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
    ServerResponse::Accepted
}

fn delete_agent(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId) -> ServerResponse {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(record) = state.agents.get(&agent_id) else {
        return protocol_error("agent_not_found", "agent session was not found");
    };
    if matches!(record.run.state, AgentState::Starting | AgentState::Working)
        || state.children.contains_key(&agent_id)
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

fn attach_codex_io(state: Arc<Mutex<RuntimeState>>, agent_id: AgentId, child: Arc<Mutex<Child>>) {
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
                    Ok(line) => handle_codex_stdout_line(&state, agent_id, &line),
                    Err(error) => {
                        record_terminal_failure(
                            &state,
                            agent_id,
                            format!("Failed to read Codex output: {error}"),
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
                            if is_workspace_permission_denial(text) {
                                record_terminal_failure(&state, agent_id, text.to_owned());
                            } else {
                                append_message(
                                    &state,
                                    agent_id,
                                    AgentChatRole::System,
                                    text.to_owned(),
                                );
                            }
                        }
                    }
                    Err(error) => {
                        record_terminal_failure(
                            &state,
                            agent_id,
                            format!("Failed to read Codex diagnostics: {error}"),
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
        let code = child
            .lock()
            .expect("child lock should not be poisoned")
            .wait()
            .ok()
            .and_then(|status| status.code());
        if let Some(reader) = stdout_thread {
            let _ = reader.join();
        }
        if let Some(reader) = stderr_thread {
            let _ = reader.join();
        }
        finish_agent(&state, agent_id, code);
    });
}

fn handle_codex_stdout_line(state: &Arc<Mutex<RuntimeState>>, agent_id: AgentId, line: &str) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        let text = line.trim();
        if looks_like_diagnostic(text) {
            append_message(state, agent_id, AgentChatRole::System, text.to_owned());
        }
        return;
    };
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
        return;
    };

    if let Some(message) = find_workspace_permission_denial(&value) {
        record_terminal_failure(state, agent_id, message);
    }

    match event_type {
        "turn.started" => update_agent_action(state, agent_id, "Codex is working"),
        "thread.started" => {
            if let Some(thread_id) = value.get("thread_id").and_then(Value::as_str) {
                let codex_title =
                    codex_session_title(&value).or_else(|| codex_title_from_state(thread_id));
                let mut state = state
                    .lock()
                    .expect("runtime state lock should not be poisoned");
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
            update_agent_action(state, agent_id, action);
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
                        );
                    }
                }
                Some("error") => {
                    if let Some(message) = extract_diagnostic_message(&value) {
                        record_terminal_failure(state, agent_id, message);
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
            let Some(record) = state.agents.get_mut(&agent_id) else {
                return;
            };
            record.run.state = AgentState::Completed;
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
                append_message(state, agent_id, AgentChatRole::System, message);
            } else {
                record_terminal_failure(state, agent_id, message);
            }
        }
        _ => {
            if let Some(message) = extract_explicit_error(&value) {
                record_terminal_failure(state, agent_id, message);
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

fn record_terminal_failure(state: &Arc<Mutex<RuntimeState>>, agent_id: AgentId, message: String) {
    let message = message.trim().to_owned();
    if message.is_empty() {
        return;
    }
    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(record) = state.agents.get_mut(&agent_id) else {
            return;
        };
        record.terminal_failure = Some(message.clone());
    }
    append_message(state, agent_id, AgentChatRole::System, message.clone());
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

fn find_workspace_permission_denial(value: &Value) -> Option<String> {
    match value {
        Value::String(message) if is_workspace_permission_denial(message) => Some(message.clone()),
        Value::Array(items) => items.iter().find_map(find_workspace_permission_denial),
        Value::Object(items) => items.values().find_map(find_workspace_permission_denial),
        _ => None,
    }
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
    let attention = RuntimeAttention {
        id: uuid::Uuid::new_v4(),
        kind: AttentionKind::Blocked,
        agent_id: Some(agent_id),
        project_id,
        title: "Codex cannot write this project".to_owned(),
        body,
        created_at: Utc::now(),
    };
    if let Err(error) = state.store.upsert_attention(&attention) {
        eprintln!("{RUNTIME_IDENTITY} failed to persist attention: {error}");
    }
    state.attention.push(attention.clone());
    state.broadcast(ServerEvent::AttentionRaised(attention));
}

fn update_agent_action(state: &Arc<Mutex<RuntimeState>>, agent_id: AgentId, action: &str) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return;
    };
    record.run.last_visible_action = Some(action.to_owned());
    record.run.updated_at = Utc::now();
    let run = record.run.clone();
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentChanged(run));
}

fn append_message(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    role: AgentChatRole,
    text: String,
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
    let Some(record) = state.agents.get_mut(&agent_id) else {
        return;
    };
    record.messages.push(message.clone());
    record.run.updated_at = Utc::now();
    state.persist_message(&message);
    state.persist_agent(agent_id);
    state.broadcast(ServerEvent::AgentMessageAppended(message));
}

fn finish_agent(state: &Arc<Mutex<RuntimeState>>, agent_id: AgentId, code: Option<i32>) {
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state.children.remove(&agent_id);
    let (run, attention) = {
        let Some(record) = state.agents.get_mut(&agent_id) else {
            return;
        };
        if !matches!(record.run.state, AgentState::Interrupted) {
            record.run.state = match code {
                Some(0) => AgentState::Completed,
                _ => AgentState::Failed,
            };
        }
        record.run.last_visible_action = Some(match code {
            Some(code) => format!("Codex exited with code {code}"),
            None => "Codex exited without a code".to_owned(),
        });
        record.run.updated_at = Utc::now();
        record.run.finished_at = Some(record.run.updated_at);
        record.run.exit_code = code;
        record.run.resume_block_reason = record
            .run
            .native_session_id
            .is_none()
            .then_some(AgentResumeBlockReason::NoCodexThread);
        let attention = if record.run.state == AgentState::Failed {
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
                        .unwrap_or_else(|| "Codex failed".to_owned())
                });
            Some(RuntimeAttention {
                id: uuid::Uuid::new_v4(),
                kind: AttentionKind::Failed,
                agent_id: Some(agent_id),
                project_id: Some(record.run.project_id),
                title: "Codex run failed".to_owned(),
                body,
                created_at: Utc::now(),
            })
        } else {
            None
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

fn spawn_codex_child(
    binary: &str,
    cwd: &Path,
    prompt: &str,
    mode: &CodexLaunchMode,
    resume_thread: Option<&str>,
    allow_non_git: bool,
) -> io::Result<Child> {
    let mut command = Command::new(binary);
    command.args(codex_child_args(cwd, mode, resume_thread, allow_non_git));
    command.current_dir(cwd);
    command.env("TERM", "xterm-256color");
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
) -> Vec<String> {
    let mut args = match (resume_thread, mode) {
        (Some(_), _) => vec!["exec".to_owned(), "--approve-for-me".to_owned()],
        (None, CodexLaunchMode::Exec) | (None, CodexLaunchMode::InteractiveTui) => vec![
            "exec".to_owned(),
            "--approve-for-me".to_owned(),
            "--json".to_owned(),
            "--color".to_owned(),
            "never".to_owned(),
            "--cd".to_owned(),
            cwd.to_string_lossy().into_owned(),
            "-".to_owned(),
        ],
    };
    if allow_non_git {
        args.insert(1, "--skip-git-repo-check".to_owned());
    }
    if let Some(thread_id) = resume_thread {
        args.extend([
            "resume".to_owned(),
            "--json".to_owned(),
            thread_id.to_owned(),
            "-".to_owned(),
        ]);
    }
    args
}

fn canonical_project_root(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
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
        );

        assert_eq!(
            args,
            vec![
                "exec",
                "--approve-for-me",
                "--json",
                "--color",
                "never",
                "--cd",
                "/tmp/project",
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
        );

        assert_eq!(
            args,
            vec![
                "exec",
                "--approve-for-me",
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
        );
        let resumed = codex_child_args(
            Path::new("/tmp/project"),
            &CodexLaunchMode::Exec,
            Some("thread-123"),
            true,
        );

        assert_eq!(fresh[1], "--skip-git-repo-check");
        assert_eq!(resumed[1], "--skip-git-repo-check");
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
                    launch_mode: CodexLaunchMode::Exec,
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

        finish_agent(&state, agent_id, Some(1));

        let response = prompt_agent(Arc::clone(&state), agent_id, "retry".to_owned());
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
                    launch_mode: CodexLaunchMode::Exec,
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
            r#"{"type":"task_complete","payload":{"error":{"message":"You've hit your usage limit.","code":"usage_limit_exceeded"}}}"#,
        );
        finish_agent(&state, agent_id, Some(1));

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
                    launch_mode: CodexLaunchMode::Exec,
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
}
