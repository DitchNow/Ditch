// Shared daemon lifecycle for local and SSH App Server turns.

fn guarded_remote_profile(mut profile: AgentExecutionProfile) -> AgentExecutionProfile {
    profile.approval = AgentApprovalPreset::Ask;
    profile
}

#[allow(clippy::too_many_arguments)]
fn start_remote_app_server_session(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    project_name: String,
    project_root: String,
    resume_thread: Option<String>,
    prompt: String,
    execution_profile: AgentExecutionProfile,
) -> ServerResponse {
    if !state.lock().unwrap().remote_runtime {
        return protocol_error(
            "ssh_app_server_only",
            "Use the local App Server launch route for a local project",
        );
    }
    start_remote_app_server_session_linked(
        state,
        project_id,
        project_name,
        project_root,
        resume_thread,
        prompt,
        execution_profile,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn start_remote_app_server_session_linked(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    project_name: String,
    project_root: String,
    resume_thread: Option<String>,
    prompt: String,
    execution_profile: AgentExecutionProfile,
    binding: Option<(TaskId, AgentId)>,
) -> ServerResponse {
    let project = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(project) = state
            .projects
            .values()
            .find(|project| project.id == project_id)
            .cloned()
        else {
            return protocol_error("project_not_found", "remote project was not found");
        };
        project
    };
    if project.name != project_name
        || project.root != canonical_project_root(Path::new(&project_root))
    {
        return protocol_error(
            "remote_project_mismatch",
            "the SSH project identity does not match the remote runtime",
        );
    }
    if let Some(thread_id) = resume_thread.as_deref()
        && let Err(message) = validate_thread_project(&state, thread_id, &project.root)
    {
        return protocol_error("codex_thread_project_mismatch", message);
    }
    if let Err(error) = ensure_project_metadata(&project) {
        return protocol_error("project_metadata_failed", error.to_string());
    }
    if let Err(error) = verify_project_git_policy(&project) {
        return error;
    }

    let remote = state.lock().unwrap().remote_runtime;
    let mut execution_profile = if remote {
        guarded_remote_profile(execution_profile)
    } else {
        execution_profile
    };
    execution_profile.transport = ditch_core::AgentTransport::AppServer;
    let now = Utc::now();
    let mut run = AgentRun {
        id: binding.map(|b| b.1).unwrap_or_default(),
        provider: AgentProvider::Codex,
        state: AgentState::Starting,
        can_stop: false,
        launch_mode: CodexLaunchMode::Exec,
        execution_profile: execution_profile.clone(),
        project_id: project.id,
        task_id: binding.map(|b| b.0),
        pane_id: None,
        native_session_id: resume_thread.clone(),
        codex_title: None,
        user_title: None,
        origin_codex_home: state
            .lock()
            .unwrap()
            .codex_home
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        current_prompt: Some(prompt.clone()),
        last_visible_action: Some(if resume_thread.is_some() {
            "Resuming Codex App Server".to_owned()
        } else {
            "Starting Codex App Server".to_owned()
        }),
        state_confidence: 0.9,
        state_evidence: "The runtime accepted an App Server session.".to_owned(),
        started_at: now,
        updated_at: now,
        finished_at: None,
        exit_code: None,
        resume_block_reason: None,
    };
    let writer = match WriterGuard::acquire(&state, run.id, &project, &execution_profile) {
        Ok(w) => w,
        Err(e) => return protocol_error("workspace_busy", e),
    };
    if let Err(error) = validate_selected_skills(&state, &project, &execution_profile.skills) {
        return protocol_error("skill_unavailable", error);
    }
    let extra_roots = selected_skill_roots(&execution_profile.skills);
    let allow_non_git = project.git_policy == ProjectGitPolicy::AllowOutsideGit;
    let user_message = AgentChatMessage {
        agent_id: run.id,
        role: AgentChatRole::User,
        text: prompt.clone(),
        created_at: Utc::now(),
    };
    let Some(binary) = active_codex_binary(&state) else {
        return persist_launch_failure(
            &state,
            project,
            run,
            user_message,
            allow_non_git,
            "No working Codex CLI installation was found on the SSH host.".to_owned(),
        );
    };
    let codex_home = state
        .lock()
        .unwrap()
        .codex_home
        .as_ref()
        .map(|p| p.as_os_str().to_owned());
    let (event_tx, event_rx) = mpsc::channel();
    let restricted = edition::restricted_agent(&state.lock().unwrap(), run.id);
    let protected_roots = {let s=state.lock().unwrap();let mut roots=vec![s.paths.data_dir.clone()];if let Some(home)=s.codex_home.clone().or_else(||std::env::var_os("HOME").map(|h|PathBuf::from(h).join(".codex"))){roots.push(home);}roots};
    let spawned = match codex_app_server::spawn_turn(
        codex_app_server::Launch {
            restricted,
            protected_roots,
            remote,
            extra_roots: &extra_roots,
            on_spawn: Some(&|pid| writer.started(pid)),
            binary: &binary,
            cwd: &project.root,
            project_id: project.id,
            agent_id: run.id,
            prompt: &prompt,
            resume_thread: resume_thread.as_deref(),
            execution_profile: &execution_profile,
            codex_home,
        },
        event_tx,
    ) {
        Ok(spawned) => spawned,
        Err(error) => {
            return persist_launch_failure(
                &state,
                project,
                run,
                user_message,
                allow_non_git,
                format!("Failed to start Codex App Server: {error}"),
            );
        }
    };
    let process_group_id = spawned
        .child
        .lock()
        .expect("child lock should not be poisoned")
        .id() as i32;
    let run_id = uuid::Uuid::new_v4();
    run.state = AgentState::Working;
    run.can_stop = true;
    run.updated_at = Utc::now();
    run.state_evidence = "Codex App Server is running.".to_owned();

    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let home = state
            .codex_home
            .as_ref()
            .map(|value| value.to_string_lossy().into_owned());
        if let Err(error) = state
            .store
            .persist_new_agent(&run, &user_message, home.as_deref())
        {
            let _ = spawned
                .child
                .lock()
                .expect("child lock should not be poisoned")
                .kill();
            return protocol_error("agent_store_failed", error.to_string());
        }
        state.agents.insert(
            run.id,
            AgentRecord {
                run: run.clone(),
                project_root: project.root.clone(),
                allow_non_git,
                messages: vec![user_message.clone()],
                terminal_failure: None,
            },
        );
        state.children.insert(
            run.id,
            ActiveAgentChild {
                run_id,
                process_group_id,
                child: Arc::clone(&spawned.child),
                app_server: true,
            },
        );
        state.app_server_turns.insert(run.id, spawned.control);
        state.broadcast(ServerEvent::AgentChanged(run.clone()));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }
    writer.retain();
    attach_remote_app_server_turn(state, run.id, run_id, spawned.child, event_rx);
    ServerResponse::AgentStarted(run)
}

