//! Task workflow is deterministic domain state. Process ownership stays in ditchd.
use super::*;

pub fn initial_task_revision() -> u64 {
    1
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskColumn {
    Todo,
    InProgress,
    InReview,
    Done,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskCondition {
    #[default]
    Idle,
    Queued,
    Running,
    AwaitingApproval,
    Blocked,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskPriority {
    Low,
    #[default]
    Normal,
    High,
    Urgent,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskActor {
    #[default]
    User,
    Agent,
    Daemon,
    Ditchmaster,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskAction {
    Start,
    Submit { summary: String },
    RequestChanges { feedback: String },
    Accept,
    Cancel { reason: String },
    Reopen,
    ReturnToTodo { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskDraft {
    #[serde(default)]
    pub skills: Vec<crate::SkillBinding>,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub priority: TaskPriority,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskAudit {
    pub id: Uuid,
    pub task_id: TaskId,
    pub actor: TaskActor,
    pub action: String,
    pub from_column: Option<TaskColumn>,
    pub to_column: TaskColumn,
    pub agent_id: Option<AgentId>,
    pub created_at: DateTime<Utc>,
    pub reason: Option<String>,
    /// Immutable summary/feedback retained even after a later submission.
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskDetail {
    pub task: Task,
    pub history: Vec<TaskAudit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRequest {
    pub project_id: Option<ProjectId>,
    pub request_id: Uuid,
    pub operation: TaskOperation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskOperation {
    ConfigureAcceptance {
        task_id: TaskId,
        expected_revision: u64,
        config: AcceptanceConfig,
        confirmed_commands: Vec<CommandCheck>,
    },
    ApproveTestPreset {
        name: String,
        command: CommandCheck,
    },
    TestPresets,
    ReviewDiff {
        task_id: TaskId,
        submission_id: Uuid,
    },
    Revalidate {
        task_id: TaskId,
        expected_revision: u64,
    },
    CancelLoop {
        task_id: TaskId,
        expected_revision: u64,
    },
    List {
        column: Option<TaskColumn>,
        include_archived: bool,
    },
    Get {
        task_id: TaskId,
    },
    CreateIdentified {
        task_id: TaskId,
        draft: TaskDraft,
    },
    Create {
        draft: TaskDraft,
    },
    Update {
        task_id: TaskId,
        expected_revision: u64,
        draft: TaskDraft,
    },
    Transition {
        task_id: TaskId,
        expected_revision: u64,
        action: TaskAction,
    },
    Move {
        task_id: TaskId,
        expected_revision: u64,
        column: TaskColumn,
        before_id: Option<TaskId>,
    },
    Start {
        task_id: TaskId,
        expected_revision: u64,
        execution_profile: AgentExecutionProfile,
    },
    Link {
        task_id: TaskId,
        expected_revision: u64,
        agent_id: AgentId,
    },
    Delete {
        task_id: TaskId,
        expected_revision: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskResponse {
    Diff { text: String },
    TestPresets(std::collections::BTreeMap<String, CommandCheck>),
    Tasks(Vec<Task>),
    Detail(TaskDetail),
    Changed(Task),
    Deleted(TaskId),
    Error(TaskError),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskErrorCode {
    NotFound,
    InvalidInput,
    IllegalTransition,
    RevisionConflict,
    AgentBusy,
    WorkspaceBusy,
    IdempotencyConflict,
    HasHistory,
    Storage,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskError {
    pub code: TaskErrorCode,
    pub message: String,
}
impl TaskError {
    pub fn new(code: TaskErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for TaskError {}

impl TaskDraft {
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.title.trim().is_empty()
            || self.title.len() > 512
            || self.description.len() > 65536
            || self.acceptance_criteria.len() > 100
            || self
                .acceptance_criteria
                .iter()
                .any(|s| s.trim().is_empty() || s.len() > 4096)
        {
            return Err(TaskError::new(
                TaskErrorCode::InvalidInput,
                "Provide a title (up to 512 bytes), description (64 KiB), and up to 100 non-empty criteria.",
            ));
        }
        Ok(())
    }
}

impl Task {
    pub fn new(project_id: ProjectId, draft: TaskDraft, order_key: i64) -> Result<Self, TaskError> {
        draft.validate()?;
        let now = Utc::now();
        Ok(Self {
            acceptance: TaskAcceptance::default(),
            skills: draft.skills.clone(),
            id: TaskId::new(),
            project_id,
            title: draft.title.trim().into(),
            description: draft.description,
            acceptance_criteria: draft.acceptance_criteria,
            priority: draft.priority,
            state: TaskState::Ready,
            created_at: now,
            updated_at: Some(now),
            condition: TaskCondition::Idle,
            order_key,
            creator: TaskActor::User,
            assigned_agent_id: None,
            revision: 1,
            started_at: None,
            submitted_at: None,
            completed_at: None,
            cancelled_at: None,
            archived: false,
            last_reason: None,
            review_summary: None,
        })
    }
    /// Preserve existing serialized state names; map legacy execution states explicitly.
    pub fn column(&self) -> TaskColumn {
        match self.state {
            TaskState::Draft | TaskState::Ready | TaskState::Cancelled => TaskColumn::Todo,
            TaskState::Running | TaskState::Blocked | TaskState::Rejected => TaskColumn::InProgress,
            TaskState::InReview => TaskColumn::InReview,
            TaskState::Accepted => TaskColumn::Done,
        }
    }
    pub fn check_revision(&self, expected: u64) -> Result<(), TaskError> {
        if self.revision != expected {
            return Err(TaskError::new(
                TaskErrorCode::RevisionConflict,
                "This task changed. Refresh it and retry.",
            ));
        }
        Ok(())
    }
    pub fn touch(&mut self) {
        self.revision += 1;
        self.updated_at = Some(Utc::now());
    }
    pub fn audit(
        &self,
        actor: TaskActor,
        action: &str,
        from_column: Option<TaskColumn>,
        reason: Option<String>,
    ) -> TaskAudit {
        TaskAudit {
            id: Uuid::new_v4(),
            task_id: self.id,
            actor,
            action: action.into(),
            from_column,
            to_column: self.column(),
            agent_id: self.assigned_agent_id,
            created_at: Utc::now(),
            reason,
            summary: None,
        }
    }
    pub fn transition(
        &self,
        action: &TaskAction,
        actor: TaskActor,
        expected: u64,
        live_child: bool,
    ) -> Result<(Self, TaskAudit), TaskError> {
        self.check_revision(expected)?;
        if live_child {
            return Err(TaskError::new(
                TaskErrorCode::AgentBusy,
                "Stop the active agent before changing this task's workflow.",
            ));
        }
        let column = self.column();
        let mut next = self.clone();
        let mut reason = None;
        let mut summary = None;
        let user = actor == TaskActor::User;
        let active = !self.archived && self.condition != TaskCondition::Cancelled;
        let name = match action {
            TaskAction::Start
                if active && matches!(column, TaskColumn::Todo | TaskColumn::InProgress) =>
            {
                next.state = TaskState::Running;
                next.condition = TaskCondition::Idle;
                next.started_at.get_or_insert(Utc::now());
                "start"
            }
            TaskAction::Submit { summary: text }
                if active
                    && column == TaskColumn::InProgress
                    && !text.trim().is_empty()
                    && text.len() <= 65536 =>
            {
                next.state = TaskState::InReview;
                next.condition = TaskCondition::Idle;
                next.review_summary = Some(text.clone());
                next.submitted_at = Some(Utc::now());
                summary = Some(text.clone());
                "submit"
            }
            TaskAction::RequestChanges { feedback }
                if user
                    && active
                    && column == TaskColumn::InReview
                    && !feedback.trim().is_empty()
                    && feedback.len() <= 65536 =>
            {
                next.state = TaskState::Running;
                next.condition = TaskCondition::Idle;
                reason = Some(feedback.clone());
                next.last_reason = reason.clone();
                "request_changes"
            }
            TaskAction::Accept
                if user
                    && active
                    && column == TaskColumn::InReview
                    && self
                        .review_summary
                        .as_ref()
                        .is_some_and(|s| !s.trim().is_empty()) =>
            {
                next.state = TaskState::Accepted;
                next.completed_at = Some(Utc::now());
                "accept"
            }
            TaskAction::Cancel { reason: text } if user && active && column != TaskColumn::Done => {
                next.condition = TaskCondition::Cancelled;
                next.archived = true;
                next.cancelled_at = Some(Utc::now());
                reason = Some(text.chars().take(4096).collect());
                "cancel"
            }
            TaskAction::Reopen if user && (column == TaskColumn::Done || !active) => {
                next.state = TaskState::Ready;
                next.condition = TaskCondition::Idle;
                next.archived = false;
                next.completed_at = None;
                next.cancelled_at = None;
                "reopen"
            }
            TaskAction::ReturnToTodo { reason: text }
                if user
                    && active
                    && column == TaskColumn::InProgress
                    && !text.trim().is_empty() =>
            {
                next.state = TaskState::Ready;
                next.condition = TaskCondition::Idle;
                reason = Some(text.chars().take(4096).collect());
                "return_to_todo"
            }
            _ => {
                return Err(TaskError::new(
                    TaskErrorCode::IllegalTransition,
                    "This transition is not allowed. Done requires explicit human acceptance from In Review; review and backward moves require a summary or feedback.",
                ));
            }
        };
        next.touch();
        let mut audit = next.audit(actor, name, Some(column), reason);
        audit.summary = summary;
        Ok((next, audit))
    }
}

/// A daemon grants leases before process launch. Local canonicalization happens on
/// the execution host, never by treating an SSH path as a local path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceLease {
    pub target: String,
    pub root: PathBuf,
    pub unconfined: bool,
}
impl WorkspaceLease {
    pub fn conflicts(&self, other: &Self) -> bool {
        self.unconfined
            || other.unconfined
            || (self.target == other.target
                && (self.root.starts_with(&other.root) || other.root.starts_with(&self.root)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task() -> Task {
        Task::new(
            ProjectId::new(),
            TaskDraft {
                skills: vec![],
                title: "Work".into(),
                description: String::new(),
                acceptance_criteria: vec!["Reviewed".into()],
                priority: TaskPriority::Normal,
            },
            1024,
        )
        .unwrap()
    }
    #[test]
    fn every_state_action_and_actor_obeys_the_workflow() {
        let actions = [
            TaskAction::Start,
            TaskAction::Submit {
                summary: "Evidence".into(),
            },
            TaskAction::RequestChanges {
                feedback: "Fix it".into(),
            },
            TaskAction::Accept,
            TaskAction::Cancel {
                reason: "Stop".into(),
            },
            TaskAction::Reopen,
            TaskAction::ReturnToTodo {
                reason: "Defer".into(),
            },
        ];
        let states = [
            TaskState::Draft,
            TaskState::Ready,
            TaskState::Running,
            TaskState::Blocked,
            TaskState::Rejected,
            TaskState::InReview,
            TaskState::Accepted,
        ];
        for state in states {
            for actor in [
                TaskActor::User,
                TaskActor::Agent,
                TaskActor::Daemon,
                TaskActor::Ditchmaster,
            ] {
                for (index, action) in actions.iter().enumerate() {
                    let mut t = task();
                    t.state = state.clone();
                    t.review_summary = Some("Evidence".into());
                    let user = actor == TaskActor::User;
                    let expected = match index {
                        0 => matches!(t.column(), TaskColumn::Todo | TaskColumn::InProgress),
                        1 => t.column() == TaskColumn::InProgress,
                        2 | 3 => user && t.column() == TaskColumn::InReview,
                        4 => user && t.column() != TaskColumn::Done,
                        5 => user && t.column() == TaskColumn::Done,
                        6 => user && t.column() == TaskColumn::InProgress,
                        _ => unreachable!(),
                    };
                    let result = t.transition(action, actor, t.revision, false);
                    assert_eq!(result.is_ok(), expected, "{state:?} {action:?} {actor:?}");
                    assert!(t.transition(action, actor, t.revision, true).is_err());
                    assert!(t.transition(action, actor, t.revision + 1, false).is_err());
                    if let Ok((next, audit)) = result {
                        assert_eq!(next.revision, t.revision + 1);
                        assert_eq!(audit.actor, actor);
                        if next.column() == TaskColumn::Done {
                            assert_eq!(actor, TaskActor::User);
                            assert_eq!(audit.action, "accept");
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn review_requires_nonempty_summary_feedback_and_keeps_prior_submission() {
        let t = task()
            .transition(&TaskAction::Start, TaskActor::User, 1, false)
            .unwrap()
            .0;
        assert!(
            t.transition(
                &TaskAction::Submit {
                    summary: " ".into()
                },
                TaskActor::Agent,
                t.revision,
                false
            )
            .is_err()
        );
        let (review, submission) = t
            .transition(
                &TaskAction::Submit {
                    summary: "First result".into(),
                },
                TaskActor::Agent,
                t.revision,
                false,
            )
            .unwrap();
        assert!(
            review
                .transition(
                    &TaskAction::RequestChanges {
                        feedback: "".into()
                    },
                    TaskActor::User,
                    review.revision,
                    false
                )
                .is_err()
        );
        let corrected = review
            .transition(
                &TaskAction::RequestChanges {
                    feedback: "Add tests".into(),
                },
                TaskActor::User,
                review.revision,
                false,
            )
            .unwrap()
            .0;
        assert_eq!(corrected.last_reason.as_deref(), Some("Add tests"));
        assert_eq!(submission.summary.as_deref(), Some("First result"));
        let cancelled = corrected
            .transition(
                &TaskAction::Cancel {
                    reason: "Later".into(),
                },
                TaskActor::User,
                corrected.revision,
                false,
            )
            .unwrap()
            .0;
        assert_ne!(cancelled.column(), TaskColumn::Done);
        assert!(cancelled.archived);
        assert!(
            cancelled
                .transition(
                    &TaskAction::Start,
                    TaskActor::Agent,
                    cancelled.revision,
                    false
                )
                .is_err()
        );
    }
    #[test]
    fn leases_conflict_on_ancestors_and_unconfined_access_not_string_prefixes() {
        let local = WorkspaceLease {
            target: "host-a".into(),
            root: "/work/a".into(),
            unconfined: false,
        };
        let mut other = local.clone();
        assert!(local.conflicts(&other));
        other.root = "/work/a/nested".into();
        assert!(local.conflicts(&other));
        assert!(other.conflicts(&local));
        other.root = "/work/another".into();
        assert!(!local.conflicts(&other));
        other.root = local.root.clone();
        other.target = "host-b".into();
        assert!(!local.conflicts(&other));
        other.unconfined = true;
        assert!(local.conflicts(&other));
    }
}
