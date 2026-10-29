// A durable project-local queue. No coordinator or Ditchmaster settings are involved.
fn board_execution(
    s: &RuntimeState,
    project_id: ProjectId,
) -> Result<ditch_core::BoardExecutionState, String> {
    s.store
        .setting(&format!("ditch.board.execution.{project_id:?}"))
        .map_err(|e| e.to_string())?
        .map(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))
        // Preserve the existing automatic-queue behavior until explicitly paused.
        .unwrap_or_else(|| {
            Ok(ditch_core::BoardExecutionState {
                project_id,
                paused: false,
                revision: 0,
            })
        })
}

fn board_execution_request(
    state: &Arc<Mutex<RuntimeState>>,
    request: &TaskRequest,
) -> Option<ServerResponse> {
    let TaskOperation::BoardExecution {
        paused,
        expected_revision,
    } = &request.operation
    else {
        return None;
    };
    let result = (|| -> Result<TaskResponse, TaskError> {
        let mut s = state.lock().unwrap();
        let storage =
            |e: ditch_store::StoreError| TaskError::new(TaskErrorCode::Storage, e.to_string());
        if let Some(replay) = s.store.task_replay(request).map_err(storage)? {
            return Ok(replay);
        }
        let project = request
            .project_id
            .and_then(|id| {
                s.projects
                    .values()
                    .find(|p| p.id == id && !p.is_remote() && p.archived_at.is_none())
            })
            .cloned()
            .ok_or_else(|| {
                TaskError::new(
                    TaskErrorCode::NotFound,
                    "Choose an available project to control its queue.",
                )
            })?;
        let mut execution = board_execution(&s, project.id)
            .map_err(|e| TaskError::new(TaskErrorCode::Storage, e))?;
        if let Some(paused) = paused {
            if *expected_revision != Some(execution.revision) {
                return Err(TaskError::new(
                    TaskErrorCode::RevisionConflict,
                    "Board execution changed elsewhere. Refresh its status and try again.",
                ));
            }
            if execution.paused != *paused {
                execution.paused = *paused;
                execution.revision = execution.revision.checked_add(1).ok_or_else(|| {
                    TaskError::new(
                        TaskErrorCode::InvalidInput,
                        "Board execution revision limit reached",
                    )
                })?;
            }
            let response = TaskResponse::BoardExecution(execution.clone());
            s.store
                .set_task_setting_receipt(
                    &format!("ditch.board.execution.{:?}", project.id),
                    &serde_json::to_string(&execution).unwrap(),
                    request,
                    &response,
                )
                .map_err(storage)?;
            s.broadcast(ServerEvent::BoardExecutionChanged(execution));
            return Ok(response);
        }
        Ok(TaskResponse::BoardExecution(execution))
    })();
    Some(ServerResponse::TaskResponse(
        result.unwrap_or_else(TaskResponse::Error),
    ))
}

fn board_agent_idle(s: &RuntimeState, id: AgentId) -> bool {
    !s.children.contains_key(&id)
        && !s.writer_leases.contains_key(&id)
        && !s.acceptance_owners.contains(&id)
        && s.agents.get(&id).is_some_and(|a| {
            !a.run.can_stop
                && !matches!(
                    a.run.state,
                    AgentState::Starting
                        | AgentState::Working
                        | AgentState::Stopping
                        | AgentState::AwaitingApproval
                )
        })
}