fn prompt_remote_app_server_agent(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    prompt: String,
    execution_profile: AgentExecutionProfile,
) -> ServerResponse {
    let (project_id, project_root, thread_id) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(record) = state.agents.get(&agent_id) else {
            return protocol_error("agent_not_found", "agent session was not found");
        };
        if state.children.contains_key(&agent_id) {
            return protocol_error("agent_busy", "agent session is already working");
        }
        let Some(thread_id) = record.run.native_session_id.clone() else {
            return protocol_error(
                "agent_not_resumable",
                "Codex App Server never created a thread for this agent",
            );
        };
        (
            record.run.project_id,
            record.project_root.clone(),
            thread_id,
        )
    };
    let remote = state.lock().unwrap().remote_runtime;
    let mut execution_profile = if remote {
        guarded_remote_profile(execution_profile)
    } else {
        execution_profile
    };
    execution_profile.transport = ditch_core::AgentTransport::AppServer;
    let Some(project) = project_by_id(&state, project_id) else {
        return protocol_error("project_not_found", "Project not registered");
    };
    let writer = match WriterGuard::acquire(&state, agent_id, &project, &execution_profile) {
        Ok(w) => w,
        Err(e) => return protocol_error("workspace_busy", e),
    };
    let Some(binary) = active_codex_binary(&state) else {
        return protocol_error(
            "codex_not_found",
            "No working Codex CLI installation was found on the SSH host",
        );
    };
    execution_profile.skills = state.lock().unwrap().agents[&agent_id]
        .run
        .execution_profile
        .skills
        .clone();
    if let Err(error) = validate_selected_skills(&state, &project, &execution_profile.skills) {
        return protocol_error("skill_unavailable", error);
    }
    let extra_roots = selected_skill_roots(&execution_profile.skills);
    let codex_home = state
        .lock()
        .unwrap()
        .codex_home
        .as_ref()
        .map(|p| p.as_os_str().to_owned());
    let (event_tx, event_rx) = mpsc::channel();
    let restricted = edition::restricted_agent(&state.lock().unwrap(), agent_id);
    let protected_roots = {let s=state.lock().unwrap();let mut roots=vec![s.paths.data_dir.clone()];if let Some(home)=s.codex_home.clone().or_else(||std::env::var_os("HOME").map(|h|PathBuf::from(h).join(".codex"))){roots.push(home);}roots};
    let spawned = match codex_app_server::spawn_turn(
        codex_app_server::Launch {
            restricted,
            protected_roots,
            remote,
            extra_roots: &extra_roots,
            on_spawn: Some(&|pid| writer.started(pid)),
            binary: &binary,
            cwd: &project_root,
            project_id,
            agent_id,
            prompt: &prompt,
            resume_thread: Some(&thread_id),
            execution_profile: &execution_profile,
            codex_home,
        },
        event_tx,
    ) {
        Ok(spawned) => spawned,
        Err(error) => {
            let message = format!("Failed to start Codex App Server: {error}");
            record_terminal_failure(&state, agent_id, message.clone(), None);
            return protocol_error("codex_start_failed", message);
        }
    };
    let user_message = AgentChatMessage {
        agent_id,
        role: AgentChatRole::User,
        text: prompt.clone(),
        created_at: Utc::now(),
    };
    let process_group_id = spawned
        .child
        .lock()
        .expect("child lock should not be poisoned")
        .id() as i32;
    let run_id = uuid::Uuid::new_v4();
    {
        let mut state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(record) = state.agents.get_mut(&agent_id) else {
            let _ = spawned.child.lock().map(|mut child| child.kill());
            return protocol_error("agent_not_found", "agent session was not found");
        };
        record.run.state = AgentState::Working;
        record.run.can_stop = true;
        record.run.execution_profile = execution_profile;
        record.run.current_prompt = Some(prompt);
        record.run.last_visible_action = Some("Prompt sent to Codex App Server".to_owned());
        record.run.updated_at = Utc::now();
        record.run.finished_at = None;
        record.run.exit_code = None;
        record.run.resume_block_reason = None;
        record.terminal_failure = None;
        record.messages.push(user_message.clone());
        let run = record.run.clone();
        state.persist_message(&user_message);
        state.persist_agent(agent_id);
        state.children.insert(
            agent_id,
            ActiveAgentChild {
                run_id,
                process_group_id,
                child: Arc::clone(&spawned.child),
                app_server: true,
            },
        );
        state.app_server_turns.insert(agent_id, spawned.control);
        state.broadcast(ServerEvent::AgentChanged(run));
        state.broadcast(ServerEvent::AgentMessageAppended(user_message));
    }
    writer.retain();
    attach_remote_app_server_turn(state, agent_id, run_id, spawned.child, event_rx);
    ServerResponse::Accepted
}

