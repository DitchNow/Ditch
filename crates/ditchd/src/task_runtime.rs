use ditch_core::{
    Task, TaskAction, TaskActor, TaskAudit, TaskColumn, TaskCondition, TaskError, TaskErrorCode,
    TaskId, TaskOperation, TaskRequest, TaskResponse, WorkspaceLease,
};

fn task_error(code: TaskErrorCode, message: impl Into<String>) -> ServerResponse {
    ServerResponse::TaskResponse(TaskResponse::Error(TaskError::new(code, message)))
}

/// A reservation exists before spawning, closing the concurrent-start race.
struct WriterGuard {
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    retained: bool,
    process_group: std::cell::Cell<Option<i32>>,
}
impl WriterGuard {
    fn acquire(
        state: &Arc<Mutex<RuntimeState>>,
        agent_id: AgentId,
        project: &Project,
        profile: &AgentExecutionProfile,
    ) -> Result<Self, String> {
        let root = if project.is_remote() {
            project.root.clone()
        } else {
            fs::canonicalize(&project.root)
                .map_err(|e| format!("Cannot resolve project workspace: {e}"))?
        };
        #[cfg(target_os = "macos")]
        let root = if project.is_remote() {
            root
        } else {
            PathBuf::from(root.to_string_lossy().to_lowercase())
        };
        let mut locked = state.lock().expect("runtime state lock poisoned");
        locked.reap_orphan_writers();
        if locked.acceptance_owners.contains(&agent_id)
            && locked.writer_leases.contains_key(&agent_id)
            && !locked.children.contains_key(&agent_id)
        {
            return Ok(Self {
                state: Arc::clone(state),
                agent_id,
                retained: true,
                process_group: std::cell::Cell::new(None),
            });
        }
        let target = match &project.execution_target {
            ProjectExecutionTarget::Local => "local".into(),
            ProjectExecutionTarget::Remote {
                remote_machine_id, ..
            } => remote_machine_id.to_string(),
        };
        let lease = WorkspaceLease {
            target,
            root,
            unconfined: locked.remote_runtime
                || project.is_remote()
                || profile.approval == AgentApprovalPreset::FullAccess,
        };
        let remote_live = locked.agents.values().any(|r| {
            r.run.can_stop
                && locked
                    .projects
                    .values()
                    .any(|p| p.id == r.run.project_id && p.is_remote())
        });
        if remote_live
            || locked.writer_leases.contains_key(&agent_id)
            || locked
                .writer_leases
                .values()
                .any(|other| lease.conflicts(other))
        {
            return Err("Another agent owns a conflicting workspace. Stop it or wait for completion. Full Access and unconfined SSH work require exclusive access.".into());
        }
        if let Some(task_id) = locked.agents.get(&agent_id).and_then(|r| r.run.task_id) {
            let task = locked
                .tasks
                .get(&task_id)
                .ok_or("The linked task is unavailable.")?;
            if task.column() != TaskColumn::InProgress
                || task.archived
                || task.assigned_agent_id != Some(agent_id)
            {
                return Err(
                    "Use Request Changes or reopen the linked task before sending more work."
                        .into(),
                );
            }
        }
        locked
            .store
            .reserve_writer(agent_id, &lease)
            .map_err(|e| e.to_string())?;
        locked.writer_leases.insert(agent_id, lease);
        Ok(Self {
            state: Arc::clone(state),
            agent_id,
            retained: false,
            process_group: std::cell::Cell::new(None),
        })
    }
    fn started(&self, pid: i32) -> io::Result<()> {
        self.process_group.set(Some(pid));
        self.state
            .lock()
            .expect("runtime lock poisoned")
            .store
            .writer_started(self.agent_id, pid)
            .map_err(io::Error::other)
    }
    fn retain(mut self) {
        self.retained = true;
    }
    fn retain_as(mut self, agent_id: AgentId) {
        let mut state = self.state.lock().expect("runtime state lock poisoned");
        if state.store.transfer_writer(self.agent_id, agent_id).is_ok()
            && let Some(lease) = state.writer_leases.remove(&self.agent_id)
        {
            state.writer_leases.insert(agent_id, lease);
        }
        self.retained = true;
    }
}
impl Drop for WriterGuard {
    fn drop(&mut self) {
        if !self.retained {
            if let Some(pid) = self
                .process_group
                .get()
                .filter(|pid| process_group_exists(*pid))
            {
                signal_process_group(pid, libc::SIGKILL);
                self.state
                    .lock()
                    .expect("runtime lock poisoned")
                    .orphan_writer_groups
                    .insert(self.agent_id, pid);
            } else {
                self.state
                    .lock()
                    .expect("runtime state lock poisoned")
                    .release_writer(self.agent_id);
            }
        }
    }
}