fn board_agent_thread(
    s: &RuntimeState,
    task: &Task,
    id: AgentId,
) -> Result<Option<String>, String> {
    let run = &s
        .agents
        .get(&id)
        .ok_or("The selected agent is unavailable. Move the task to Todo to choose another agent.")?
        .run;
    if run.project_id != task.project_id || run.coordinator_group.is_some() {
        return Err("Choose a user-managed agent from this project".into());
    }
    if run.task_id.is_some_and(|other| other != task.id) {
        return Err(
            "The selected agent belongs to another task. Return to Todo to reassign it.".into(),
        );
    }
    if task.continue_agent_id.is_some() && run.native_session_id.is_none() {
        return Err("The selected conversation has no resumable thread. Move the task to Todo to choose another agent.".into());
    }
    if run.resume_block_reason.is_some()
        && !(run.native_session_id.is_none()
            && task.continue_agent_id.is_none()
            && run.resume_block_reason == Some(AgentResumeBlockReason::NoCodexThread))
    {
        return Err(
            "This agent cannot resume. Move the task to Todo to choose another agent.".into(),
        );
    }
    let home = s
        .codex_home
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());
    if run.origin_codex_home != home {
        return Err("This agent belongs to a different Codex account. Move the task to Todo to choose another agent.".into());
    }
    Ok(run.native_session_id.clone())
}

