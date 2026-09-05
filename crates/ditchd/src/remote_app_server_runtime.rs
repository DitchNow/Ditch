// SSH-only runtime adapter for Codex App Server. This file is included in the
// shared runtime module so it can reuse the existing durable agent state and
// event helpers without changing the local Codex execution path.

fn guarded_remote_profile(mut profile: AgentExecutionProfile) -> AgentExecutionProfile {
    profile.approval = AgentApprovalPreset::Ask;
    profile
}

fn start_remote_app_server_session(
    state: Arc<Mutex<RuntimeState>>,
    project_id: ProjectId,
    project_name: String,
    project_root: String,
    resume_thread: Option<String>,
    prompt: String,
    execution_profile: AgentExecutionProfile,
) -> ServerResponse {
    let project = {
        let state = state
            .lock()
            .expect("runtime state lock should not be poisoned");
        if !state.remote_runtime {
            return protocol_error(
                "ssh_app_server_only",
                "Codex App Server launches are accepted only by an SSH runtime",
            );
        }
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

    let execution_profile = guarded_remote_profile(execution_profile);
    let now = Utc::now();
    let mut run = AgentRun {
        id: AgentId::new(),
        provider: AgentProvider::Codex,
        state: AgentState::Starting,
        can_stop: false,
        launch_mode: CodexLaunchMode::Exec,
        execution_profile: execution_profile.clone(),
        project_id: project.id,
        task_id: None,
        pane_id: None,
        native_session_id: resume_thread.clone(),
        codex_title: None,
        user_title: None,
        origin_codex_home: std::env::var("CODEX_HOME").ok(),
        current_prompt: Some(prompt.clone()),
        last_visible_action: Some(if resume_thread.is_some() {
            "Resuming Codex App Server".to_owned()
        } else {
            "Starting Codex App Server".to_owned()
        }),
        state_confidence: 0.9,
        state_evidence: "The SSH runtime accepted an App Server session.".to_owned(),
        started_at: now,
        updated_at: now,
        finished_at: None,
        exit_code: None,
        resume_block_reason: None,
    };
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
    let (event_tx, event_rx) = mpsc::channel();
    let spawned = match codex_app_server::spawn_turn(
        codex_app_server::Launch {
            binary: &binary,
            cwd: &project.root,
            project_id: project.id,
            agent_id: run.id,
            prompt: &prompt,
            resume_thread: resume_thread.as_deref(),
            execution_profile: &execution_profile,
            path: effective_path_for_binary(Path::new(&binary)),
            codex_home: std::env::var_os("CODEX_HOME"),
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
    run.state_evidence = "Codex App Server is running on the SSH host.".to_owned();

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
        if !state.remote_runtime {
            return protocol_error(
                "ssh_app_server_only",
                "Codex App Server prompts are accepted only by an SSH runtime",
            );
        }
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
        (record.run.project_id, record.project_root.clone(), thread_id)
    };
    let execution_profile = guarded_remote_profile(execution_profile);
    let Some(binary) = active_codex_binary(&state) else {
        return protocol_error(
            "codex_not_found",
            "No working Codex CLI installation was found on the SSH host",
        );
    };
    let (event_tx, event_rx) = mpsc::channel();
    let spawned = match codex_app_server::spawn_turn(
        codex_app_server::Launch {
            binary: &binary,
            cwd: &project_root,
            project_id,
            agent_id,
            prompt: &prompt,
            resume_thread: Some(&thread_id),
            execution_profile: &execution_profile,
            path: effective_path_for_binary(Path::new(&binary)),
            codex_home: std::env::var_os("CODEX_HOME"),
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
    thread::spawn(move || {
        for event in events {
            if handle_remote_app_server_event(&event_state, agent_id, run_id, event) {
                break;
            }
        }
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
        finish_agent(&state, agent_id, Some(run_id), code);
    });
}

fn handle_remote_app_server_event(
    state: &Arc<Mutex<RuntimeState>>,
    agent_id: AgentId,
    run_id: uuid::Uuid,
    event: codex_app_server::Event,
) -> bool {
    match event {
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
        codex_app_server::Event::AssistantMessage(text) => append_message(
            state,
            agent_id,
            AgentChatRole::Assistant,
            text,
            Some(run_id),
        ),
        codex_app_server::Event::ToolMessage(text) => append_message(
            state,
            agent_id,
            AgentChatRole::Tool,
            text,
            Some(run_id),
        ),
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
            state.pending_permissions.insert(request.id, request.clone());
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
        let Some(agent_id) = state
            .app_server_permission_agents
            .get(&request_id)
            .copied()
        else {
            return protocol_error("permission_not_found", "approval request expired");
        };
        let Some(control) = state.app_server_turns.get(&agent_id).cloned() else {
            return protocol_error("permission_not_found", "Codex turn is no longer active");
        };
        (agent_id, control)
    };
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