impl RuntimeState {
    fn release_writer(&mut self, id: AgentId) {
        if self.acceptance_owners.contains(&id)
            || self.tasks.values().any(|t| {
                t.assigned_agent_id == Some(id)
                    && matches!(
                        t.condition,
                        TaskCondition::Queued
                            | TaskCondition::Running
                            | TaskCondition::AwaitingApproval
                    )
                    && self
                        .projects
                        .values()
                        .any(|p| p.id == t.project_id && p.is_remote())
            })
        {
            return;
        }
        // The leader can exit while a helper still owns this process group.
        // Keep its lease until the remaining children have actually stopped.
        let Ok(reservations) = self.store.writer_reservations() else {
            return;
        };
        if let Some(pid) = reservations
            .into_iter()
            .find_map(|(owner, _, pid)| (owner == id).then_some(pid).flatten())
            .filter(|pid| process_group_exists(*pid))
        {
            if !self.validator_owners.contains(&id) {
                signal_process_group(pid, libc::SIGKILL);
            }
            self.orphan_writer_groups.insert(id, pid);
            return;
        }
        if self.store.release_writer(id).is_ok() {
            self.validator_owners.remove(&id);
            self.writer_leases.remove(&id);
            self.orphan_writer_groups.remove(&id);
        }
    }
    fn reap_orphan_writers(&mut self) {
        let exited: Vec<_> = self
            .orphan_writer_groups
            .iter()
            .filter_map(|(id, pid)| (!process_group_exists(*pid)).then_some(*id))
            .collect();
        for id in exited {
            self.release_writer(id);
            self.sync_agent_task(id);
        }
    }