fn board_queue_request(
    state: &Arc<Mutex<RuntimeState>>,
    request: &TaskRequest,
) -> Option<ServerResponse> {
    let (id, revision) = match &request.operation {
        TaskOperation::Enqueue {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::ReorderQueue {
            task_id,
            expected_revision,
            ..
        }
        | TaskOperation::SelectAgent {
            task_id,
            expected_revision,
            ..
        } => (*task_id, *expected_revision),
        TaskOperation::Transition {
            task_id,
            expected_revision,
            action: TaskAction::RequestChanges { .. },
        } if state
            .lock()
            .unwrap()
            .tasks
            .get(task_id)
            .is_some_and(|t| t.execution_queue.is_some()) =>
        {
            (*task_id, *expected_revision)
        }
        _ => return None,
    };
    let result = (|| -> Result<TaskResponse, TaskError> {
        let mut s = state.lock().unwrap();
        let storage =
            |e: ditch_store::StoreError| TaskError::new(TaskErrorCode::Storage, e.to_string());
        if let Some(replay) = s.store.task_replay(request).map_err(storage)? {
            return Ok(replay);
        }
        let mut task = s
            .tasks
            .get(&id)
            .filter(|t| Some(t.project_id) == request.project_id)
            .cloned()
            .ok_or_else(|| TaskError::new(TaskErrorCode::NotFound, "Task not found"))?;
        task.check_revision(revision)?;
        if !s
            .projects
            .values()
            .any(|p| p.id == task.project_id && !p.is_remote() && p.archived_at.is_none())
        {
            return Err(TaskError::new(
                TaskErrorCode::Unsupported,
                "Use the project's connected runtime to queue work.",
            ));
        }
        if task.archived || task.coordinator_group.is_some() || s.task_live(&task) {
            return Err(TaskError::new(
                TaskErrorCode::AgentBusy,
                "Only idle user-managed tasks can enter or change this queue.",
            ));
        }
        let mut changes = Vec::new();
        let mut agent_changes = Vec::new();
        match &request.operation {
            TaskOperation::SelectAgent {
                agent_id,
                replace_task_id,
                replace_task_revision,
                ..
            } => {
                if task.column() != TaskColumn::Todo {
                    return Err(TaskError::new(
                        TaskErrorCode::IllegalTransition,
                        "Move the task to Todo before changing its agent.",
                    ));
                }
                let previous = task.continue_agent_id.or(task.assigned_agent_id);
                if previous
                    .is_some_and(|id| s.agents.contains_key(&id) && !board_agent_idle(&s, id))
                {
                    return Err(TaskError::new(
                        TaskErrorCode::AgentBusy,
                        "Stop the current agent before changing it.",
                    ));
                }
                if let Some(id) = agent_id {
                    let mut run = s
                        .agents
                        .get(id)
                        .ok_or_else(|| {
                            TaskError::new(TaskErrorCode::NotFound, "Agent unavailable")
                        })?
                        .run
                        .clone();
                    if !board_agent_idle(&s, *id)
                        || run.project_id != task.project_id
                        || run.coordinator_group.is_some()
                    {
                        return Err(TaskError::new(
                            TaskErrorCode::AgentBusy,
                            "Choose a stopped user-managed agent in this project.",
                        ));
                    }
                    let other = run.task_id.filter(|other| *other != task.id);
                    if let Some(other_id) = other {
                        let mut other = s.tasks.get(&other_id).cloned().ok_or_else(|| {
                            TaskError::new(
                                TaskErrorCode::NotFound,
                                "The agent's current task is unavailable",
                            )
                        })?;
                        if other.column() != TaskColumn::Todo
                            || other.archived
                            || other.coordinator_group.is_some()
                            || s.task_live(&other)
                        {
                            return Err(TaskError::new(
                                TaskErrorCode::AgentBusy,
                                "The agent is locked to another task. Return that task to Todo first.",
                            ));
                        }
                        if *replace_task_id != Some(other.id)
                            || *replace_task_revision != Some(other.revision)
                        {
                            return Err(TaskError::new(
                                TaskErrorCode::RevisionConflict,
                                "Confirm reassignment from the agent's current Todo task.",
                            ));
                        }
                        other.assigned_agent_id = None;
                        other.continue_agent_id = None;
                        other.execution_queue = None;
                        other.touch();
                        let audit = other.audit(
                            TaskActor::User,
                            "agent_detached",
                            None,
                            Some("Reassigned to another Todo task".into()),
                        );
                        changes.push((other, audit));
                    } else if replace_task_id.is_some() || replace_task_revision.is_some() {
                        return Err(TaskError::new(
                            TaskErrorCode::RevisionConflict,
                            "Agent assignment changed. Reopen the selector.",
                        ));
                    }
                    // Check resume identity before changing either side of the binding.
                    let home = s
                        .codex_home
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned());
                    if run.native_session_id.is_none()
                        || run.resume_block_reason.is_some()
                        || run.origin_codex_home != home
                    {
                        return Err(TaskError::new(
                            TaskErrorCode::InvalidInput,
                            "This agent has no resumable conversation under the current Codex account.",
                        ));
                    }
                    run.task_id = Some(task.id);
                    agent_changes.push(run);
                }
                if let Some(id) = previous.filter(|old| Some(*old) != *agent_id) {
                    if let Some(record) =
                        s.agents.get(&id).filter(|a| a.run.task_id == Some(task.id))
                    {
                        let mut run = record.run.clone();
                        run.task_id = None;
                        agent_changes.push(run);
                    }
                }
                task.assigned_agent_id = *agent_id;
                task.continue_agent_id = *agent_id;
                task.execution_queue = None;
                task.touch();
                let audit = task.audit(TaskActor::User, "agent_selected", None, None);
                changes.push((task.clone(), audit));
            }
            TaskOperation::Enqueue {
                execution_profile, ..
            } => {
                if !matches!(task.column(), TaskColumn::Todo | TaskColumn::InProgress)
                    || task.waiting_in_queue()
                {
                    return Err(TaskError::new(
                        TaskErrorCode::IllegalTransition,
                        "Move a Todo task into In Progress to queue it.",
                    ));
                }
                let position = s
                    .tasks
                    .values()
                    .filter(|t| t.project_id == task.project_id)
                    .filter_map(|t| t.execution_queue.as_ref().map(|q| q.position))
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| {
                        TaskError::new(TaskErrorCode::InvalidInput, "Queue position limit reached")
                    })?;
                let from = task.column();
                let retry = task.column() == TaskColumn::InProgress;
                let agent_id = if retry {
                    task.execution_queue
                        .as_ref()
                        .and_then(|q| q.agent_id)
                        .or(task.continue_agent_id)
                        .or(task.assigned_agent_id)
                } else {
                    task.continue_agent_id.or(task.assigned_agent_id)
                };
                let mut profile = if retry {
                    task.execution_queue
                        .as_ref()
                        .map(|q| q.profile.clone())
                        .unwrap_or_else(|| execution_profile.clone())
                } else {
                    execution_profile.clone()
                };
                profile.transport = ditch_core::AgentTransport::AppServer;
                profile.skills = task.skills.clone();
                let feedback = task
                    .execution_queue
                    .as_ref()
                    .and_then(|q| q.feedback.clone());
                task.execution_queue = Some(ditch_core::TaskExecutionQueue {
                    agent_id,
                    position,
                    profile,
                    feedback,
                });
                task.state = ditch_core::TaskState::Running;
                task.condition = TaskCondition::Queued;
                task.assigned_agent_id = None;
                task.acceptance.current_submission = None;
                task.last_reason = None;
                task.touch();
                let audit = task.audit(TaskActor::User, "queue", Some(from), None);
                changes.push((task.clone(), audit));
            }
            TaskOperation::Transition {
                action: action @ TaskAction::RequestChanges { feedback },
                ..
            } => {
                let (mut next, audit) =
                    task.transition(action, TaskActor::User, task.revision, false)?;
                let position = s
                    .tasks
                    .values()
                    .filter(|t| t.project_id == task.project_id)
                    .filter_map(|t| t.execution_queue.as_ref().map(|q| q.position))
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| {
                        TaskError::new(TaskErrorCode::InvalidInput, "Queue position limit reached")
                    })?;
                let queue = next.execution_queue.as_mut().unwrap();
                queue.agent_id = queue
                    .agent_id
                    .or(task.continue_agent_id)
                    .or(task.assigned_agent_id);
                queue.position = position;
                queue.feedback = Some(feedback.clone());
                next.condition = TaskCondition::Queued;
                next.assigned_agent_id = None;
                next.acceptance.current_submission = None;
                changes.push((next.clone(), audit));
                task = next;
            }
            TaskOperation::ReorderQueue { before_id, .. } => {
                if !task.waiting_in_queue() || *before_id == Some(id) {
                    return Err(TaskError::new(
                        TaskErrorCode::IllegalTransition,
                        "Only waiting tasks can be reordered.",
                    ));
                }
                let mut pending: Vec<_> = s
                    .tasks
                    .values()
                    .filter(|t| {
                        t.project_id == task.project_id && t.waiting_in_queue() && t.id != id
                    })
                    .cloned()
                    .collect();
                pending.sort_by_key(|t| t.execution_queue.as_ref().unwrap().position);
                let index =
                    match before_id {
                        Some(before) => pending.iter().position(|t| t.id == *before).ok_or_else(
                            || {
                                TaskError::new(
                                    TaskErrorCode::RevisionConflict,
                                    "The destination is no longer waiting in this project's queue.",
                                )
                            },
                        )?,
                        None => pending.len(),
                    };
                pending.insert(index, task.clone());
                let base = s
                    .tasks
                    .values()
                    .filter(|t| t.project_id == task.project_id && !t.waiting_in_queue())
                    .filter_map(|t| t.execution_queue.as_ref().map(|q| q.position))
                    .max()
                    .unwrap_or(0);
                for (index, mut next) in pending.into_iter().enumerate() {
                    let position = base.checked_add(index as u64 + 1).ok_or_else(|| {
                        TaskError::new(TaskErrorCode::InvalidInput, "Queue position limit reached")
                    })?;
                    if next.execution_queue.as_ref().unwrap().position != position || next.id == id
                    {
                        next.execution_queue.as_mut().unwrap().position = position;
                        next.touch();
                        let audit = next.audit(
                            TaskActor::User,
                            "queue_reorder",
                            Some(TaskColumn::InProgress),
                            None,
                        );
                        if next.id == id {
                            task = next.clone();
                        }
                        changes.push((next, audit));
                    }
                }
            }
            _ => unreachable!(),
        }
        let response = TaskResponse::Changed(task);
        s.store
            .save_task_agent_changes(&changes, Some((request, &response)), &agent_changes)
            .map_err(storage)?;
        for run in agent_changes {
            if let Some(record) = s.agents.get_mut(&run.id) {
                record.run = run.clone();
            }
            s.broadcast(ServerEvent::AgentChanged(run));
        }
        s.publish_tasks(changes);
        Ok(response)
    })();
    Some(ServerResponse::TaskResponse(
        result.unwrap_or_else(TaskResponse::Error),
    ))
}

