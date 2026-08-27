use chrono::{DateTime, Utc};
use ditch_core::{
    AgentExecutionProfile, AgentId, AgentRun, AppPaths, AttentionKind, CodexLaunchMode,
    PermissionRequest, Project, ProjectGitPolicy, ProjectId, Task,
};
use ditch_ssh::{NewSshHost, ResolvedSshHost, SshHostSummary};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const REMOTE_RUNTIME_PROTOCOL_VERSION: u16 = 1;

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
    DiscoverSshHosts,
    ResolveSshHost {
        alias: String,
    },
    PreviewSshHost {
        host: NewSshHost,
    },
    AddSshHost {
        host: NewSshHost,
    },
    CheckRemoteSetup {
        alias: String,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        remember_password: bool,
        #[serde(default)]
        trust_unknown_host: bool,
    },
    InstallRemoteRuntime {
        alias: String,
    },
    InstallRemoteCodex {
        alias: String,
    },
    InstallRemoteGit {
        alias: String,
    },
    OpenRemoteCodexAuthentication {
        alias: String,
        columns: u16,
        rows: u16,
    },
    /// Internal remote-daemon command. It can only launch the fixed Codex
    /// login flow and is not a generic PTY/shell launcher.
    OpenCodexAuthentication {
        columns: u16,
        rows: u16,
    },
    WriteSetupTerminal {
        terminal_id: Uuid,
        data: Vec<u8>,
    },
    ResizeSetupTerminal {
        terminal_id: Uuid,
        columns: u16,
        rows: u16,
    },
    TakeSetupTerminalOutput {
        terminal_id: Uuid,
    },
    CloseSetupTerminal {
        terminal_id: Uuid,
    },
    ListRemoteDirectory {
        alias: String,
        absolute_path: String,
    },
    /// Bootstrap-only typed query handled by a remote daemon. This is not a
    /// generic shell or file mutation API.
    ListFilesystemDirectory {
        absolute_path: String,
    },
    CreateRemoteProject {
        ssh_host_alias: String,
        name: String,
        remote_root: String,
        git_policy: ProjectGitPolicy,
    },
    CheckRemoteProject {
        project_id: ProjectId,
    },
    DeleteProject {
        project_id: ProjectId,
    },
    StartCodexSession {
        #[serde(default)]
        project_id: Option<ProjectId>,
        project_name: String,
        project_root: String,
        prompt: String,
        mode: CodexLaunchMode,
        #[serde(default)]
        execution_profile: AgentExecutionProfile,
    },
    ResumeCodexSession {
        #[serde(default)]
        project_id: Option<ProjectId>,
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
        #[serde(default)]
        project_id: Option<ProjectId>,
    },
    DiscoverCodexInstallations,
    CheckCodexReadiness,
    UpdateSelectedCodex,
    SelectCodexBinary {
        path: String,
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
    ForceKillAgent {
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
    MarkAttentionRead {
        attention_ids: Vec<Uuid>,
    },
    MarkAllAttentionRead,
    RemoteControlStatus,
    EnsureRemoteMachineIdentity,
    RemoteMachineControlStatus {
        alias: String,
    },
    CreateRemoteMachinePairing {
        alias: String,
    },
    GetRemoteMachinePairing {
        alias: String,
        pairing_id: Uuid,
    },
    ConfirmRemoteMachinePairing {
        alias: String,
        pairing_id: Uuid,
    },
    CancelRemoteMachinePairing {
        alias: String,
        pairing_id: Uuid,
    },
    CreateRemotePairing,
    GetRemotePairing {
        pairing_id: Uuid,
    },
    ConfirmRemotePairing {
        pairing_id: Uuid,
    },
    CancelRemotePairing {
        pairing_id: Uuid,
    },
    RevokeRemoteDevice {
        device_id: Uuid,
    },
    DisableRemoteControl,
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
    SshHosts(Vec<SshHostSummary>),
    ResolvedSshHost(ResolvedSshHost),
    SshConfigPreview(String),
    RemoteSetup(RemoteSetupStatus),
    RemoteDirectory(RemoteDirectory),
    AgentModels(Vec<AgentModel>),
    CodexInstallations(Vec<CodexInstallation>),
    CodexReadiness(CodexReadiness),
    ProjectTerminal(ProjectTerminal),
    SetupTerminal(SetupTerminal),
    SetupTerminalOutput(SetupTerminalOutput),
    ProjectDirectory(ProjectDirectory),
    ProjectFile(ProjectFile),
    ProjectFileSaved(ProjectFileSaved),
    ProjectCreated(Project),
    AgentStarted(AgentRun),
    AgentMessages(AgentMessagePage),
    RemoteControlStatus(RemoteControlStatus),
    RemotePairing(RemotePairing),
    Accepted,
    Error(ProtocolError),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodexInstallation {
    pub path: String,
    pub version: String,
    pub selected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodexReadiness {
    pub path: Option<String>,
    pub version: Option<String>,
    pub compatible: bool,
    pub authenticated: bool,
    pub update_supported: bool,
    pub doctor_supported: bool,
    pub issues: Vec<String>,
    pub diagnostics: Option<String>,
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
pub struct SetupTerminal {
    pub id: Uuid,
    pub purpose: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetupTerminalOutput {
    pub terminal_id: Uuid,
    pub data: Vec<u8>,
    pub exited: bool,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCheckState {
    Checking,
    Ready,
    Missing,
    AuthenticationRequired,
    InstallAvailable,
    ManualActionRequired,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteSetupCheck {
    pub key: String,
    pub label: String,
    pub state: RemoteCheckState,
    pub detail: String,
    #[serde(default)]
    pub technical_detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteRuntimeHandshake {
    pub client_version: String,
    pub server_version: Option<String>,
    pub protocol_version: u16,
    pub build_identifier: Option<String>,
    pub daemon_epoch: Option<Uuid>,
    pub os: Option<String>,
    pub architecture: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteSetupStatus {
    pub ssh_host_alias: String,
    pub remote_machine_id: Option<Uuid>,
    pub ready: bool,
    pub connection_state: String,
    pub home_directory: Option<String>,
    pub checks: Vec<RemoteSetupCheck>,
    pub handshake: RemoteRuntimeHandshake,
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteDirectoryEntry {
    pub name: String,
    pub absolute_path: String,
    pub is_directory: bool,
    pub is_symlink: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteDirectory {
    pub ssh_host_alias: String,
    pub absolute_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<RemoteDirectoryEntry>,
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
    pub unread_attention_count: usize,
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
    #[serde(default)]
    pub protocol_version: u16,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub architecture: String,
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
    #[serde(default)]
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerEvent {
    SnapshotReplaced(Snapshot),
    AttentionSnapshotReplaced(Vec<RuntimeAttention>),
    RuntimeStatusChanged(RuntimeStatus),
    RemoteHostStatusChanged(RemoteHostPresence),
    ProjectChanged(Project),
    ProjectDeleted {
        project_id: ProjectId,
    },
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
    AttentionRead {
        attention_ids: Vec<Uuid>,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteHostPresence {
    pub ssh_host_alias: String,
    pub state: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteDeviceSummary {
    pub device_id: Uuid,
    pub name: String,
    pub state: String,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub currently_connected: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteControlStatus {
    pub configured: bool,
    pub enabled: bool,
    pub machine_id: Option<Uuid>,
    pub owner_id: Option<Uuid>,
    pub machine_name: String,
    /// Online means the daemon has a currently authenticated relay socket, not
    /// merely that D1 contains an old last-seen timestamp.
    pub online: bool,
    pub relay_origin: Option<String>,
    pub devices: Vec<RemoteDeviceSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemotePairing {
    pub pairing_id: Uuid,
    pub machine_id: Uuid,
    pub state: String,
    pub expires_at: DateTime<Utc>,
    /// Present only on initial creation so Flutter can render it as a QR. It is
    /// never persisted locally and is omitted after a claim or restart.
    pub qr_payload: Option<String>,
    pub pending_device_name: Option<String>,
    pub pending_device_id: Option<Uuid>,
}