    fn task_live(&self, task: &Task) -> bool {
        self.acceptance_controls.contains_key(&task.id)
            || task.assigned_agent_id.is_some_and(|id| {
                self.children.contains_key(&id) || self.writer_leases.contains_key(&id)
            })
            || matches!(
                task.condition,
                TaskCondition::Queued | TaskCondition::Running | TaskCondition::AwaitingApproval
            )
    }
    fn publish_tasks(&mut self, changes: Vec<(Task, TaskAudit)>) {
        for (task, _) in changes {
            self.tasks.insert(task.id, task.clone());
            self.broadcast(ServerEvent::TaskChanged(task));
        }
    }
    fn sync_agent_task(&mut self, agent_id: AgentId) {
        if self.acceptance_owners.contains(&agent_id) {
            return;
        }
        let Some(record) = self.agents.get(&agent_id) else {
            return;
        };
        let Some(task_id) = record.run.task_id else {
            return;
        };
        let Some(task) = self.tasks.get(&task_id).cloned() else {
            return;
        };
        // SSH task authority belongs to the host daemon; client projection follows its events.
        if self
            .projects
            .values()
            .any(|p| p.id == task.project_id && p.is_remote())
        {
            return;
        }
        if task.assigned_agent_id != Some(agent_id)
            || task.archived
            || task.column() != TaskColumn::InProgress
        {
            return;
        }
        let live = self.children.contains_key(&agent_id)
            || record.run.can_stop
            || self.orphan_writer_groups.contains_key(&agent_id);
        // Linking a completed conversation is not a new execution. Likewise an
        // old completion event must not undo Request Changes or resubmit a
        // blocked, summary-less result.
        if !live
            && record.run.state == AgentState::Completed
            && !matches!(
                task.condition,
                TaskCondition::Running | TaskCondition::Queued | TaskCondition::AwaitingApproval
            )
        {
            return;
        }
        let condition = match record.run.state {
            AgentState::AwaitingApproval => TaskCondition::AwaitingApproval,
            AgentState::Blocked => TaskCondition::Blocked,
            _ if live => TaskCondition::Running,
            AgentState::Failed | AgentState::Interrupted | AgentState::Stale => {
                TaskCondition::Failed
            }
            _ => TaskCondition::Idle,
        };
        let summary = record
            .messages
            .iter()
            .rev()
            .take_while(|m| m.role != AgentChatRole::User)
            .find(|m| m.role == AgentChatRole::Assistant)
            .map(|m| m.text.clone());
        let completed = !live && record.run.state == AgentState::Completed;
        let change = if completed && summary.as_ref().is_some_and(|s| !s.trim().is_empty()) {
            task.transition(
                &TaskAction::Submit {
                    summary: summary.unwrap().chars().take(16000).collect(),
                },
                TaskActor::Agent,
                task.revision,
                false,
            )
            .ok()
        } else if condition != task.condition || (completed && task.last_reason.is_none()) {
            let mut next = task.clone();
            next.condition = if completed {
                TaskCondition::Blocked
            } else {
                condition
            };
            if completed {
                next.last_reason = Some("The agent exited without a final summary. Review its chat and submit a summary manually.".into());
            } else if matches!(condition, TaskCondition::Failed | TaskCondition::Blocked) {
                next.last_reason = record
                    .terminal_failure
                    .clone()
                    .or_else(|| record.run.last_visible_action.clone());
            }
            next.touch();
            let audit = next.audit(
                TaskActor::Daemon,
                "agent_condition",
                Some(task.column()),
                next.last_reason.clone(),
            );
            Some((next, audit))
        } else {
            None
        };
        if let Some((next, audit)) = change {
            let changes = vec![(next.clone(), audit)];
            if let Err(error) = self.store.save_task_changes(&changes, None, None) {
                eprintln!("{RUNTIME_IDENTITY} task persistence failed: {error}");
                return;
            }
            self.publish_tasks(changes);
            if next.column() == TaskColumn::InReview
                || matches!(
                    next.condition,
                    TaskCondition::Failed | TaskCondition::Blocked
                )
            {
                self.task_attention(&next);
            }
        }
    }
    fn task_attention(&mut self, task: &Task) {
        let title = if task.column() == TaskColumn::InReview {
            "Task ready for review"
        } else {
            match task
                .last_reason
                .as_deref()
                .unwrap_or("")
                .split(':')
                .next()
                .unwrap_or("")
            {
                "budget_exhausted" => "Attempt budget exhausted",
                "workspace_changed" => "Workspace changed externally",
                "validator_infrastructure" => "Validator infrastructure error",
                "cancelled" => "Task loop cancelled",
                _ => "Task loop blocked",
            }
        };
        let body = format!(
            "{}: {}",
            task.title,
            task.last_reason
                .as_deref()
                .unwrap_or("Inspect the summary, then Accept or Request Changes.")
        );
        if self.attention.iter().any(|a| {
            a.agent_id == task.assigned_agent_id
                && a.project_id == Some(task.project_id)
                && a.title == title
                && a.body == body
        }) {
            return;
        }
        let attention = RuntimeAttention {
            id: Uuid::new_v4(),
            kind: if task.column() == TaskColumn::InReview {
                AttentionKind::Completed
            } else {
                AttentionKind::Blocked
            },
            agent_id: task.assigned_agent_id,
            project_id: Some(task.project_id),
            project_name: self
                .projects
                .values()
                .find(|p| p.id == task.project_id)
                .map(|p| p.name.clone()),
            agent_name: None,
            title: title.into(),
            body,
            created_at: Utc::now(),
            read_at: None,
        };
        if self.store.upsert_attention(&attention).is_ok() {
            self.attention.push(attention.clone());
            self.broadcast(ServerEvent::AttentionRaised(attention));
        }
    }
}

