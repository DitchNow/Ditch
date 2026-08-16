use chrono::{DateTime, Utc};
use ditch_core::{
    AgentId, AgentRun, AppPaths, CodexLaunchMode, PermissionRequest, Project, ProjectId, Task,
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
    Snapshot,
    ListProjects,
    CreateProject {
        name: String,
        root: String,
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
    pub attention: Vec<PermissionRequest>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerEvent {
    ProjectChanged(Project),
    TaskChanged(Task),
    AgentChanged(AgentRun),
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
