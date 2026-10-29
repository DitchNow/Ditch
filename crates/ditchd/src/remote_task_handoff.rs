// SSH board ownership changes are separate from planning and execution authorization.
const BOARD_HANDOFF_CAPABILITY: &str = "board_handoff_v2";

fn remote_board_capabilities(
    operation: &TaskOperation,
    pending: bool,
    managed: bool,
) -> Vec<&'static str> {
    let primary = match operation {
        TaskOperation::ListReleases | TaskOperation::AcceptRelease { .. } | TaskOperation::AssignRelease { .. } => "task_releases_v1",
        TaskOperation::MaterializeBoardTask { .. } => BOARD_HANDOFF_CAPABILITY,
        TaskOperation::RepositoryDiscovery | TaskOperation::AttachGitHub { .. } => {
            "github_task_metadata_v1"
        }
        TaskOperation::BoardExecution { .. } => "board_execution_v1",
        TaskOperation::Enqueue { .. } | TaskOperation::SelectAgent { .. } => {
            "board_agent_resume_v1"
        }
        TaskOperation::ReorderQueue { .. } => "board_queue_v1",
        TaskOperation::GeneratePlan { .. }
        | TaskOperation::CancelPlan { .. }
        | TaskOperation::UpdateDescription { .. } => "task_planning_v1",
        TaskOperation::CreateIdentified {
            board_task: Some(_),
            ..
        } => "board_workflow_v1",
        TaskOperation::CreateIdentified { .. } => "identified_tasks_v1",
        TaskOperation::List { .. }
        | TaskOperation::Get { .. }
        | TaskOperation::Create { .. }
        | TaskOperation::Update { .. }
        | TaskOperation::Move { .. }
        | TaskOperation::Link { .. }
        | TaskOperation::Delete { .. } => "tasks_v1",
        _ => "acceptance_v1",
    };
    let mut required = vec![primary];
    if pending {
        required.push(BOARD_HANDOFF_CAPABILITY);
    }
    if managed {
        required.push("coordinator_concurrency_v1");
    }
    if matches!(
        operation,
        TaskOperation::Move {
            column: TaskColumn::Todo,
            ..
        } | TaskOperation::Transition {
            action: TaskAction::Prioritize,
            ..
        }
    ) {
        required.push("task_planning_v1");
    }
    required.sort_unstable();
    required.dedup();
    required
}

fn restore_remote_board_action(
    state: &Arc<Mutex<RuntimeState>>,
    request: &TaskRequest,
) -> Result<Option<TaskRequest>, String> {
    let key = format!("board_handoff_action:{}", request.request_id);
    let value = state
        .lock()
        .unwrap()
        .store
        .setting(&key)
        .map_err(|e| e.to_string())?;
    let Some(value) = value else {
        return Ok(None);
    };
    let (original, forwarded): (TaskRequest, TaskRequest) = serde_json::from_str(&value)
        .map_err(|e| format!("Invalid forwarded action receipt: {e}"))?;
    if original != *request {
        return Err("This request ID was already used for a different board action.".into());
    }
    Ok(Some(forwarded))
}

fn handoff_scope(task: &Task) -> Task {
    let mut scope = task.clone();
    scope.remote_pending = false;
    scope.revision = 0;
    scope.order_key = 0;
    scope.created_at = DateTime::<Utc>::default();
    scope.updated_at = None;
    scope.planning.job = None;
    if let Some(source) = &mut scope.github_source {
        // Source identity and project paths remain part of the approved scope.
        source.last_synced_at.clear();
        source.state.clear();
        source.url.clear();
        source.repository.clear();
    }
    scope
}

fn transferable_board_task(task: &Task) -> bool {
    matches!(task.column(), TaskColumn::Backlog | TaskColumn::Todo)
        && task.condition == ditch_core::TaskCondition::Idle
        && !task.archived
        && task.assigned_agent_id.is_none()
        && task.execution_queue.is_none()
        && task.acceptance.attempts.is_empty()
        && task.acceptance.submissions.is_empty()
        && task.acceptance.current_submission.is_none()
        && !task
            .planning
            .job
            .as_ref()
            .is_some_and(|job| job.status == "running")
}

