// The job is retained by the runtime and persisted on its task. No UI owns it.
fn task_planning_request(
    state: &Arc<Mutex<RuntimeState>>,
    request: &TaskRequest,
) -> Option<ServerResponse> {
    let task_id = match &request.operation {
        TaskOperation::GeneratePlan { task_id, .. }
        | TaskOperation::CancelPlan { task_id, .. }
        | TaskOperation::UpdateDescription { task_id, .. } => *task_id,
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
            .get(&task_id)
            .filter(|t| Some(t.project_id) == request.project_id)
            .cloned()
            .ok_or_else(|| TaskError::new(TaskErrorCode::NotFound, "Task not found"))?;
        let project = s
            .projects
            .values()
            .find(|p| p.id == task.project_id && p.archived_at.is_none())
            .cloned()
            .ok_or_else(|| TaskError::new(TaskErrorCode::NotFound, "Project unavailable"))?;
        if !matches!(request.operation, TaskOperation::CancelPlan { .. })
            && (task.archived
                || matches!(task.column(), TaskColumn::Done | TaskColumn::InReview)
                || s.task_live(&task))
        {
            return Err(TaskError::new(
                TaskErrorCode::IllegalTransition,
                "Stop or reopen this task before changing its description or plan.",
            ));
        }
        let mut launch = None;
        let action = match &request.operation {
            TaskOperation::UpdateDescription {
                expected_description,
                expected_plan,
                description,
                plan,
                ..
            } => {
                if description.len() > 65536 || plan.len() > 65536 {
                    return Err(TaskError::new(
                        TaskErrorCode::InvalidInput,
                        "Description and plan must each be at most 64 KiB.",
                    ));
                }
                if task.description != *expected_description || task.planning.text != *expected_plan
                {
                    return Err(TaskError::new(
                        TaskErrorCode::RevisionConflict,
                        "Description changed elsewhere. Reopen the task to review the latest version; your draft is still here.",
                    ));
                }
                task.description = description.clone();
                task.planning.text = plan.clone();
                "description_updated"
            }
            TaskOperation::GeneratePlan {
                expected_revision,
                model,
                ..
            } => {
                task.check_revision(*expected_revision)?;
                let prepared =
                    prepare_task_plan(&s, &project, &task, request.request_id, model.clone())?;
                task.planning.job = Some(prepared.job.clone());
                launch = Some(prepared);
                "plan_started"
            }
            TaskOperation::CancelPlan { job_id, .. } => {
                let job = task
                    .planning
                    .job
                    .as_mut()
                    .filter(|j| j.id == *job_id)
                    .ok_or_else(|| {
                        TaskError::new(TaskErrorCode::NotFound, "Planning job no longer exists")
                    })?;
                if job.status == "running" {
                    job.status = "canceled".into();
                    job.finished_at = Some(Utc::now());
                }
                "plan_canceled"
            }
            _ => unreachable!(),
        };
        task.touch();
        let response = TaskResponse::Changed(task.clone());
        let changes = vec![(
            task.clone(),
            task.audit(TaskActor::User, action, None, None),
        )];
        s.store
            .save_task_changes(&changes, Some((request, &response)), None)
            .map_err(storage)?;
        s.publish_tasks(changes);
        drop(s);
        if let Some(launch) = launch {
            launch_task_plan(state, task.project_id, task_id, launch);
        }
        Ok(response)
    })();
    Some(ServerResponse::TaskResponse(
        result.unwrap_or_else(TaskResponse::Error),
    ))
}

struct TaskPlanLaunch {
    context: serde_json::Value,
    binary: String,
    home: Option<PathBuf>,
    model: Option<String>,
    root: PathBuf,
    job: ditch_core::TaskPlanJob,
}

fn new_task_plan_job(task: &Task, id: Uuid) -> ditch_core::TaskPlanJob {
    ditch_core::TaskPlanJob {
        id,
        status: "running".into(),
        title: task.title.clone(),
        description: task.description.clone(),
        previous_plan: task.planning.text.clone(),
        started_at: Utc::now(),
        finished_at: None,
        result: None,
        error: None,
    }
}

fn prepare_task_plan(
    state: &RuntimeState,
    project: &Project,
    task: &Task,
    id: Uuid,
    model: Option<String>,
) -> Result<TaskPlanLaunch, TaskError> {
    if task
        .planning
        .job
        .as_ref()
        .is_some_and(|j| j.status == "running")
    {
        return Err(TaskError::new(
            TaskErrorCode::AgentBusy,
            "A plan is already being generated.",
        ));
    }
    if state
        .tasks
        .values()
        .filter(|t| {
            t.planning
                .job
                .as_ref()
                .is_some_and(|j| j.status == "running")
        })
        .count()
        >= 4
    {
        return Err(TaskError::new(
            TaskErrorCode::AgentBusy,
            "Four plans are already running. Try again when one finishes.",
        ));
    }
    if project.is_remote() {
        return Err(TaskError::new(
            TaskErrorCode::Unsupported,
            "Connect to this project's runtime to generate a plan.",
        ));
    }
    let binary = state.codex_binary.clone().ok_or_else(|| {
        TaskError::new(
            TaskErrorCode::InvalidInput,
            "Set up Codex before generating a plan.",
        )
    })?;
    if model.as_ref().is_some_and(|m| m.len() > 200) {
        return Err(TaskError::new(TaskErrorCode::InvalidInput, "Invalid model"));
    }
    Ok(TaskPlanLaunch {
        context: serde_json::json!({"type":task.issue_kind,"github_source":task.github_source}),
        binary,
        home: state.codex_home.clone(),
        model,
        root: project.root.clone(),
        job: new_task_plan_job(task, id),
    })
}