fn handle_task_request(state: Arc<Mutex<RuntimeState>>, request: TaskRequest) -> ServerResponse {
    // Route by stable project identity, never by a remote path interpreted locally.
    if let Some(project) = request
        .project_id
        .and_then(|id| project_by_id(&state, id))
        .filter(Project::is_remote)
    {
        let alias = ssh_remote::remote_alias(&project).unwrap().to_owned();
        let connections = state
            .lock()
            .expect("runtime state lock poisoned")
            .remote_connections
            .clone();
        // Additive capability probe preserves v3 SSH interoperability and avoids sending
        // unknown enum variants to an older remote runtime.
        match connections.request(&alias, ClientRequest::RuntimeStatus) {
            Ok(ServerResponse::RuntimeStatus(status))
                if status.capabilities.iter().any(|c| {
                    c == if matches!(request.operation, TaskOperation::CreateIdentified { .. }) { "identified_tasks_v1" } else if matches!(
                        request.operation,
                        TaskOperation::List { .. }
                            | TaskOperation::Get { .. }
                            | TaskOperation::Create { .. }
                            | TaskOperation::Update { .. }
                            | TaskOperation::Move { .. }
                            | TaskOperation::Link { .. }
                            | TaskOperation::Delete { .. }
                    ) {
                        "tasks_v1"
                    } else {
                        "acceptance_v1"
                    }
                }) => {}
            _ => {
                return task_error(
                    TaskErrorCode::Unsupported,
                    "This SSH runtime does not support tasks. Install the matching runtime before using its Board.",
                );
            }
        }
        ensure_remote_event_subscription(&state, &alias);
        return if matches!(
            request.operation,
            TaskOperation::Start { .. }
                | TaskOperation::Revalidate { .. }
                | TaskOperation::Transition {
                    action: TaskAction::RequestChanges { .. } | TaskAction::Submit { .. },
                    ..
                }
        ) {
            remote_writer_request(&state, &project, ClientRequest::TaskRequest(request))
        } else {
            match connections.request(&alias, ClientRequest::TaskRequest(request)) {
                Ok(response) => response,
                Err(e) => task_error(
                    TaskErrorCode::Unsupported,
                    format!("SSH task request failed; retry with the same request ID: {e}"),
                ),
            }
        };
    }
    if let Some(response) = acceptance_request(&state, &request) {
        return response;
    }
    if let TaskOperation::Create { draft } | TaskOperation::CreateIdentified { draft, .. } | TaskOperation::Update { draft, .. } =
        &request.operation
        && !draft.skills.is_empty()
    {
        let Some(project) = request.project_id.and_then(|id| project_by_id(&state, id)) else {
            return task_error(TaskErrorCode::NotFound, "Select a project");
        };
        if let Err(error) = validate_selected_skills(&state, &project, &draft.skills) {
            return task_error(TaskErrorCode::InvalidInput, error);
        }
    }
    let result = prepare_task_request(&state, &request);
    match result {
        Err(error) => ServerResponse::TaskResponse(TaskResponse::Error(error)),
        Ok((response, None)) => ServerResponse::TaskResponse(response),
        Ok((_, Some((task, project, profile)))) => {
            begin_acceptance(state, task, project, profile, false)
        }
    }
}