fn materialize_board_task(
    state: &Arc<Mutex<RuntimeState>>,
    request: &TaskRequest,
) -> Option<ServerResponse> {
    let TaskOperation::MaterializeBoardTask {
        task,
        expected_remote,
    } = &request.operation
    else {
        return None;
    };
    let result = (|| -> Result<TaskResponse, ditch_core::TaskError> {
        let mut s = state.lock().unwrap();
        let storage = |e: ditch_store::StoreError| {
            ditch_core::TaskError::new(TaskErrorCode::Storage, e.to_string())
        };
        if let Some(receipt) = s.store.task_replay(request).map_err(storage)? {
            return Ok(receipt);
        }
        ditch_core::TaskDraft {
            issue_kind: task.issue_kind,
            skills: task.skills.clone(),
            title: task.title.clone(),
            description: task.description.clone(),
            acceptance_criteria: task.acceptance_criteria.clone(),
            priority: task.priority,
        }
        .validate()?;
        let valid_project = s.projects.values().any(|p| {
            Some(p.id) == request.project_id
                && p.id == task.project_id
                && !p.is_remote()
                && p.archived_at.is_none()
        });
        if !valid_project
            || !task.remote_pending
            || task.revision == 0
            || task.planning.text.len() > 65536
            || !transferable_board_task(task)
            || task
                .acceptance
                .config
                .criteria
                .iter()
                .any(|c| !matches!(c.kind, ditch_core::CriterionKind::Human))
        {
            return Err(ditch_core::TaskError::new(
                TaskErrorCode::InvalidInput,
                "Only an idle Backlog or Todo task can be transferred to its SSH project.",
            ));
        }
        task.acceptance
            .config
            .validate()
            .map_err(|e| ditch_core::TaskError::new(TaskErrorCode::InvalidInput, e))?;
        let existing = s.tasks.get(&task.id);
        if existing != expected_remote.as_deref()
            || existing.is_some_and(|t| {
                t.project_id != task.project_id || !transferable_board_task(t) || s.task_live(t)
            })
        {
            return Err(ditch_core::TaskError::new(
                TaskErrorCode::RevisionConflict,
                "The SSH task changed or is already active. Refresh the board before retrying; no work was started.",
            ));
        }
        let mut remote = *task.clone();
        remote.remote_pending = false;
        if let Some(existing) = existing {
            remote.revision = remote
                .revision
                .max(existing.revision.checked_add(1).ok_or_else(|| {
                    ditch_core::TaskError::new(
                        TaskErrorCode::InvalidInput,
                        "Task revision limit reached",
                    )
                })?);
        }
        let response = TaskResponse::Materialized(Box::new(remote.clone()));
        let changes = vec![(
            remote.clone(),
            remote.audit(TaskActor::Daemon, "remote_materialized", None, None),
        )];
        // Task and immutable receipt commit together. Neither starts an agent.
        s.store
            .save_task_changes(&changes, Some((request, &response)), None)
            .map_err(storage)?;
        s.publish_tasks(changes);
        Ok(response)
    })();
    Some(ServerResponse::TaskResponse(
        result.unwrap_or_else(TaskResponse::Error),
    ))
}

fn remote_task_reply(response: ServerResponse) -> Result<TaskResponse, String> {
    match response {
        ServerResponse::TaskResponse(TaskResponse::Error(error)) => Err(error.message),
        ServerResponse::TaskResponse(response) => Ok(response),
        _ => {
            Err("The SSH runtime returned an unexpected task response. Reconnect and retry.".into())
        }
    }
}

fn read_remote_board_task(
    send: &impl Fn(TaskRequest) -> Result<ServerResponse, String>,
    project: &Project,
    id: TaskId,
) -> Result<Option<Task>, String> {
    let response = send(TaskRequest {
        project_id: Some(project.id),
        request_id: Uuid::new_v4(),
        operation: TaskOperation::Get { task_id: id },
    })?;
    match response {
        ServerResponse::TaskResponse(TaskResponse::Detail(detail)) => Ok(Some(detail.task)),
        ServerResponse::TaskResponse(TaskResponse::Error(e))
            if e.code == TaskErrorCode::NotFound =>
        {
            Ok(None)
        }
        response => remote_task_reply(response)
            .and_then(|_| Err("Could not inspect the SSH task before transfer.".into())),
    }
}