// Called while committing Backlog -> Todo, so retries cannot start a second job.
// A planning failure must not undo the user's board move.
fn prepare_automatic_task_plan(
    state: &RuntimeState,
    project: &Project,
    task: &mut Task,
) -> Option<TaskPlanLaunch> {
    // Transitions only fill a missing plan. Existing or separately retained
    // generated plans can be replaced only by an explicit GeneratePlan request.
    if !task.planning.text.trim().is_empty()
        || task.planning.job.as_ref().is_some_and(|j| {
            j.status == "running"
                || j.result
                    .as_ref()
                    .is_some_and(|text| !text.trim().is_empty())
        })
    {
        return None;
    }
    let id = Uuid::new_v4();
    match prepare_task_plan(state, project, task, id, None) {
        Ok(launch) => {
            task.planning.job = Some(launch.job.clone());
            Some(launch)
        }
        Err(error) => {
            let mut job = new_task_plan_job(task, id);
            job.status = "failed".into();
            job.finished_at = Some(Utc::now());
            job.error = Some(error.message);
            task.planning.job = Some(job);
            None
        }
    }
}

fn launch_task_plan(
    state: &Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    task_id: TaskId,
    launch: TaskPlanLaunch,
) {
    let runtime = Arc::clone(state);
    let job_id = launch.job.id;
    let spawned = thread::Builder::new()
        .name("task-planning".into())
        .spawn(move || {
            let active = || {
                let s = runtime.lock().unwrap();
                s.projects
                    .values()
                    .any(|p| p.id == project_id && p.archived_at.is_none())
                    && s.tasks.get(&task_id).is_some_and(|t| {
                        !t.archived
                            && t.planning
                                .job
                                .as_ref()
                                .is_some_and(|j| j.id == job_id && j.status == "running")
                    })
            };
            let result = task_planner::generate(
                &launch.binary,
                &launch.root,
                launch.home.as_deref(),
                launch.model.as_deref(),
                &launch.job.title,
                &launch.job.description,
                &launch.context,
                active,
            );
            finish_task_plan(&runtime, task_id, job_id, result);
        });
    if spawned.is_err() {
        finish_task_plan(
            state,
            task_id,
            job_id,
            Err(io::Error::other("Could not start planning")),
        );
    }
}

fn finish_task_plan(
    state: &Arc<Mutex<RuntimeState>>,
    task_id: TaskId,
    job_id: Uuid,
    result: io::Result<String>,
) {
    let mut s = state.lock().unwrap();
    let Some(mut task) = s.tasks.get(&task_id).cloned() else {
        return;
    };
    let editable = !task.archived
        && !matches!(task.column(), TaskColumn::Done | TaskColumn::InReview)
        && !s.task_live(&task);
    let Some(job) = task
        .planning
        .job
        .as_mut()
        .filter(|j| j.id == job_id && j.status == "running")
    else {
        return;
    };
    job.finished_at = Some(Utc::now());
    match result {
        Ok(plan) => {
            job.status = "completed".into();
            // Never overwrite a user edit, even when it happens during generation.
            if editable && task.planning.text == job.previous_plan {
                task.planning.text = plan.clone();
            }
            job.result = Some(plan);
        }
        Err(_) => {
            job.status = "failed".into();
            // Provider errors may include secrets or project data; retain a safe message.
            job.error = Some(
                "Could not generate a plan. Check your Codex sign-in and model, then retry.".into(),
            );
        }
    }
    task.touch();
    let changes = vec![(
        task.clone(),
        task.audit(TaskActor::Daemon, "plan_finished", None, None),
    )];
    if let Err(e) = s.store.save_task_changes(&changes, None, None) {
        eprintln!("Could not persist task planning result: {e}");
        task.planning.text = s.tasks[&task_id].planning.text.clone();
        let job = task.planning.job.as_mut().unwrap();
        job.status = "failed".into();
        job.error = Some(
            if job.result.is_some() {
                "Could not save the plan. Your result is available below; use it and save again."
            } else {
                "Could not save planning status. Please try again."
            }
            .into(),
        );
        // Keep the result available to the open app without pretending it is durable.
        s.tasks.insert(task.id, task.clone());
        s.broadcast(ServerEvent::TaskChanged(task));
        return;
    }
    s.publish_tasks(changes);
}
