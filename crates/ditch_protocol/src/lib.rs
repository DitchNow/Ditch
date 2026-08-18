use chrono::{DateTime, Utc};
use ditch_core::{
    AgentId, AgentRun, AppPaths, AttentionKind, CodexLaunchMode, PermissionRequest, Project,
    ProjectGitPolicy, ProjectId, Task,
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
    },
    ResumeCodexSession {
        project_name: String,
        project_root: String,
        thread_id: String,
        prompt: String,
    },
    StartCodex {
        project_id: ProjectId,
        prompt: String,
        mode: CodexLaunchMode,
    },
    PromptAgent {
        agent_id: AgentId,
        prompt: String,
    },
    StopAgent {
        agent_id: AgentId,
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
    ProjectCreated(Project),
    AgentStarted(AgentRun),
    Accepted,
    Error(ProtocolError),
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
pub struct RuntimeAttention {
    pub id: Uuid,
    pub kind: AttentionKind,
    pub agent_id: Option<AgentId>,
    #[serde(default)]
    pub project_id: Option<ProjectId>,
    pub title: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerEvent {
    SnapshotReplaced(Snapshot),
    RuntimeStatusChanged(RuntimeStatus),
    ProjectChanged(Project),
    TaskChanged(Task),
    AgentChanged(AgentRun),
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}
