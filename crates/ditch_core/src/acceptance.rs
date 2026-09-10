//! Durable task acceptance contracts. A worker session and an attempt are separate identities.
use super::*;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopPolicy {
    pub enabled: bool,
    pub max_attempts: u32,
    pub deadline_seconds: u64,
    pub validator_timeout_seconds: u64,
    pub stop_on_denial: bool,
    pub stop_on_identical_failure: bool,
    pub require_change: bool,
    pub retry_mode: RetryMode,
}
impl Default for LoopPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_attempts: 3,
            deadline_seconds: 1800,
            validator_timeout_seconds: 60,
            stop_on_denial: true,
            stop_on_identical_failure: true,
            require_change: false,
            retry_mode: RetryMode::Fresh,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RetryMode {
    Fresh,
    Resume,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandCheck {
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub expected_exit: i32,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CriterionKind {
    Human,
    AgentAssertion,
    Command(CommandCheck),
    TestPreset {
        name: String,
    },
    File {
        path: String,
        predicate: FilePredicate,
    },
    Git(GitPredicate),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FilePredicate {
    Exists,
    Absent,
    Contains(String),
    NotContains(String),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GitPredicate {
    Clean,
    Dirty,
    ChangedPath(String),
    ForbiddenPath(String),
    MaxDiffBytes(u64),
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AcceptanceCriterion {
    pub id: Uuid,
    pub label: String,
    pub required: bool,
    pub kind: CriterionKind,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AcceptanceConfig {
    pub policy: LoopPolicy,
    pub criteria: Vec<AcceptanceCriterion>,
}
impl AcceptanceConfig {
    pub fn validate(&self) -> Result<(), String> {
        let p = &self.policy;
        if !(1..=10).contains(&p.max_attempts)
            || !(1..=86400).contains(&p.deadline_seconds)
            || !(1..=300).contains(&p.validator_timeout_seconds)
            || !p.stop_on_denial
            || self.criteria.len() > 16
        {
            return Err("Use 1–10 attempts, a deadline of 1–86400 seconds, validator timeouts of 1–300 seconds and at most 16 criteria. Permission denial always stops the loop.".into());
        }
        let mut ids = std::collections::HashSet::new();
        for c in &self.criteria {
            if !ids.insert(c.id) || c.label.trim().is_empty() || c.label.len() > 512 {
                return Err("Criteria need unique IDs and bounded labels".into());
            }
            match &c.kind {
                CriterionKind::Command(command) => command.validate()?,
                CriterionKind::File { path, predicate } => {
                    relative_check_path(path)?;
                    if let FilePredicate::Contains(v) | FilePredicate::NotContains(v) = predicate
                        && v.len() > 4096
                    {
                        return Err("Content predicate exceeds 4 KiB".into());
                    }
                }
                CriterionKind::Git(
                    GitPredicate::ChangedPath(p) | GitPredicate::ForbiddenPath(p),
                ) => {
                    relative_check_path(p)?;
                }
                CriterionKind::TestPreset { name } if name.is_empty() || name.len() > 128 => {
                    return Err("Invalid preset name".into());
                }
                _ => {}
            }
        }
        Ok(())
    }
}
impl CommandCheck {
    pub fn validate(&self) -> Result<(), String> {
        if self.argv.iter().map(String::len).sum::<usize>() > 16384
            || self.argv.is_empty()
            || self.argv.len() > 64
            || self.argv.iter().any(|v| v.contains('\0') || v.len() > 4096)
            || self.argv[0].is_empty()
            || !(1..=300).contains(&self.timeout_seconds)
        {
            return Err("Validator requires fixed argv and a 1–300 second timeout".into());
        }
        relative_check_path(&self.cwd)?;
        Ok(())
    }
}
pub fn relative_check_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 1024
        || path.contains(['\\', '\0', '\n', '\r'])
        || std::path::Path::new(path).components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("Validator path must stay inside the registered project".into());
    }
    Ok(())
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceEvidence {
    pub fingerprint: String,
    pub head: Option<String>,
    pub dirty_paths: Vec<String>,
    pub diff_stat: String,
    pub diff_bytes: u64,
    pub warnings: Vec<String>,
    pub captured_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckStatus {
    Passed,
    Failed,
    HumanReview,
    Assertion,
    InfrastructureError,
    Cancelled,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidatorEvidence {
    pub criterion: AcceptanceCriterion,
    pub status: CheckStatus,
    pub command: Option<CommandCheck>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub output: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AttemptResult {
    Queued,
    Working,
    Validating,
    Passed,
    Failed,
    Blocked,
    Cancelled,
    Interrupted,
}
impl AttemptResult {
    pub fn active(&self) -> bool {
        matches!(self, Self::Queued | Self::Working | Self::Validating)
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttemptFailure {
    pub code: String,
    pub message: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskAttempt {
    #[serde(default)]
    pub validation_only: bool,
    pub id: Uuid,
    pub cycle_id: Uuid,
    pub task_id: TaskId,
    pub agent_id: AgentId,
    pub thread_id: Option<String>,
    pub ordinal: u32,
    pub skills: Vec<SkillBinding>,
    pub execution_profile: AgentExecutionProfile,
    pub prompt: String,
    pub reviewer_feedback: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub result: AttemptResult,
    pub failure: Option<AttemptFailure>,
    pub base: WorkspaceEvidence,
    pub workspace: Option<WorkspaceEvidence>,
    pub validators: Vec<ValidatorEvidence>,
    pub summary: Option<String>,
    pub provider_usage: Option<serde_json::Value>,
    pub approvals: Vec<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReviewSubmission {
    pub id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub agent_id: Option<AgentId>,
    pub summary: String,
    pub criteria: Vec<ValidatorEvidence>,
    pub workspace: WorkspaceEvidence,
    pub preexisting_dirty_paths: Vec<String>,
    pub approvals: Vec<String>,
    pub created_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskAcceptance {
    pub config: AcceptanceConfig,
    pub attempts: Vec<TaskAttempt>,
    pub submissions: Vec<ReviewSubmission>,
    pub current_submission: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budgets_and_commands_have_hard_limits() {
        let mut config = AcceptanceConfig::default();
        assert!(config.validate().is_ok());
        for max in [0, 11, u32::MAX] {
            config.policy.max_attempts = max;
            assert!(config.validate().is_err());
        }
        config = AcceptanceConfig::default();
        config.policy.stop_on_denial = false;
        assert!(config.validate().is_err());
        for path in ["../outside", "/absolute", "a/../outside"] {
            assert!(relative_check_path(path).is_err());
        }
        assert!(relative_check_path(".").is_ok());
        assert!(relative_check_path("tests/fixture").is_ok());
        assert!(
            CommandCheck {
                argv: vec![],
                cwd: ".".into(),
                timeout_seconds: 10,
                expected_exit: 0
            }
            .validate()
            .is_err()
        );
    }
}
