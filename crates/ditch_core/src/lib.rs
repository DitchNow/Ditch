use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AttemptId(pub Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub Uuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RuntimePaneId(pub Uuid);

macro_rules! id_newtype {
    ($name:ident) => {
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

id_newtype!(ProjectId);
id_newtype!(TaskId);
id_newtype!(AttemptId);
id_newtype!(AgentId);
id_newtype!(RuntimePaneId);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskState {
    Draft,
    Ready,
    Running,
    Blocked,
    InReview,
    Accepted,
    Rejected,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AgentState {
    Starting,
    Working,
    AwaitingApproval,
    Blocked,
    Idle,
    Completed,
    Failed,
    Stale,
    Interrupted,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AgentResumeBlockReason {
    NoCodexThread,
    CodexHomeMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AttentionKind {
    ApprovalRequired,
    Blocked,
    Completed,
    Failed,
    Stale,
    NoOutput,
    UnknownState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PermissionActionKind {
    EditFiles,
    RunCommand,
    InstallDependencies,
    ModifyGitState,
    AccessNetwork,
    ManageCodexConfig,
    ManageProjectTools,
    StopProcessGroup,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AgentProvider {
    Codex,
    Claude,
    Shell,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CodexLaunchMode {
    InteractiveTui,
    Exec,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ProjectGitPolicy {
    RequireRepository,
    InitializeRepository,
    AllowOutsideGit,
}

impl Default for ProjectGitPolicy {
    fn default() -> Self {
        Self::RequireRepository
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: PathBuf,
    pub created_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub git_policy: ProjectGitPolicy,
}

impl Project {
    pub fn new(name: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        Self {
            id: ProjectId::new(),
            name: name.into(),
            root: root.into(),
            created_at: Utc::now(),
            archived_at: None,
            git_policy: ProjectGitPolicy::RequireRepository,
        }
    }

    pub fn ditch_dir(&self) -> PathBuf {
        self.root.join(".ditch")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub project_id: ProjectId,
    pub title: String,
    pub description: String,
    pub state: TaskState,
    pub acceptance_criteria: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: AgentId,
    pub provider: AgentProvider,
    pub state: AgentState,
    pub launch_mode: CodexLaunchMode,
    pub project_id: ProjectId,
    pub task_id: Option<TaskId>,
    pub pane_id: Option<RuntimePaneId>,
    pub native_session_id: Option<String>,
    #[serde(default)]
    pub origin_codex_home: Option<String>,
    pub current_prompt: Option<String>,
    pub last_visible_action: Option<String>,
    pub state_confidence: f32,
    pub state_evidence: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub resume_block_reason: Option<AgentResumeBlockReason>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub agent_id: Option<AgentId>,
    pub action: PermissionActionKind,
    pub summary: String,
    pub target: String,
    pub command: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub database_path: PathBuf,
    pub socket_path: PathBuf,
    pub logs_dir: PathBuf,
    pub scrollback_dir: PathBuf,
}

impl AppPaths {
    pub fn for_current_user() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let data_dir = home
            .join("Library")
            .join("Application Support")
            .join("The Ditch");

        Self {
            database_path: data_dir.join("ditch.sqlite3"),
            socket_path: data_dir.join("ditchd.sock"),
            logs_dir: data_dir.join("logs"),
            scrollback_dir: data_dir.join("scrollback"),
            data_dir,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProductPolicy {
    pub persist_scrollback: bool,
    pub default_codex_mode: CodexLaunchMode,
    pub allow_project_worktrees: bool,
    pub global_codex_config_requires_explicit_approval: bool,
}

impl Default for ProductPolicy {
    fn default() -> Self {
        Self {
            persist_scrollback: true,
            default_codex_mode: CodexLaunchMode::InteractiveTui,
            allow_project_worktrees: true,
            global_codex_config_requires_explicit_approval: true,
        }
    }
}