fn attach_remote_app_server_turn(
    state: Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: uuid::Uuid,
    child: Arc<Mutex<Child>>,
    events: mpsc::Receiver<codex_app_server::Event>,
) {
    let event_state = Arc::clone(&state);
    let (drained_tx, drained_rx) = mpsc::channel();
    thread::spawn(move || {
        for event in events {
            if handle_remote_app_server_event(&event_state, agent_id, run_id, event) {
                break;
            }
        }
        let _ = drained_tx.send(());
    });
    thread::spawn(move || {
        let code = loop {
            match child
                .lock()
                .expect("child lock should not be poisoned")
                .try_wait()
            {
                Ok(Some(status)) => break status.code(),
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(_) => break None,
            }
        };
        let _ = drained_rx.recv_timeout(Duration::from_secs(20));
        finish_agent(&state, agent_id, Some(run_id), code);
    });
}

fn handle_remote_app_server_event(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: uuid::Uuid,
    event: codex_app_server::Event,
) -> bool {
    let cleanup_state = Arc::clone(state);
    match event {
        codex_app_server::Event::SkillsChanged => {
            let mut locked = state.lock().unwrap();
            if !is_current_run(&locked, agent_id, run_id) {
                return true;
            }
            let project_id = locked.agents.get(&agent_id).map(|r| r.run.project_id);
            locked.broadcast(ServerEvent::SkillsChanged { project_id });
        }
        codex_app_server::Event::ProviderUsage(usage) => {
            let mut locked = state.lock().unwrap();
            if !is_current_run(&locked, agent_id, run_id) {
                return false;
            }
            if let Some(id) = locked.agents.get(&agent_id).and_then(|r| r.run.task_id)
                && let Some(mut task) = locked.tasks.get(&id).cloned()
                && let Some(attempt) = task
                    .acceptance
                    .attempts
                    .last_mut()
                    .filter(|a| a.result.active())
            {
                attempt.provider_usage = Some(usage);
                let _ = acceptance_save(&mut locked, task, "provider_usage");
            }
        }
        codex_app_server::Event::Title(title) => {
            let mut locked = state.lock().unwrap();
            if !is_current_run(&locked, agent_id, run_id) {
                return true;
            }
            if let Some(record) = locked.agents.get_mut(&agent_id) {
                record.run.codex_title = Some(title);
                let run = record.run.clone();
                locked.persist_agent(agent_id);
                locked.broadcast(ServerEvent::AgentChanged(run));
            }
        }
        codex_app_server::Event::AssistantDelta(text) => {
            let mut locked = state.lock().unwrap();
            if !is_current_run(&locked, agent_id, run_id) {
                return true;
            }
            let stream = locked.app_server_streams.entry(agent_id).or_default();
            if stream.len() < 65536 {
                stream.extend(text.chars().take(65536 - stream.len()));
            }
            let text = stream.clone();
            locked.broadcast(ServerEvent::AgentStreamingText { agent_id, text });
        }
        codex_app_server::Event::ThreadReady(thread_id) => {
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if !is_current_run(&state, agent_id, run_id) {
                return true;
            }
            let Some(record) = state.agents.get_mut(&agent_id) else {
                return true;
            };
            record.run.native_session_id = Some(thread_id);
            record.run.resume_block_reason = None;
            record.run.updated_at = Utc::now();
            let run = record.run.clone();
            state.persist_agent(agent_id);
            state.broadcast(ServerEvent::AgentChanged(run));
        }
        codex_app_server::Event::TurnStarted => start_agent_turn(state, agent_id, Some(run_id)),
        codex_app_server::Event::Action(action) => {
            update_agent_action(state, agent_id, &action, Some(run_id));
        }
        codex_app_server::Event::AssistantMessage(text) => {
            {
                let mut locked = state.lock().unwrap();
                if !is_current_run(&locked, agent_id, run_id) {
                    return true;
                }
                locked.app_server_streams.remove(&agent_id);
                locked.broadcast(ServerEvent::AgentStreamingText {
                    agent_id,
                    text: String::new(),
                });
            }
            append_message(
                state,
                agent_id,
                AgentChatRole::Assistant,
                text,
                Some(run_id),
            );
        }
        codex_app_server::Event::ToolMessage(text) => {
            append_message(state, agent_id, AgentChatRole::Tool, text, Some(run_id))
        }
        codex_app_server::Event::ApprovalRequested(request) => {
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if !is_current_run(&state, agent_id, run_id) {
                return true;
            }
            state
                .app_server_permission_agents
                .insert(request.id, agent_id);
            state
                .pending_permissions
                .insert(request.id, request.clone());
            let Some(record) = state.agents.get_mut(&agent_id) else {
                return true;
            };
            record.run.state = AgentState::AwaitingApproval;
            record.run.last_visible_action = Some(request.summary.clone());
            record.run.updated_at = Utc::now();
            let run = record.run.clone();
            state.persist_agent(agent_id);
            state.broadcast(ServerEvent::AgentChanged(run));
            state.broadcast(ServerEvent::PermissionRequested(request));
        }
        codex_app_server::Event::TurnCompleted { status, error } => {
            if let Some(error) = error {
                record_terminal_failure(state, agent_id, error, Some(run_id));
            }
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if !is_current_run(&state, agent_id, run_id) {
                return true;
            }
            if let Some(active) = state.children.get(&agent_id) {
                let group = active.process_group_id;
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(500));
                    let locked = cleanup_state.lock().unwrap();
                    if is_current_run(&locked, agent_id, run_id) && process_group_exists(group) {
                        signal_process_group(group, libc::SIGKILL);
                    }
                });
            }
            state.app_server_streams.remove(&agent_id);
            state.broadcast(ServerEvent::AgentStreamingText {
                agent_id,
                text: String::new(),
            });
            state.app_server_turns.remove(&agent_id);
            state
                .app_server_permission_agents
                .retain(|_, owner| *owner != agent_id);
            state
                .pending_permissions
                .retain(|_, request| request.agent_id != Some(agent_id));
            let Some(record) = state.agents.get_mut(&agent_id) else {
                return true;
            };
            record.run.can_stop = false;
            record.run.updated_at = Utc::now();
            record.run.finished_at = Some(record.run.updated_at);
            record.run.state = match status.as_str() {
                "completed" => AgentState::Completed,
                "interrupted" => AgentState::Interrupted,
                _ => AgentState::Failed,
            };
            record.run.exit_code = Some(if record.run.state == AgentState::Completed {
                0
            } else {
                1
            });
            record.run.last_visible_action = Some(
                match record.run.state {
                    AgentState::Completed => "Turn completed",
                    AgentState::Interrupted => "Turn interrupted",
                    _ => "Turn failed",
                }
                .to_owned(),
            );
            let run = record.run.clone();
            state.persist_agent(agent_id);
            state.broadcast(ServerEvent::AgentChanged(run));
            return true;
        }
        codex_app_server::Event::Diagnostic(message) => {
            log_codex_diagnostic(state, agent_id, "app-server", &message);
        }
        codex_app_server::Event::Failed(message) => {
            record_terminal_failure(state, agent_id, message, Some(run_id));
            let mut state = state
                .lock()
                .expect("runtime state lock should not be poisoned");
            if !is_current_run(&state, agent_id, run_id) {
                return true;
            }
            if let Some(active) = state.children.get(&agent_id) {
                let group = active.process_group_id;
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(500));
                    let locked = cleanup_state.lock().unwrap();
                    if is_current_run(&locked, agent_id, run_id) && process_group_exists(group) {
                        signal_process_group(group, libc::SIGKILL);
                    }
                });
            }
            state.app_server_streams.remove(&agent_id);
            state.broadcast(ServerEvent::AgentStreamingText {
                agent_id,
                text: String::new(),
            });
            state.app_server_turns.remove(&agent_id);
            state
                .app_server_permission_agents
                .retain(|_, owner| *owner != agent_id);
            state
                .pending_permissions
                .retain(|_, request| request.agent_id != Some(agent_id));
            return true;
        }
    }
    false
}

