// Included in the shared runtime tests: these run in both editions.
pub(super) fn recovery_agent(runtime: &mut RuntimeState) -> AgentId {
    let project = Project::new("Recovery fixture", runtime.paths.data_dir.join("project"));
    runtime.store.upsert_project(&project).unwrap();
    runtime.projects.insert(project.root_key(), project.clone());
    let now = Utc::now();
    let run = AgentRun {
        id: AgentId::new(),
        provider: AgentProvider::Codex,
        state: AgentState::Working,
        can_stop: true,
        launch_mode: CodexLaunchMode::Exec,
        execution_profile: AgentExecutionProfile::default(),
        project_id: project.id,
        task_id: None,
        pane_id: None,
        native_session_id: Some("saved-thread".into()),
        codex_title: None,
        user_title: None,
        origin_codex_home: None,
        current_prompt: Some("original prompt".into()),
        last_visible_action: None,
        state_confidence: 1.0,
        state_evidence: "fixture".into(),
        started_at: now,
        updated_at: now,
        finished_at: None,
        exit_code: None,
        resume_block_reason: None,
    };
    let message = AgentChatMessage {
        agent_id: run.id,
        role: AgentChatRole::User,
        text: "original prompt".into(),
        created_at: now,
    };
    runtime
        .store
        .persist_new_agent(&run, &message, None)
        .unwrap();
    let id = run.id;
    runtime.agents.insert(
        id,
        AgentRecord {
            run,
            project_root: project.root,
            allow_non_git: false,
            messages: vec![message],
            terminal_failure: None,
        },
    );
    id
}

#[test]
fn rejoin_backfills_all_pages_without_launching_or_reprompting() {
    let mut runtime = test_runtime();
    let id = recovery_agent(&mut runtime);
    for index in 0..405 {
        runtime.persist_message(&AgentChatMessage {
            agent_id: id,
            role: AgentChatRole::Assistant,
            text: format!("message {index}"),
            created_at: Utc::now(),
        });
    }
    let state = Arc::new(Mutex::new(runtime));
    let mut cursor = 0;
    let mut collected = Vec::new();
    loop {
        let ServerResponse::AgentRejoined {
            messages,
            next_sequence,
            has_more,
            agent,
            ..
        } = handle_request(
            ClientRequest::RejoinAgent {
                agent_id: id,
                after_sequence: cursor,
            },
            state.clone(),
        )
        else {
            panic!("rejoin failed")
        };
        assert_eq!(agent.native_session_id.as_deref(), Some("saved-thread"));
        assert!(messages.len() <= 200);
        assert!(next_sequence > cursor);
        collected.extend(messages.into_iter().map(|message| message.sequence));
        cursor = next_sequence;
        if !has_more {
            break;
        }
    }
    assert_eq!(collected.len(), 406);
    assert!(collected.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(state.lock().unwrap().children.is_empty());
    assert_eq!(
        state.lock().unwrap().agents[&id]
            .run
            .current_prompt
            .as_deref(),
        Some("original prompt")
    );
}

#[test]
fn operation_receipt_survives_restart_and_rejects_changed_payload() {
    let runtime = test_runtime();
    let paths = runtime.paths.clone();
    let state = Arc::new(Mutex::new(runtime));
    let id = Uuid::new_v4();
    let target = AgentId::new();
    let first = handle_operation(
        id,
        ClientRequest::StopAgent { agent_id: target },
        state.clone(),
    );
    assert_eq!(
        handle_operation(
            id,
            ClientRequest::StopAgent { agent_id: target },
            state.clone()
        ),
        first
    );
    drop(state);
    let state = Arc::new(Mutex::new(RuntimeState::new(paths, false).unwrap()));
    assert_eq!(
        handle_operation(
            id,
            ClientRequest::StopAgent { agent_id: target },
            state.clone()
        ),
        first
    );
    assert!(
        matches!(handle_operation(id, ClientRequest::ForceKillAgent { agent_id: target }, state.clone()), ServerResponse::Error(error) if error.code == "operation_conflict")
    );
    let uncertain = Uuid::new_v4();
    state
        .lock()
        .unwrap()
        .store
        .reserve_operation(uncertain, "hash")
        .unwrap();
    assert!(
        matches!(handle_request(ClientRequest::GetOperationOutcome { request_id: uncertain }, state), ServerResponse::OperationOutcome { state, response: None, .. } if state == "unknown")
    );
}

#[test]
fn event_replay_is_bounded_and_slow_subscribers_are_disconnected() {
    let mut runtime = test_runtime();
    let (tx, _rx) = mpsc::sync_channel(1);
    runtime.subscribers.push(tx);
    for _ in 0..2100 {
        runtime.broadcast(ServerEvent::RuntimeStatusChanged(runtime.runtime_status()));
    }
    assert!(runtime.subscribers.is_empty());
    assert!(runtime.event_replay.len() <= 2048);
    assert!(runtime.event_replay_bytes <= 8 * 1024 * 1024);
    assert_eq!(
        runtime.event_replay.back().unwrap().0,
        runtime.next_sequence - 1
    );
}

#[test]
fn event_subscription_replays_in_order_and_replaces_an_old_epoch() {
    for old_epoch in [false, true] {
        let mut runtime = test_runtime();
        let epoch = runtime.instance_id;
        runtime.broadcast(ServerEvent::RuntimeStatusChanged(runtime.runtime_status()));
        runtime.broadcast(ServerEvent::RuntimeStatusChanged(runtime.runtime_status()));
        let state = Arc::new(Mutex::new(runtime));
        let (writer, reader) = UnixStream::pair().unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let cloned = state.clone();
        let task = thread::spawn(move || {
            subscribe_since(
                writer,
                cloned,
                Some((if old_epoch { Uuid::new_v4() } else { epoch }, 1)),
            )
        });
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["body"]["sequence"], 2);
        assert_eq!(frame["body"]["epoch"], epoch.to_string());
        assert_eq!(
            frame["body"]["event"].get("SnapshotReplaced").is_some(),
            old_epoch
        );
        state.lock().unwrap().subscribers.clear();
        assert!(task.join().unwrap().is_ok());
    }
}