type PreparedTask = (TaskResponse, Option<(Task, Project, AgentExecutionProfile)>);
fn prepare_task_request(
    state: &Arc<Mutex<RuntimeState>>,
    request: &TaskRequest,
) -> Result<PreparedTask, TaskError> {
    let mut state = state.lock().expect("runtime state lock poisoned");
    state.reap_orphan_writers();
    let storage =
        |e: ditch_store::StoreError| TaskError::new(TaskErrorCode::Storage, e.to_string());
    if !matches!(
        request.operation,
        TaskOperation::List { .. } | TaskOperation::Get { .. }
    ) && let Some(replay) = state.store.task_replay(request).map_err(storage)?
    {
        return Ok((replay, None));
    }
    if let TaskOperation::List {
        column,
        include_archived,
    } = &request.operation
    {
        let mut tasks: Vec<_> = state
            .tasks
            .values()
            .filter(|t| {
                request.project_id.is_none_or(|p| p == t.project_id)
                    && column.is_none_or(|c| c == t.column())
                    && (*include_archived || !t.archived)
            })
            .cloned()
            .collect();
        tasks.sort_by_key(|t| (t.order_key, t.id.0));
        return Ok((TaskResponse::Tasks(tasks), None));
    }
    let project_id = request.project_id.ok_or_else(|| {
        TaskError::new(
            TaskErrorCode::InvalidInput,
            "A task action must name its project.",
        )
    })?;
    let project = state
        .projects
        .values()
        .find(|p| p.id == project_id && p.archived_at.is_none())
        .cloned()
        .ok_or_else(|| TaskError::new(TaskErrorCode::NotFound, "Project not found."))?;
    if let TaskOperation::Create { draft } | TaskOperation::CreateIdentified { draft, .. } = &request.operation {
        let order = state
            .tasks
            .values()
            .map(|t| t.order_key)
            .max()
            .unwrap_or(0)
            .saturating_add(1024);
        let mut task = Task::new(project_id, draft.clone(), order)?;
        if let TaskOperation::CreateIdentified { task_id, .. } = &request.operation {
            if state.tasks.contains_key(task_id) { return Err(TaskError::new(TaskErrorCode::IdempotencyConflict, "Task identity already exists")); }
            task.id = *task_id;
        }
        for binding in &mut task.skills {
            binding.origin = TaskActor::User;
            binding.reason = None;
            binding.created_at = Utc::now();
        }
        let audit = task.audit(TaskActor::User, "create", None, None);
        let response = TaskResponse::Changed(task.clone());
        let changes = vec![(task, audit)];
        state
            .store
            .save_task_changes(&changes, Some((request, &response)), None)
            .map_err(storage)?;
        state.publish_tasks(changes);
        return Ok((response, None));
    }
    let (task_id, expected) = match &request.operation {
        TaskOperation::Get { task_id } => (*task_id, None),
        TaskOperation::Update {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::Transition {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::Move {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::Start {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::Link {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::Delete {
            task_id,
            expected_revision,
        } => (*task_id, Some(*expected_revision)),
        _ => unreachable!(),
    };
    let task = state
        .tasks
        .get(&task_id)
        .filter(|t| t.project_id == project_id)
        .cloned()
        .ok_or_else(|| {
            TaskError::new(TaskErrorCode::NotFound, "Task not found in this project.")
        })?;
    if let Some(expected) = expected {
        task.check_revision(expected)?;
    }
    if matches!(request.operation, TaskOperation::Get { .. }) {
        return Ok((
            TaskResponse::Detail(state.store.task_detail(&task).map_err(storage)?),
            None,
        ));
    }
    let live = state.task_live(&task);
    if live {
        return Err(TaskError::new(
            TaskErrorCode::AgentBusy,
            "Stop the active task agent before changing this task.",
        ));
    }
    let mut next = task.clone();
    let mut changes = Vec::new();
    let mut linked = None;
    let mut start = None;
    let audit = match &request.operation {
        TaskOperation::Update { draft, .. } => {
            draft.validate()?;
            if task.archived || task.column() == TaskColumn::Done {
                return Err(TaskError::new(
                    TaskErrorCode::IllegalTransition,
                    "Reopen the task before editing it.",
                ));
            }
            // Editing review requirements invalidates its current review eligibility.
            if task.column() == TaskColumn::InReview {
                return Err(TaskError::new(
                    TaskErrorCode::IllegalTransition,
                    "Request Changes before editing a review submission.",
                ));
            }
            next.skills = draft.skills.clone();
            for binding in &mut next.skills {
                binding.origin = TaskActor::User;
                binding.reason = None;
                binding.created_at = Utc::now();
            }
            next.title = draft.title.trim().into();
            next.description = draft.description.clone();
            next.acceptance_criteria = draft.acceptance_criteria.clone();
            next.priority = draft.priority;
            next.touch();
            next.audit(TaskActor::User, "update", Some(task.column()), None)
        }
        TaskOperation::Transition { action, .. } => {
            let (value, audit) = task.transition(action, TaskActor::User, task.revision, false)?;
            next = value;
            audit
        }
        TaskOperation::Move {
            column, before_id, ..
        } => {
            if task.archived {
                return Err(TaskError::new(
                    TaskErrorCode::IllegalTransition,
                    "Reopen this task before moving it.",
                ));
            }
            let audit = if *column == task.column() {
                next.touch();
                next.audit(TaskActor::User, "reorder", Some(task.column()), None)
            } else {
                // Review and acceptance always use their explicit, evidence-bearing actions.
                let action = match (task.column(), column) {
                    (TaskColumn::Todo, TaskColumn::InProgress) => TaskAction::Start,
                    (TaskColumn::InProgress, TaskColumn::Todo) => TaskAction::ReturnToTodo {
                        reason: "Moved back to Todo by user".into(),
                    },
                    _ => {
                        return Err(TaskError::new(
                            TaskErrorCode::IllegalTransition,
                            "Use Submit for Review, Request Changes, Accept, or Reopen for this move.",
                        ));
                    }
                };
                let (value, audit) =
                    task.transition(&action, TaskActor::User, task.revision, false)?;
                next = value;
                audit
            };
            let mut siblings: Vec<_> = state
                .tasks
                .values()
                .filter(|t| {
                    t.id != task.id
                        && t.project_id == project_id
                        && t.column() == *column
                        && !t.archived
                })
                .cloned()
                .collect();
            siblings.sort_by_key(|t| (t.order_key, t.id.0));
            let index = if let Some(before) = before_id {
                siblings
                    .iter()
                    .position(|t| t.id == *before)
                    .ok_or_else(|| {
                        TaskError::new(
                            TaskErrorCode::RevisionConflict,
                            "The destination card moved. Refresh the board.",
                        )
                    })?
            } else {
                siblings.len()
            };
            // Fixed-point fractional keys; rebalance only when there is no insertion gap.
            let low = if index == 0 {
                0
            } else {
                siblings[index - 1].order_key
            };
            let high = siblings
                .get(index)
                .map(|t| t.order_key)
                .unwrap_or(low.saturating_add(2048));
            if high > low.saturating_add(1) {
                next.order_key = low + (high - low) / 2;
            } else {
                siblings.insert(index, next.clone());
                for (i, mut sibling) in siblings.into_iter().enumerate() {
                    sibling.order_key = (i as i64 + 1) * 1024;
                    if sibling.id == next.id {
                        next.order_key = sibling.order_key;
                    } else {
                        sibling.touch();
                        let audit = sibling.audit(
                            TaskActor::Daemon,
                            "rebalance",
                            Some(sibling.column()),
                            None,
                        );
                        changes.push((sibling, audit));
                    }
                }
            }
            audit
        }
        TaskOperation::Start {
            execution_profile, ..
        } => {
            let (value, mut audit) =
                task.transition(&TaskAction::Start, TaskActor::User, task.revision, false)?;
            next = value;
            next.assigned_agent_id = Some(AgentId::new());
            audit.agent_id = next.assigned_agent_id;
            next.condition = TaskCondition::Queued;
            start = Some((next.clone(), project, execution_profile.clone()));
            audit
        }
        TaskOperation::Link { agent_id, .. } => {
            let record = state
                .agents
                .get(agent_id)
                .ok_or_else(|| TaskError::new(TaskErrorCode::NotFound, "Agent not found."))?;
            if record.run.project_id != project_id
                || record.run.task_id.is_some()
                || record.run.can_stop
                || matches!(
                    record.run.state,
                    AgentState::Starting
                        | AgentState::Working
                        | AgentState::AwaitingApproval
                        | AgentState::Stopping
                )
                || state.children.contains_key(agent_id)
                || state.writer_leases.contains_key(agent_id)
            {
                return Err(TaskError::new(
                    TaskErrorCode::AgentBusy,
                    "Choose an unlinked idle agent in this project.",
                ));
            }
            let (value, _) =
                task.transition(&TaskAction::Start, TaskActor::User, task.revision, false)?;
            next = value;
            next.assigned_agent_id = Some(*agent_id);
            let mut run = record.run.clone();
            run.task_id = Some(task.id);
            linked = Some(run);
            next.audit(TaskActor::User, "link", Some(task.column()), None)
        }
        TaskOperation::Delete { .. } => {
            state
                .store
                .delete_draft_task(&task, request)
                .map_err(|e| TaskError::new(TaskErrorCode::HasHistory, e.to_string()))?;
            state.tasks.remove(&task.id);
            state.broadcast(ServerEvent::TaskDeleted {
                task_id: task.id,
                project_id,
            });
            return Ok((TaskResponse::Deleted(task.id), None));
        }
        _ => unreachable!(),
    };
    let response = TaskResponse::Changed(next.clone());
    changes.push((next.clone(), audit));
    state
        .store
        .save_task_changes(&changes, Some((request, &response)), linked.as_ref())
        .map_err(storage)?;
    if let Some(run) = linked {
        if let Some(record) = state.agents.get_mut(&run.id) {
            record.run = run.clone();
        }
        state.broadcast(ServerEvent::AgentChanged(run));
    }
    state.publish_tasks(changes);
    if next.column() == TaskColumn::InReview {
        state.task_attention(&next);
    }
    Ok((response, start))
}

/// Transport failure after a remote launch is ambiguous. Keep the reservation
/// until a subsequent authoritative host snapshot confirms no writer is active.
fn remote_writer_request(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    request: ClientRequest,
) -> ServerResponse {
    let owner = AgentId::new();
    let guard = match WriterGuard::acquire(state, owner, project, &AgentExecutionProfile::default())
    {
        Ok(guard) => guard,
        Err(e) => return protocol_error("workspace_busy", e),
    };
    let alias = ssh_remote::remote_alias(project).expect("remote project");
    let connections = state
        .lock()
        .expect("runtime lock poisoned")
        .remote_connections
        .clone();
    match connections.request(alias, request) {
        Ok(response) => {
            match &response {
                ServerResponse::AgentStarted(run) if run.can_stop => guard.retain_as(run.id),
                ServerResponse::TaskResponse(TaskResponse::Changed(task))
                    if matches!(
                        task.condition,
                        TaskCondition::Queued
                            | TaskCondition::Running
                            | TaskCondition::AwaitingApproval
                    ) =>
                {
                    if let Some(id) = task.assigned_agent_id {
                        guard.retain_as(id);
                    }
                }
                ServerResponse::Accepted => {
                    state
                        .lock()
                        .expect("runtime lock poisoned")
                        .remote_write_uncertain
                        .insert(owner, alias.into());
                    guard.retain();
                }
                _ => {}
            }
            response
        }
        Err(error) => {
            state
                .lock()
                .expect("runtime lock poisoned")
                .remote_write_uncertain
                .insert(owner, alias.into());
            guard.retain();
            protocol_error(
                "remote_unavailable",
                format!(
                    "Remote writer ownership is being reconciled; refresh before retrying: {error}"
                ),
            )
        }
    }
}