fn respond_permission_for_target(
    state: Arc<Mutex<RuntimeState>>,
    request_id: uuid::Uuid,
    decision: codex_app_server::PermissionDecision,
) -> ServerResponse {
    let remote = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        state
            .remote_permission_hosts
            .get(&request_id)
            .cloned()
            .map(|alias| (alias, state.remote_connections.clone()))
    };
    if let Some((alias, connections)) = remote {
        let request = match decision {
            codex_app_server::PermissionDecision::ApproveOnce => {
                ClientRequest::ApprovePermission { request_id }
            }
            codex_app_server::PermissionDecision::ApproveForSession => {
                ClientRequest::ApprovePermissionForSession { request_id }
            }
            codex_app_server::PermissionDecision::Deny => ClientRequest::DenyPermission {
                request_id,
                reason: "Denied by user".to_owned(),
            },
        };
        return match connections.request(&alias, request) {
            Ok(response) => {
                if matches!(response, ServerResponse::Accepted) {
                    let mut state = state
                        .lock()
                        .expect("runtime state lock should not be poisoned");
                    state.remote_permission_hosts.remove(&request_id);
                    state.pending_permissions.remove(&request_id);
                }
                response
            }
            Err(error) => protocol_error("remote_unavailable", error.to_string()),
        };
    }

    let (agent_id, control) = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        let Some(agent_id) = state.app_server_permission_agents.get(&request_id).copied() else {
            return protocol_error("permission_not_found", "approval request expired");
        };
        let Some(control) = state.app_server_turns.get(&agent_id).cloned() else {
            return protocol_error("permission_not_found", "Codex turn is no longer active");
        };
        (agent_id, control)
    };
    if decision != codex_app_server::PermissionDecision::Deny {
        let mut locked = state.lock().unwrap();
        let Some(mut lease) = locked.writer_leases.get(&agent_id).cloned() else {
            return protocol_error("workspace_busy", "Writer reservation is missing");
        };
        // An explicit approval may grant filesystem/network permissions beyond
        // the base sandbox. Reserve exclusive ownership before delivering it.
        if !lease.unconfined {
            lease.unconfined = true;
            if locked
                .writer_leases
                .iter()
                .any(|(id, other)| *id != agent_id && lease.conflicts(other))
            {
                return protocol_error(
                    "workspace_busy",
                    "This approval may expand access. Stop other writers before approving, or deny this request.",
                );
            }
            if let Err(error) = locked.store.update_writer_lease(agent_id, &lease) {
                return protocol_error("writer_store_failed", error.to_string());
            }
            locked.writer_leases.insert(agent_id, lease);
        }
    }
    {
        let mut locked = state.lock().unwrap();
        let task_id = locked.agents.get(&agent_id).and_then(|r| r.run.task_id);
        if let Some(task_id) = task_id {
            if decision == codex_app_server::PermissionDecision::Deny
                && let Some(control) = locked.acceptance_controls.get(&task_id)
            {
                control.cancel.store(true, Ordering::Release);
                let _ = control.wake.send(());
            }
            if let Some(mut task) = locked.tasks.get(&task_id).cloned()
                && let Some(attempt) = task
                    .acceptance
                    .attempts
                    .last_mut()
                    .filter(|a| a.result.active())
            {
                attempt
                    .approvals
                    .push(format!("{:?}: {}", decision, request_id));
                let _ = acceptance_save(&mut locked, task, "attempt_approval");
            }
        }
    }
    if let Err(error) = control.respond(request_id, decision) {
        return protocol_error("permission_response_failed", error.to_string());
    }
    let mut state = state
        .lock()
        .expect("runtime state lock should not be poisoned");
    state.app_server_permission_agents.remove(&request_id);
    state.pending_permissions.remove(&request_id);
    if let Some(record) = state.agents.get_mut(&agent_id) {
        record.run.state = AgentState::Working;
        record.run.last_visible_action = Some(
            match decision {
                codex_app_server::PermissionDecision::ApproveOnce => "Permission granted once",
                codex_app_server::PermissionDecision::ApproveForSession => {
                    "Permission granted for this session"
                }
                codex_app_server::PermissionDecision::Deny => "Permission denied",
            }
            .to_owned(),
        );
        record.run.updated_at = Utc::now();
        let run = record.run.clone();
        state.persist_agent(agent_id);
        state.broadcast(ServerEvent::AgentChanged(run));
    }
    ServerResponse::Accepted
}