fn board_workspace(project: &Project) -> (String, PathBuf) {
    let target = match &project.execution_target {
        ProjectExecutionTarget::Local => "local".into(),
        ProjectExecutionTarget::Remote {
            remote_machine_id, ..
        } => remote_machine_id.to_string(),
    };
    let root = if project.is_remote() {
        project.root.clone()
    } else {
        fs::canonicalize(&project.root).unwrap_or_else(|_| project.root.clone())
    };
    let root = if cfg!(target_os = "macos") && !project.is_remote() {
        PathBuf::from(root.to_string_lossy().to_lowercase())
    } else {
        root
    };
    (target, root)
}

fn board_task_owns_agent(s: &RuntimeState, task: &Task, id: AgentId) -> bool {
    task.execution_queue.is_some()
        && (task.assigned_agent_id == Some(id)
            || task
                .execution_queue
                .as_ref()
                .is_some_and(|q| q.agent_id == Some(id))
            || s.agents
                .get(&id)
                .is_some_and(|a| a.run.task_id == Some(task.id))
            || s.acceptance_controls
                .get(&task.id)
                .is_some_and(|c| c.owner == id))
}

fn board_agent_owned(s: &RuntimeState, id: AgentId) -> bool {
    s.tasks
        .values()
        .any(|task| board_task_owns_agent(s, task, id))
}