fn prepare_remote_board_task(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    task_id: TaskId,
    expected_revision: u64,
) -> Result<u64, String> {
    if !project.is_remote() {
        return Ok(expected_revision);
    }
    let task = state
        .lock()
        .unwrap()
        .tasks
        .get(&task_id)
        .cloned()
        .ok_or("Task disappeared")?;
    if !task.remote_pending {
        return Ok(expected_revision);
    }
    if task.revision != expected_revision || !transferable_board_task(&task) {
        return Err("Task changed before SSH transfer. Refresh the board and retry.".into());
    }
    let alias = ssh_remote::remote_alias(project).ok_or("SSH host unavailable")?;
    let connections = state.lock().unwrap().remote_connections.clone();
    match connections.request(alias, ClientRequest::RuntimeStatus) {
        Ok(ServerResponse::RuntimeStatus(status)) if status.capabilities.iter().any(|c| c == BOARD_HANDOFF_CAPABILITY)
            && (task.coordinator_group.is_none() || status.capabilities.iter().any(|c| c == "coordinator_concurrency_v1")) => {},
        Err(_) => return Err("Waiting for SSH. Reconnect and retry; the task is preserved.".into()),
        _ => return Err("Update the SSH runtime: this task requires board_handoff_v2 before it can be transferred.".into()),
    }
    let send = |request| {
        connections
            .request(alias, ClientRequest::TaskRequest(request))
            .map_err(|_| {
                "Waiting for SSH. Reconnect and retry; the pending task and transfer are preserved."
                    .to_string()
            })
    };
    let key = format!("board_handoff_v2:{}", task_id.0);
    let mut journal: Option<TaskRequest> = state
        .lock()
        .unwrap()
        .store
        .setting(&key)
        .map_err(|e| e.to_string())?
        .map(|v| serde_json::from_str(&v))
        .transpose()
        .map_err(|e| format!("Invalid task transfer receipt: {e}"))?
        .flatten();
    if journal.is_none() {
        let existing = read_remote_board_task(&send, project, task_id)?;
        // Recover a legacy partially acknowledged creation only if it is the same scope.
        if existing.as_ref().is_some_and(|remote| {
            !transferable_board_task(remote) || handoff_scope(remote) != handoff_scope(&task)
        }) {
            return Err("A different or active copy of this task exists on SSH. Review both copies before transferring; neither was overwritten.".into());
        }
        journal = Some(new_board_handoff(project, task.clone(), existing));
        let mut s = state.lock().unwrap();
        // A concurrent caller may already have persisted the same task's transfer.
        let saved: Option<TaskRequest> = s
            .store
            .setting(&key)
            .map_err(|e| e.to_string())?
            .map(|v| serde_json::from_str(&v))
            .transpose()
            .map_err(|e| format!("Invalid task transfer receipt: {e}"))?
            .flatten();
        if saved.is_some() {
            journal = saved;
        } else {
            s.store
                .set_setting(&key, &serde_json::to_string(&journal).unwrap())
                .map_err(|e| e.to_string())?;
        }
    }
    // Finish a previous immutable transfer first, then apply any edits made while offline.
    for _ in 0..2 {
        let request = journal.as_ref().unwrap();
        let TaskOperation::MaterializeBoardTask { task: frozen, .. } = &request.operation else {
            return Err("Invalid SSH task transfer operation".into());
        };
        if frozen.id != task_id
            || frozen.project_id != project.id
            || request.project_id != Some(project.id)
        {
            return Err("SSH task transfer identity mismatch".into());
        }
        let TaskResponse::Materialized(receipt) = remote_task_reply(send(request.clone())?)? else {
            return Err("SSH runtime did not acknowledge the versioned task transfer.".into());
        };
        if handoff_scope(&receipt) != handoff_scope(frozen) || receipt.remote_pending {
            return Err(
                "SSH task transfer changed approved content. No execution was authorized.".into(),
            );
        }
        let current = read_remote_board_task(&send, project, task_id)?
            .ok_or("Transferred SSH task was removed. Refresh the board before retrying.")?;
        if !transferable_board_task(&current) || handoff_scope(&current) != handoff_scope(&receipt)
        {
            return Err("The transferred SSH task changed or is already active. Refresh before retrying; no additional agent was started.".into());
        }
        if **frozen != task {
            journal = Some(new_board_handoff(project, task.clone(), Some(current)));
            state
                .lock()
                .unwrap()
                .store
                .set_setting(&key, &serde_json::to_string(&journal).unwrap())
                .map_err(|e| e.to_string())?;
            continue;
        }
        let mut s = state.lock().unwrap();
        if s.tasks.get(&task.id) != Some(&task) {
            return Err("Task changed during SSH transfer. Your edits are preserved; retry to synchronize them.".into());
        }
        let revision = current.revision;
        let changes = vec![(
            current.clone(),
            current.audit(TaskActor::Daemon, "remote_materialized", None, None),
        )];
        s.store
            .save_task_changes(&changes, None, None)
            .map_err(|e| e.to_string())?;
        s.publish_tasks(changes);
        // A leftover journal after a crash is harmless once remote_pending is cleared.
        let _ = s.store.set_setting(&key, "null");
        return Ok(revision);
    }
    Err("Task changed during SSH transfer. Refresh and retry.".into())
}

fn new_board_handoff(project: &Project, task: Task, existing: Option<Task>) -> TaskRequest {
    TaskRequest {
        project_id: Some(project.id),
        request_id: Uuid::new_v4(),
        operation: TaskOperation::MaterializeBoardTask {
            task: Box::new(task),
            expected_remote: existing.map(Box::new),
        },
    }
}

fn remote_board_admission(operation: &TaskOperation) -> Option<(TaskId, u64)> {
    match operation {
        TaskOperation::Start {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::Enqueue {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::SelectAgent {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::GeneratePlan {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::Move {
            task_id,
            expected_revision,
            column: TaskColumn::Todo,
            ..
        }
        | TaskOperation::Transition {
            task_id,
            expected_revision,
            action: TaskAction::Prioritize,
        } => Some((*task_id, *expected_revision)),
        _ => None,
    }
}

fn set_remote_board_revision(operation: &mut TaskOperation, revision: u64) {
    match operation {
        TaskOperation::Start {
            expected_revision, ..
        }
        | TaskOperation::Enqueue {
            expected_revision, ..
        }
        | TaskOperation::SelectAgent {
            expected_revision, ..
        }
        | TaskOperation::GeneratePlan {
            expected_revision, ..
        }
        | TaskOperation::Move {
            expected_revision, ..
        }
        | TaskOperation::Transition {
            expected_revision, ..
        } => *expected_revision = revision,
        _ => {}
    }
}
