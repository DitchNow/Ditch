//! Task workflow is deterministic domain state. Process ownership stays in ditchd.
use super::*;

pub fn initial_task_revision() -> u64 {
    1
}

/// Planning is auxiliary work, never the task's execution agent or board state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskPlanning {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub job: Option<TaskPlanJob>,
}

/// User-authorized execution order, scoped to the task's project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskExecutionQueue {
    /// Locked for this In Progress admission, including failed launches/retries.
    #[serde(default)]
    pub agent_id: Option<AgentId>,
    pub position: u64,
    pub profile: AgentExecutionProfile,
    #[serde(default)]
    pub feedback: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskPlanJob {
    pub id: Uuid,
    pub status: String,
    pub title: String,
    pub description: String,
    pub previous_plan: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskColumn {
    Backlog,
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
    Prioritize,
    ReturnToBacklog,
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
    pub issue_kind: TaskIssueKind,
    #[serde(default)]
    pub skills: Vec<crate::SkillBinding>,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Vec<String>,
    pub priority: TaskPriority,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskIssueKind {
    Bug,
    Feature,
    #[default]
    Custom,
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
    ListReleases,
    AcceptRelease {
        tasks: Vec<TaskRevision>,
        destination: ReleaseDestination,
    },
    AssignRelease {
        tasks: Vec<TaskRevision>,
        destination: ReleaseDestination,
    },
    /// Versioned SSH handoff. Copies an idle board task; never authorizes execution.
    MaterializeBoardTask {
        task: Box<Task>,
        expected_remote: Option<Box<Task>>,
    },
    RepositoryDiscovery,
    AttachGitHub {
        task_id: TaskId,
        source: crate::github::GitHubTaskSource,
        issue: crate::github::GitHubIssue,
    },
    BoardExecution {
        paused: Option<bool>,
        expected_revision: Option<u64>,
    },
    Enqueue {
        task_id: TaskId,
        expected_revision: u64,
        execution_profile: AgentExecutionProfile,
    },
    ReorderQueue {
        task_id: TaskId,
        expected_revision: u64,
        before_id: Option<TaskId>,
    },
    GeneratePlan {
        task_id: TaskId,
        expected_revision: u64,
        model: Option<String>,
    },
    CancelPlan {
        task_id: TaskId,
        job_id: Uuid,
    },
    UpdateDescription {
        task_id: TaskId,
        expected_description: String,
        expected_plan: String,
        description: String,
        plan: String,
    },
    BoardProjects {
        members: Option<Vec<ProjectId>>,
    },
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
    CreateBacklog {
        draft: TaskDraft,
    },
    CreateIdentified {
        #[serde(default)]
        board_task: Option<Box<Task>>,
        task_id: TaskId,
        #[serde(default)]
        coordinator_group: Option<Uuid>,
        #[serde(default)]
        continue_agent_id: Option<AgentId>,
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
    SelectAgent {
        task_id: TaskId,
        expected_revision: u64,
        agent_id: Option<AgentId>,
        /// Explicit confirmation when taking a stopped agent from another Todo task.
        #[serde(default)]
        replace_task_id: Option<TaskId>,
        #[serde(default)]
        replace_task_revision: Option<u64>,
    },
    Delete {
        task_id: TaskId,
        expected_revision: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TaskResponse {
    Releases(Vec<ProjectRelease>),
    ReleaseBatch {
        tasks: Vec<Task>,
        releases: Vec<ProjectRelease>,
    },
    /// Immutable transfer receipt, unlike Changed which replays the latest task.
    Materialized(Box<Task>),
    RepositoryDiscovery(serde_json::Value),
    BoardExecution(BoardExecutionState),
    BoardProjects(Vec<ProjectId>),
    Diff {
        text: String,
    },
    TestPresets(std::collections::BTreeMap<String, CommandCheck>),
    Tasks(Vec<Task>),
    Detail(TaskDetail),
    Changed(Task),
    Deleted(TaskId),
    Error(TaskError),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BoardExecutionState {
    pub project_id: ProjectId,
    pub paused: bool,
    pub revision: u64,
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
            release_id: None,
            issue_kind: draft.issue_kind,
            execution_queue: None,
            planning: TaskPlanning::default(),
            remote_pending: false,
            github_source: None,
            coordinator_group: None,
            continue_agent_id: None,
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
            TaskState::Backlog => TaskColumn::Backlog,
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
    pub fn waiting_in_queue(&self) -> bool {
        self.execution_queue.is_some()
            && self.column() == TaskColumn::InProgress
            && self.condition == TaskCondition::Queued
            && self.assigned_agent_id.is_none()
            && !self.archived
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
            TaskAction::Prioritize if user && active && column == TaskColumn::Backlog => {
                next.state = TaskState::Ready;
                "prioritize"
            }
            TaskAction::ReturnToBacklog if user && active && column == TaskColumn::Todo => {
                next.state = TaskState::Backlog;
                "return_to_backlog"
            }
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
                next.release_id = None;
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
    #[serde(default)]
    pub coordinator_group: Option<Uuid>,
    pub target: String,
    pub root: PathBuf,
    pub unconfined: bool,
}
impl WorkspaceLease {
    pub fn conflicts(&self, other: &Self) -> bool {
        self.coordinator_group.is_some()
            && self.coordinator_group == other.coordinator_group
            && self.target == other.target
            && self.root == other.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task() -> Task {
        Task::new(
            ProjectId::new(),
            TaskDraft {
                issue_kind: Default::default(),
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
    fn old_tasks_default_to_custom_and_new_types_round_trip() {
        let mut current = task();
        current.issue_kind = TaskIssueKind::Bug;
        let mut value = serde_json::to_value(&current).unwrap();
        assert_eq!(serde_json::from_value::<Task>(value.clone()).unwrap().issue_kind, TaskIssueKind::Bug);
        value.as_object_mut().unwrap().remove("issue_kind");
        assert_eq!(serde_json::from_value::<Task>(value).unwrap().issue_kind, TaskIssueKind::Custom);
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
    fn leases_only_conflict_for_the_same_coordinator_project() {
        let local = WorkspaceLease {
            coordinator_group: Some(Uuid::new_v4()),
            target: "host-a".into(),
            root: "/work/a".into(),
            unconfined: false,
        };
        let mut other = local.clone();
        assert!(local.conflicts(&other));
        other.root = "/work/a/nested".into();
        assert!(!local.conflicts(&other));
        assert!(!other.conflicts(&local));
        other.root = "/work/another".into();
        assert!(!local.conflicts(&other));
        other.root = local.root.clone();
        other.target = "host-b".into();
        assert!(!local.conflicts(&other));
        other.unconfined = true;
        assert!(!local.conflicts(&other));
        other.target = local.target.clone();
        assert!(local.conflicts(&other));
        other.coordinator_group = None;
        assert!(!local.conflicts(&other));
        other.coordinator_group = Some(Uuid::new_v4());
        assert!(!local.conflicts(&other));
    }
}

/// Project-owned labels, independent of Git tags.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectRelease {
    pub id: Uuid,
    pub project_id: ProjectId,
    pub name: String,
    pub created_at: DateTime<Utc>,
}
impl ProjectRelease {
    pub fn new(project_id: ProjectId, name: &str) -> Result<Self, TaskError> {
        let name = name.trim();
        if name.is_empty() || name.len() > 128 {
            return Err(TaskError::new(
                TaskErrorCode::InvalidInput,
                "Release names must contain 1–128 bytes after trimming.",
            ));
        }
        Ok(Self {
            id: Uuid::new_v4(),
            project_id,
            name: name.into(),
            created_at: Utc::now(),
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskRevision {
    pub task_id: TaskId,
    pub expected_revision: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReleaseDestination {
    Other,
    Existing(Uuid),
    New(String),
}

#[cfg(test)]
mod release_tests {
    use super::*;
    #[test]
    fn legacy_tasks_default_to_other_and_release_names_are_trimmed() {
        let task = Task::new(
            ProjectId::new(),
            TaskDraft {
                issue_kind: Default::default(),
                skills: vec![],
                title: "Legacy".into(),
                description: String::new(),
                acceptance_criteria: vec![],
                priority: TaskPriority::Normal,
            },
            1,
        )
        .unwrap();
        let mut json = serde_json::to_value(&task).unwrap();
        json.as_object_mut().unwrap().remove("release_id");
        assert_eq!(
            serde_json::from_value::<Task>(json).unwrap().release_id,
            None
        );
        assert_eq!(
            ProjectRelease::new(task.project_id, " Preview 1 ")
                .unwrap()
                .name,
            "Preview 1"
        );
        assert!(ProjectRelease::new(task.project_id, " ").is_err());
        assert!(ProjectRelease::new(task.project_id, &"x".repeat(129)).is_err());
    }
}
