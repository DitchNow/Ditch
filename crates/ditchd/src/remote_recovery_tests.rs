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

fn wait_for_session(state: &Arc<Mutex<RuntimeState>>, check: impl Fn(&RuntimeState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if check(&state.lock().unwrap()) {
            return;
        }
        assert!(Instant::now() < deadline, "session transition timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn local_and_remote_app_server_approval_rejoin_model_and_stop_lifecycle() {
    for remote in [false, true] {
        let mut runtime = test_runtime();
        runtime.remote_runtime = remote;
        let root = runtime.paths.data_dir.join("workspace");
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let binary = root.join("fake-codex");
        fs::write(&binary, r#"#!/usr/bin/python3
import json, sys, time
if '--version' in sys.argv:
    print('codex-cli 0.154.0'); sys.exit(0)
def read(): return json.loads(sys.stdin.readline())
def send(v): print(json.dumps(v), flush=True)
read(); send({'id':'ditch:initialize','result':{}})
read(); request=read()
with open('requests.jsonl','a') as f: f.write(json.dumps(request)+'\n')
assert request['method'] in ['thread/start','thread/resume']
if request['method']=='thread/resume': assert request['params']['threadId']=='fixture-thread'
send({'id':'ditch:thread','result':{'thread':{'id':'fixture-thread'},'model':request['params']['model']}})
turn=read()
with open('requests.jsonl','a') as f: f.write(json.dumps(turn)+'\n')
prompt=turn['params']['input'][0]['text']
send({'id':'ditch:turn','result':{'turn':{'id':'fixture-turn'}}})
send({'method':'turn/started','params':{'turn':{'id':'fixture-turn'}}})
if prompt=='ask':
    for i in [1,2]:
        send({'id':i,'method':'item/commandExecution/requestApproval','params':{'itemId':str(i),'command':'printf test'}})
        answer=read()
        assert answer['id']==i
        assert answer['result']['decision']==('accept' if i==1 else 'acceptForSession')
        time.sleep(0.2)
        send({'method':'serverRequest/resolved','params':{'requestId':i}})
if prompt=='stop':
    send({'id':3,'method':'item/commandExecution/requestApproval','params':{'itemId':'3','command':'printf stop'}})
    interrupt=read()
    assert interrupt['method']=='turn/interrupt'
    assert interrupt['params']=={'threadId':'fixture-thread','turnId':'fixture-turn'}
    send({'id':interrupt['id'],'result':{}})
if prompt=='cancel':
    send({'id':4,'method':'item/commandExecution/requestApproval','params':{'itemId':'4','command':'printf cancel'}})
    answer=read(); assert answer['result']['decision']=='cancel'
    interrupt=read(); assert interrupt['method']=='turn/interrupt'
    send({'id':interrupt['id'],'result':{}})
send({'method':'item/agentMessage/delta','params':{'delta':'Progress'}})
send({'method':'item/completed','params':{'item':{'type':'agentMessage','id':'message','text':'Fixture done'}}})
send({'method':'turn/completed','params':{'turn':{'status':'interrupted' if prompt in ['stop','cancel'] else 'completed'}}})
time.sleep(0.3) # Keep the thread writer alive briefly after turn completion.
"#).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        runtime.codex_binary = Some(binary.to_string_lossy().into_owned());
        let mut project = Project::new("Lifecycle fixture", &root);
        project.git_policy = ProjectGitPolicy::AllowOutsideGit;
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let state = Arc::new(Mutex::new(runtime));
        let response = handle_request(
            ClientRequest::StartCodexSession {
                project_id: Some(project.id),
                project_name: project.name.clone(),
                project_root: root.to_string_lossy().into_owned(),
                prompt: "ask".into(),
                mode: CodexLaunchMode::Exec,
                execution_profile: AgentExecutionProfile {
                    model: Some("model-a".into()),
                    approval: AgentApprovalPreset::Ask,
                    ..Default::default()
                },
            },
            state.clone(),
        );
        let ServerResponse::AgentStarted(run) = response else {
            panic!("start failed: {response:?}");
        };
        let id = run.id;
        wait_for_session(&state, |s| s.pending_permissions.len() == 1);
        assert!(handle_remote_app_server_event(
            &state,
            id,
            Uuid::new_v4(),
            codex_app_server::Event::Failed("late old failure".into())
        ));
        assert_eq!(state.lock().unwrap().pending_permissions.len(), 1);
        assert!(state.lock().unwrap().app_server_turns.contains_key(&id));
        let first = *state
            .lock()
            .unwrap()
            .pending_permissions
            .keys()
            .next()
            .unwrap();
        assert_eq!(
            respond_permission_for_target(
                state.clone(),
                first,
                codex_app_server::PermissionDecision::ApproveOnce
            ),
            ServerResponse::Accepted
        );
        // Mac/UI reconnect must see a response in flight, not an already-resolved request.
        let ServerResponse::AgentRejoined { permissions, .. } = handle_request(
            ClientRequest::RejoinAgent {
                agent_id: id,
                after_sequence: 0,
            },
            state.clone(),
        ) else {
            panic!("rejoin failed");
        };
        assert!(
            permissions
                .iter()
                .any(|p| p.id == first && p.response_pending)
        );
        assert_eq!(
            state.lock().unwrap().agents[&id].run.state,
            AgentState::AwaitingApproval
        );
        // Replaying a response while confirmation is pending cannot write it twice.
        assert_eq!(
            respond_permission_for_target(
                state.clone(),
                first,
                codex_app_server::PermissionDecision::ApproveOnce
            ),
            ServerResponse::Accepted
        );
        wait_for_session(&state, |s| {
            !s.pending_permissions.contains_key(&first) && s.pending_permissions.len() == 1
        });
        let second = *state
            .lock()
            .unwrap()
            .pending_permissions
            .keys()
            .next()
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(
            respond_permission_for_target(
                state.clone(),
                second,
                codex_app_server::PermissionDecision::ApproveForSession
            ),
            ServerResponse::Accepted
        );
        wait_for_session(&state, |s| s.agents[&id].run.state == AgentState::Completed);
        assert!(state.lock().unwrap().children.contains_key(&id));
        assert_eq!(
            state.lock().unwrap().agents[&id].run.state,
            AgentState::Completed
        );
        for (prompt, approval, model) in [
            ("automatic", AgentApprovalPreset::ApproveForMe, "model-b"),
            ("stop", AgentApprovalPreset::Ask, "model-b"),
            ("cancel", AgentApprovalPreset::FullAccess, "model-a"),
        ] {
            assert_eq!(
                handle_request(
                    ClientRequest::PromptAgent {
                        agent_id: id,
                        prompt: prompt.into(),
                        execution_profile: AgentExecutionProfile {
                            model: Some(model.into()),
                            approval,
                            ..Default::default()
                        }
                    },
                    state.clone()
                ),
                ServerResponse::Accepted
            );
            if prompt == "stop" || prompt == "cancel" {
                wait_for_session(&state, |s| s.pending_permissions.len() == 1);
                if prompt == "stop" {
                    assert_eq!(stop_agent(state.clone(), id), ServerResponse::Accepted);
                } else {
                    let request = *state
                        .lock()
                        .unwrap()
                        .pending_permissions
                        .keys()
                        .next()
                        .unwrap();
                    assert_eq!(
                        respond_permission_for_target(
                            state.clone(),
                            request,
                            codex_app_server::PermissionDecision::Deny
                        ),
                        ServerResponse::Accepted
                    );
                }
            }
            wait_for_session(&state, |s| !s.children.contains_key(&id));
            let guard = state.lock().unwrap();
            assert!(guard.pending_permissions.is_empty());
            assert!(
                !guard
                    .attention
                    .iter()
                    .any(|a| a.kind == AttentionKind::ApprovalRequired)
            );
            assert_eq!(
                guard.agents[&id].run.native_session_id.as_deref(),
                Some("fixture-thread")
            );
            assert_eq!(
                guard.agents[&id].run.execution_profile.model.as_deref(),
                Some(model)
            );
            assert_eq!(
                guard.agents[&id].run.state,
                if prompt == "automatic" {
                    AgentState::Completed
                } else {
                    AgentState::Interrupted
                }
            );
        }
        let requests: Vec<Value> = fs::read_to_string(root.join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        for (index, (policy, model, sandbox)) in [
            ("on-request", "model-a", "workspaceWrite"),
            ("never", "model-b", "workspaceWrite"),
            ("on-request", "model-b", "workspaceWrite"),
            ("never", "model-a", "dangerFullAccess"),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(requests[index * 2]["params"]["approvalPolicy"], policy);
            assert_eq!(requests[index * 2 + 1]["params"]["approvalPolicy"], policy);
            assert_eq!(requests[index * 2 + 1]["params"]["model"], model);
            assert_eq!(
                requests[index * 2 + 1]["params"]["sandboxPolicy"]["type"],
                sandbox
            );
        }
    }
}