// Shared by queue claims and writer admission. Task status can change before its
// worker/validator exits, so durable reservations and live ownership both count.
fn board_execution_busy(s: &RuntimeState, project: &Project, except: Option<AgentId>) -> bool {
    let workspace = board_workspace(project);
    s.tasks.values().any(|task| {
        if task.execution_queue.is_none()
            || !s
                .projects
                .values()
                .any(|p| p.id == task.project_id && board_workspace(p) == workspace)
        {
            return false;
        }
        let owns = |id: AgentId| Some(id) != except && board_task_owns_agent(s, task, id);
        // The assigned ID consumes launch authorization before any process exists.
        (task.column() == TaskColumn::InProgress
            && !task.archived
            && task.assigned_agent_id.is_some_and(|id| Some(id) != except)
            && matches!(
                task.condition,
                TaskCondition::Queued | TaskCondition::Running | TaskCondition::AwaitingApproval
            ))
            || s.acceptance_controls
                .get(&task.id)
                .is_some_and(|c| Some(c.owner) != except)
            || s.acceptance_owners.iter().any(|id| owns(*id))
            || s.writer_leases.keys().any(|id| owns(*id))
            || s.children.keys().any(|id| owns(*id))
            || s.agents.values().any(|a| {
                owns(a.run.id)
                    && (a.run.can_stop
                        || matches!(
                            a.run.state,
                            AgentState::Starting
                                | AgentState::Working
                                | AgentState::Stopping
                                | AgentState::AwaitingApproval
                        ))
            })
    })
}

