use crate::{ProjectId, TaskActor, TaskId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum AgentTransport {
    #[default]
    Legacy,
    AppServer,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillBinding {
    pub identity: String,
    pub name: String,
    pub path: PathBuf,
    pub content_hash: String,
    pub revision: Option<String>,
    pub origin: TaskActor,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillEntry {
    #[serde(default)]
    pub source_id: Option<Uuid>,
    #[serde(default)]
    pub relative_path: Option<String>,
    pub identity: String,
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub scope: String,
    pub source: String,
    pub enabled: bool,
    pub recognized: bool,
    pub content_hash: String,
    pub revision: Option<String>,
    pub interface: serde_json::Value,
    pub dependencies: serde_json::Value,
    pub missing_dependencies: Vec<String>,
    pub license: Option<String>,
    pub has_scripts: bool,
    pub validation_error: Option<String>,
    pub managed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillSource {
    pub id: Uuid,
    pub name: String,
    /// Absolute directory or an explicit HTTPS/SSH Git URL; never a shell command.
    pub location: String,
    pub reference: String,
    pub seeded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillInstallPlan {
    pub id: Uuid,
    pub source: SkillSource,
    pub resolved_revision: Option<String>,
    pub entry: SkillEntry,
    pub included_paths: Vec<String>,
    pub destination: PathBuf,
    pub prior_revision: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManagedSkill {
    pub identity: String,
    pub source: SkillSource,
    pub relative_path: String,
    pub versions: Vec<SkillInstallPlan>,
    pub current: usize,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillRequest {
    pub project_id: Option<ProjectId>,
    pub operation: SkillOperation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SkillOperation {
    Capabilities,
    List {
        offset: usize,
        limit: usize,
        source: Option<String>,
        refresh: bool,
    },
    Preview {
        identity: String,
    },
    SetEnabled {
        identity: String,
        enabled: bool,
    },
    Sources {
        offset: usize,
        limit: usize,
    },
    AddSource {
        name: String,
        location: String,
        reference: String,
    },
    RemoveSource {
        source_id: Uuid,
    },
    BrowseSource {
        source_id: Uuid,
        allow_network: bool,
        offset: usize,
        limit: usize,
    },
    PrepareInstall {
        source_id: Uuid,
        relative_path: String,
        allow_network: bool,
    },
    ConfirmInstall {
        plan_id: Uuid,
    },
    DiscardPlan {
        plan_id: Uuid,
    },
    Rollback {
        identity: String,
        content_hash: String,
    },
    Versions {
        identity: String,
    },
    Remove {
        identity: String,
    },
    Bind {
        task_id: TaskId,
        expected_revision: u64,
        skills: Vec<SkillBinding>,
    },
    /// Sending this operation is explicit user authorization for the selected revision only.
    Sync {
        identity: String,
        content_hash: String,
        target_project_id: ProjectId,
    },
    /// Internal typed host operation; the payload contains validated regular files only.
    Import {
        #[serde(default)]
        resolved_revision: Option<String>,
        #[serde(default)]
        license: Option<String>,
        source: SkillSource,
        relative_path: String,
        content_hash: String,
        files: Vec<SkillFile>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillFile {
    pub path: String,
    pub content_base64: String,
    pub executable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SkillResponse {
    Capabilities {
        app_server: bool,
        extra_roots: bool,
        sandbox_supported: bool,
        selected_boundary: String,
        details: Vec<String>,
    },
    Entries {
        entries: Vec<SkillEntry>,
        next_offset: Option<usize>,
        errors: Vec<String>,
        app_server: bool,
    },
    Sources {
        sources: Vec<SkillSource>,
        next_offset: Option<usize>,
    },
    Preview {
        entry: SkillEntry,
        instructions: String,
    },
    Plan(Box<SkillInstallPlan>),
    Versions(Vec<SkillInstallPlan>),
    Installed(SkillEntry),
    Bound(crate::Task),
    Accepted,
    Error {
        code: String,
        message: String,
    },
}
