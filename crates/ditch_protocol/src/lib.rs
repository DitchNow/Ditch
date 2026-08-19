use chrono::{DateTime, Utc};
use ditch_core::{
    AgentExecutionProfile, AgentId, AgentRun, AppPaths, AttentionKind, CodexLaunchMode,
    PermissionRequest, Project, ProjectGitPolicy, ProjectId, Task,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub protocol_version: u16,
    pub id: Uuid,
    pub sent_at: DateTime<Utc>,
    pub body: T,
}

impl<T> Envelope<T> {
    pub fn new(body: T) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            id: Uuid::new_v4(),
            sent_at: Utc::now(),
            body,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClientRequest {
    Health,
    RuntimeStatus,
    Snapshot,
    Shutdown,
    SubscribeEvents {
        since_sequence: u64,
    },
    SubscribeAttention,
    ListAgentMessages {
        agent_id: AgentId,
        before_sequence: Option<u64>,
        limit: u16,
    },
    ListProjects,
    DiscoverProjects {
        search_root: String,
    },
    CreateProject {
        name: String,
        root: String,
        git_policy: ProjectGitPolicy,
    },
    StartCodexSession {
        project_name: String,
        project_root: String,
        prompt: String,
        mode: CodexLaunchMode,
        #[serde(default)]
        execution_profile: AgentExecutionProfile,
    },
    ResumeCodexSession {
        project_name: String,
        project_root: String,
        thread_id: String,
        prompt: String,
        #[serde(default)]
        execution_profile: AgentExecutionProfile,
    },
    StartCodex {
        project_id: ProjectId,
        prompt: String,
        mode: CodexLaunchMode,
    },
    PromptAgent {
        agent_id: AgentId,
        prompt: String,
        #[serde(default)]
        execution_profile: AgentExecutionProfile,
    },
    ListAgentModels {
        provider: ditch_core::AgentProvider,
    },
    OpenProjectTerminal {
        project_id: ProjectId,
        columns: u16,
        rows: u16,
    },
    WriteProjectTerminal {
        terminal_id: Uuid,
        data: Vec<u8>,
    },
    ResizeProjectTerminal {
        terminal_id: Uuid,
        columns: u16,
        rows: u16,
    },
    CloseProjectTerminal {
        terminal_id: Uuid,
    },
    ListProjectDirectory {
        project_id: ProjectId,
        relative_path: String,
    },
    ReadProjectFile {
        project_id: ProjectId,
        relative_path: String,
    },
    WriteProjectFile {
        project_id: ProjectId,
        relative_path: String,
        expected_revision: Option<String>,
        content: String,
    },
    StopAgent {
        agent_id: AgentId,
    },
    DeleteAgent {
        agent_id: AgentId,
    },
    RenameAgent {
        agent_id: AgentId,
        title: Option<String>,
    },
    DismissAttention {
        attention_id: Uuid,
    },
    ApprovePermission {
        request_id: Uuid,
    },
    DenyPermission {
        request_id: Uuid,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerResponse {
    Health(HealthResponse),
    RuntimeStatus(RuntimeStatus),
    Snapshot(Snapshot),
    Projects(Vec<Project>),
    AgentModels(Vec<AgentModel>),
    ProjectTerminal(ProjectTerminal),
    ProjectDirectory(ProjectDirectory),
    ProjectFile(ProjectFile),
    ProjectFileSaved(ProjectFileSaved),
    ProjectCreated(Project),
    AgentStarted(AgentRun),
    AgentMessages(AgentMessagePage),
    Accepted,
    Error(ProtocolError),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentModel {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub default_reasoning_effort: Option<String>,
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<String>,
    /// The maximum prompt context accepted by this model, when the provider
    /// publishes it.  It is deliberately optional: model discovery is the
    /// authority and Ditch must not invent a limit when it has none.
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectTerminal {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub shell: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProjectFileKind {
    Directory,
    File,
    Symlink,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectFileEntry {
    pub name: String,
    pub relative_path: String,
    pub kind: ProjectFileKind,
    pub size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectDirectory {
    pub project_id: ProjectId,
    pub relative_path: String,
    pub entries: Vec<ProjectFileEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    pub project_id: ProjectId,
    pub relative_path: String,
    pub content: String,
    pub revision: String,
    pub size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectFileSaved {
    pub project_id: ProjectId,
    pub relative_path: String,
    pub revision: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub version: String,
    pub app_paths: AppPaths,
    pub codex_binary: Option<String>,
    pub claude_binary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub projects: Vec<Project>,
    pub tasks: Vec<Task>,
    pub agents: Vec<AgentRun>,
    pub attention: Vec<RuntimeAttention>,
    pub messages: Vec<AgentChatMessage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub identity: String,
    pub pid: u32,
    pub socket_path: String,
    pub active_session_count: usize,
    #[serde(default)]
    pub attention_count: usize,
    #[serde(default)]
    pub instance_id: Uuid,
    #[serde(default)]
    pub codex_home: Option<String>,
    #[serde(default)]
    pub codex_binary: Option<String>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub build_version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AgentChatRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentChatMessage {
    pub agent_id: AgentId,
    pub role: AgentChatRole,
    pub text: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SequencedAgentMessage {
    pub sequence: u64,
    pub message: AgentChatMessage,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentMessagePage {
    pub agent_id: AgentId,
    pub messages: Vec<SequencedAgentMessage>,
    pub next_before_sequence: Option<u64>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeAttention {
    pub id: Uuid,
    pub kind: AttentionKind,
    pub agent_id: Option<AgentId>,
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub agent_name: Option<String>,
    pub title: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerEvent {
    SnapshotReplaced(Snapshot),
    AttentionSnapshotReplaced(Vec<RuntimeAttention>),
    RuntimeStatusChanged(RuntimeStatus),
    ProjectChanged(Project),
    TaskChanged(Task),
    AgentChanged(AgentRun),
    AgentDeleted {
        agent_id: AgentId,
    },
    AgentMessageAppended(AgentChatMessage),
    AttentionRaised(RuntimeAttention),
    AttentionDismissed {
        attention_id: Uuid,
    },
    PermissionRequested(PermissionRequest),
    Bell {
        agent_id: Option<AgentId>,
        reason: String,
    },
    ScrollbackAppended {
        agent_id: AgentId,
        bytes: usize,
    },
    ProjectTerminalOutput {
        terminal_id: Uuid,
        /// Raw PTY bytes. JSON serializes this as an array, preserving ANSI
        /// control sequences and avoiding lossy text conversion in the daemon.
        data: Vec<u8>,
    },
    ProjectTerminalExited {
        terminal_id: Uuid,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}