// Selection and reservation are one transaction under the same lock as manual starts.
// A consumed authorization is never launched again after a restart or launch failure.
fn claim_board_task(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
) -> Result<Option<Task>, String> {
    let mut s = state.lock().unwrap();
    s.reap_orphan_writers();
    if s.remote_maintenance_active()
        || board_execution(&s, project.id)?.paused
        || !s
            .projects
            .values()
            .any(|p| p.id == project.id && p.archived_at.is_none() && !p.is_remote())
    {
        return Ok(None);
    }
    let Some(mut task) = s
        .tasks
        .values()
        .filter(|t| t.project_id == project.id && t.waiting_in_queue())
        .min_by_key(|t| t.execution_queue.as_ref().unwrap().position)
        .cloned()
    else {
        return Ok(None);
    };
    let selected = task
        .execution_queue
        .as_ref()
        .and_then(|q| q.agent_id)
        .or(task.continue_agent_id);
    let reason =
        if selected.is_some_and(|id| s.agents.contains_key(&id) && !board_agent_idle(&s, id)) {
            Some("Waiting for the selected agent to become idle".to_string())
        } else if board_execution_busy(&s, project, None) {
            Some("Waiting for another board task in this project".to_string())
        } else if task
            .planning
            .job
            .as_ref()
            .is_some_and(|j| j.status == "running")
        {
            Some("Waiting for implementation plan".to_string())
        } else if task.planning.text.trim().is_empty() {
            // A generation attempt never invalidates the saved, user-editable plan.
            Some("Add an implementation plan to run this task".to_string())
        } else {
            None
        };
    if reason.is_some() {
        if task.last_reason != reason {
            task.last_reason = reason;
            task.touch();
            let audit = task.audit(
                TaskActor::Daemon,
                "queue_waiting",
                None,
                task.last_reason.clone(),
            );
            let changes = vec![(task, audit)];
            s.store
                .save_task_changes(&changes, None, None)
                .map_err(|e| e.to_string())?;
            s.publish_tasks(changes);
        }
        return Ok(None);
    }
    let owner = task
        .execution_queue
        .as_ref()
        .and_then(|q| q.agent_id)
        .or(task.continue_agent_id)
        .unwrap_or_default();
    if s.agents.contains_key(&owner) || task.continue_agent_id == Some(owner) {
        let resume = board_agent_thread(&s, &task, owner);
        if let Err(reason) = resume {
            task.condition = TaskCondition::Blocked;
            task.last_reason = Some(reason);
            task.touch();
            let audit = task.audit(
                TaskActor::Daemon,
                "queue_resume_blocked",
                None,
                task.last_reason.clone(),
            );
            let changes = vec![(task, audit)];
            s.store
                .save_task_changes(&changes, None, None)
                .map_err(|e| e.to_string())?;
            s.publish_tasks(changes);
            return Ok(None);
        }
        if resume.ok().flatten().is_some() {
            task.continue_agent_id = Some(owner);
        }
    }
    task.execution_queue.as_mut().unwrap().agent_id = Some(owner);
    task.assigned_agent_id = Some(owner);
    task.started_at = Some(Utc::now());
    task.last_reason = task
        .execution_queue
        .as_ref()
        .and_then(|q| q.feedback.clone());
    task.touch();
    let audit = task.audit(TaskActor::Daemon, "queue_dispatch", None, None);
    let changes = vec![(task.clone(), audit)];
    s.store
        .save_task_changes(&changes, None, None)
        .map_err(|e| e.to_string())?;
    s.publish_tasks(changes);
    Ok(Some(task))
}

fn launch_board_task(state: &Arc<Mutex<RuntimeState>>, project: Project, task: Task) {
    let mut profile = task.execution_queue.as_ref().unwrap().profile.clone();
    profile.skills = task.skills.clone();
    // Uses the normal linked-agent launcher, plus the board's existing review evidence.
    // No coordinator ownership or coordinator settings are introduced.
    begin_acceptance(Arc::clone(state), task, project, profile, false);
}

fn start_board_queue(state: &Arc<Mutex<RuntimeState>>) {
    let weak = Arc::downgrade(state);
    thread::spawn(move || {
        loop {
            let Some(state) = weak.upgrade() else {
                break;
            };
            let projects: Vec<_> = state
                .lock()
                .unwrap()
                .projects
                .values()
                .filter(|p| !p.is_remote() && p.archived_at.is_none())
                .cloned()
                .collect();
            for project in projects {
                match claim_board_task(&state, &project) {
                    Ok(Some(task)) => {
                        let state = Arc::clone(&state);
                        thread::spawn(move || launch_board_task(&state, project, task));
                    }
                    Ok(None) => {}
                    Err(error) => eprintln!("Could not dispatch board queue: {error}"),
                }
            }
            drop(state);
            thread::sleep(Duration::from_secs(1));
        }
    });
}