#[test]
fn app_server_drains_final_output_before_processing_child_exit() {
    let mut runtime = test_runtime();
    let id = recovery_agent(&mut runtime);
    let child = Arc::new(Mutex::new(
        Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap(),
    ));
    // The process has already exited before the supervisor sees stdout events.
    child.lock().unwrap().wait().unwrap();
    let run_id = Uuid::new_v4();
    runtime.children.insert(
        id,
        ActiveAgentChild {
            run_id,
            process_group_id: child.lock().unwrap().id() as i32,
            child: child.clone(),
            app_server: true,
        },
    );
    let state = Arc::new(Mutex::new(runtime));
    let (tx, rx) = mpsc::channel();
    tx.send(codex_app_server::Event::AssistantMessage(
        "final answer".into(),
    ))
    .unwrap();
    tx.send(codex_app_server::Event::TurnCompleted {
        status: "completed".into(),
        error: None,
    })
    .unwrap();
    tx.send(codex_app_server::Event::OutputClosed).unwrap();
    drop(tx);
    attach_remote_app_server_turn(state.clone(), id, run_id, child, rx);
    let deadline = Instant::now() + Duration::from_secs(2);
    while state.lock().unwrap().children.contains_key(&id) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let guard = state.lock().unwrap();
    assert!(!guard.children.contains_key(&id));
    assert_eq!(guard.agents[&id].run.state, AgentState::Completed);
    assert!(
        guard.agents[&id]
            .messages
            .iter()
            .any(|message| message.text == "final answer")
    );
}

#[test]
fn remote_daemon_has_one_owner_and_reconnect_reuses_live_socket() {
    let mut runtime = test_runtime();
    runtime.paths.socket_path = PathBuf::from(format!("/tmp/ditch-{}.sock", Uuid::new_v4()));
    let owner = remote_daemon_lock(&runtime.paths).unwrap();
    assert_eq!(
        remote_daemon_lock(&runtime.paths).unwrap_err().kind(),
        io::ErrorKind::AddrInUse
    );
    let listener = UnixListener::bind(&runtime.paths.socket_path).unwrap();
    ensure_remote_daemon(&runtime.paths).unwrap();
    // Still the original listener; bootstrap did not remove or replace it.
    listener.set_nonblocking(true).unwrap();
    assert!(listener.accept().is_ok());
    drop(owner);
    assert!(remote_daemon_lock(&runtime.paths).is_ok());
    drop(listener);
    fs::remove_file(&runtime.paths.socket_path).unwrap();
    fs::remove_file(runtime.paths.socket_path.with_extension("lock")).unwrap();
}

#[test]
fn removed_registered_project_cannot_fall_back_to_a_local_launch() {
    let runtime = Arc::new(Mutex::new(test_runtime()));
    let response = handle_request(
        ClientRequest::StartCodexSession {
            project_id: Some(ProjectId::new()),
            project_name: "removed remote".into(),
            project_root: "/tmp".into(),
            prompt: "must not execute".into(),
            mode: CodexLaunchMode::Exec,
            execution_profile: AgentExecutionProfile::default(),
        },
        runtime.clone(),
    );
    assert!(matches!(response, ServerResponse::Error(e) if e.code == "project_not_found"));
    assert!(runtime.lock().unwrap().children.is_empty());
}

#[test]
fn another_ssh_host_cannot_replace_an_existing_agent_identity() {
    let mut runtime = test_runtime();
    let agent = recovery_agent(&mut runtime);
    let original = runtime.agents[&agent].run.clone();
    let project = ssh_remote::wrap_remote_project(
        Project::new("Other host", "/srv/other"),
        "other-host",
        Uuid::new_v4(),
    );
    runtime.projects.insert(project.root_key(), project.clone());
    let mut conflicting = original.clone();
    conflicting.project_id = project.id;
    forward_remote_event_locked(
        &mut runtime,
        "other-host",
        ServerEvent::AgentChanged(conflicting.clone()),
    );
    assert_eq!(runtime.agents[&agent].run.project_id, original.project_id);
    let state = Arc::new(Mutex::new(runtime));
    assert!(!cache_remote_agent(
        &state,
        "other-host",
        &project,
        &conflicting
    ));
    assert!(
        !state
            .lock()
            .unwrap()
            .remote_agent_hosts
            .contains_key(&agent)
    );
}
