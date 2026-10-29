mod task_queue_tests {
    use super::*;

    fn fixture() -> (Arc<Mutex<RuntimeState>>, Project) {
        let mut s = super::tests::test_runtime();
        let p = project(&mut s, "First");
        (Arc::new(Mutex::new(s)), p)
    }
    fn project(s: &mut RuntimeState, name: &str) -> Project {
        let root = s.paths.data_dir.join(name);
        fs::create_dir_all(&root).unwrap();
        let mut p = Project::new(name, fs::canonicalize(root).unwrap());
        p.git_policy = ProjectGitPolicy::AllowOutsideGit;
        s.store.upsert_project(&p).unwrap();
        s.projects.insert(p.root_key(), p.clone());
        p
    }
    fn task(s: &Arc<Mutex<RuntimeState>>, p: &Project, title: &str) -> Task {
        let mut task = Task::new(
            p.id,
            ditch_core::TaskDraft {
                issue_kind: Default::default(),
                skills: vec![],
                title: title.into(),
                description: "Original requirement".into(),
                acceptance_criteria: vec![],
                priority: ditch_core::TaskPriority::Normal,
            },
            1,
        )
        .unwrap();
        task.planning.text = "Inspect the code, implement, test.".into();
        let mut s = s.lock().unwrap();
        s.store
            .save_task_changes(
                &[(
                    task.clone(),
                    task.audit(TaskActor::User, "create", None, None),
                )],
                None,
                None,
            )
            .unwrap();
        s.tasks.insert(task.id, task.clone());
        task
    }
    fn request(t: &Task, operation: TaskOperation) -> TaskRequest {
        TaskRequest {
            project_id: Some(t.project_id),
            request_id: Uuid::new_v4(),
            operation,
        }
    }
    fn changed(r: ServerResponse) -> Task {
        match r {
            ServerResponse::TaskResponse(TaskResponse::Changed(t)) => t,
            r => panic!("{r:?}"),
        }
    }
    fn enqueue(s: &Arc<Mutex<RuntimeState>>, t: &Task) -> Task {
        changed(handle_task_request(
            s.clone(),
            request(
                t,
                TaskOperation::Enqueue {
                    task_id: t.id,
                    expected_revision: t.revision,
                    execution_profile: AgentExecutionProfile::default(),
                },
            ),
        ))
    }

    fn stopped_agent(s: &Arc<Mutex<RuntimeState>>, p: &Project) -> AgentId {
        let mut s = s.lock().unwrap();
        let now = Utc::now();
        let run = AgentRun {
            coordinator_group: None,
            id: AgentId::new(),
            provider: AgentProvider::Codex,
            state: AgentState::Interrupted,
            can_stop: false,
            launch_mode: CodexLaunchMode::Exec,
            execution_profile: AgentExecutionProfile::default(),
            project_id: p.id,
            task_id: None,
            pane_id: None,
            native_session_id: Some("existing-thread".into()),
            codex_title: Some("Original title".into()),
            naming_failed: false,
            user_title: Some("My agent".into()),
            origin_codex_home: s
                .codex_home
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            current_prompt: Some("Earlier conversation".into()),
            last_visible_action: None,
            state_confidence: 1.0,
            state_evidence: "fixture".into(),
            started_at: now,
            updated_at: now,
            finished_at: Some(now),
            exit_code: None,
            resume_block_reason: None,
        };
        let message = AgentChatMessage {
            agent_id: run.id,
            role: AgentChatRole::User,
            text: "Earlier conversation".into(),
            created_at: now,
        };
        s.store
            .persist_new_agent(&run, &message, run.origin_codex_home.as_deref())
            .unwrap();
        let id = run.id;
        s.agents.insert(
            id,
            AgentRecord {
                run,
                messages: vec![message],
                project_root: p.root.clone(),
                terminal_failure: None,
            },
        );
        id
    }

    fn select_request(t: &Task, id: Option<AgentId>, other: Option<&Task>) -> TaskRequest {
        request(
            t,
            TaskOperation::SelectAgent {
                task_id: t.id,
                expected_revision: t.revision,
                agent_id: id,
                replace_task_id: other.map(|t| t.id),
                replace_task_revision: other.map(|t| t.revision),
            },
        )
    }

    #[test]
    fn direct_followup_admission_preserves_inactive_links_and_ownership() {
        let (s, p) = fixture();
        let id = stopped_agent(&s, &p);
        let mut linked = task(&s, &p, "Historical work");
        linked.assigned_agent_id = Some(id);
        let profile = AgentExecutionProfile::default();
        s.lock().unwrap().agents.get_mut(&id).unwrap().run.task_id = Some(linked.id);
        for state in [ditch_core::TaskState::InReview, ditch_core::TaskState::Accepted,
                      ditch_core::TaskState::Ready, ditch_core::TaskState::Backlog] {
            for archived in [false, true] {
                linked.state = state.clone();
                linked.archived = archived;
                s.lock().unwrap().tasks.insert(linked.id, linked.clone());
                let writer = WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).unwrap();
                assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).is_err());
                s.lock().unwrap().sync_agent_task(id);
                assert_eq!(s.lock().unwrap().tasks[&linked.id], linked);
                drop(writer);
                assert!(WriterGuard::acquire(&s, id, &p, &profile).is_err());
            }
        }
        {
            let mut locked = s.lock().unwrap();
            locked.agents.get_mut(&id).unwrap().run.state = AgentState::Failed;
            locked.persist_agent(id);
            locked.store.save_task_changes(&[(linked.clone(), linked.audit(TaskActor::User, "fixture", None, None))], None, None).unwrap();
        }
        let restored = Arc::new(Mutex::new(RuntimeState::new(s.lock().unwrap().paths.clone(), false).unwrap()));
        assert!(WriterGuard::acquire_direct_prompt(&restored, id, &p, &profile).is_ok());
        assert_eq!(restored.lock().unwrap().agents[&id].run.task_id, Some(linked.id));
        linked.coordinator_group = Some(Uuid::new_v4());
        s.lock().unwrap().tasks.insert(linked.id, linked.clone());
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).is_err());
        linked.coordinator_group = None;
        s.lock().unwrap().tasks.insert(linked.id, linked.clone());
        s.lock().unwrap().acceptance_owners.insert(id);
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).is_err());
        s.lock().unwrap().acceptance_owners.remove(&id);
        s.lock().unwrap().agents.get_mut(&id).unwrap().run.can_stop = true;
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).is_err());
        s.lock().unwrap().agents.get_mut(&id).unwrap().run.can_stop = false;
        linked.project_id = ProjectId::new();
        s.lock().unwrap().tasks.insert(linked.id, linked.clone());
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).is_err());
        s.lock().unwrap().tasks.remove(&linked.id);
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).is_err());
        s.lock().unwrap().agents.get_mut(&id).unwrap().run.task_id = None;
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &profile).is_ok());
    }

    #[test]
    fn direct_followup_and_queue_claim_are_atomic() {
        for _ in 0..8 {
            let (s, p) = fixture();
            let id = stopped_agent(&s, &p);
            enqueue(&s, &task(&s, &p, "Queued work"));
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let claim = {
                let s = s.clone(); let p = p.clone(); let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    claim_board_task(&s, &p).unwrap()
                })
            };
            barrier.wait();
            let writer = WriterGuard::acquire_direct_prompt(&s, id, &p, &AgentExecutionProfile::default());
            let claimed = claim.join().unwrap();
            assert!(writer.is_ok());
            assert!(claimed.is_some());
            drop(writer);
        }
    }

    #[test]
    fn claimed_queue_agent_cannot_consume_launch_authorization_with_a_prompt() {
        let (s, p) = fixture();
        let id = stopped_agent(&s, &p);
        let t = task(&s, &p, "Continue queued work");
        let selected = changed(handle_task_request(s.clone(), select_request(&t, Some(id), None)));
        enqueue(&s, &selected);
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &AgentExecutionProfile::default()).is_err());
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert_eq!(claimed.assigned_agent_id, Some(id));
        assert!(WriterGuard::acquire_direct_prompt(&s, id, &p, &AgentExecutionProfile::default()).is_err());
        assert_eq!(s.lock().unwrap().tasks[&t.id], claimed);
        assert!(WriterGuard::acquire(&s, id, &p, &AgentExecutionProfile::default()).is_ok());
    }

    #[test]
    fn ssh_followup_admits_on_both_sides_and_retains_uncertain_ownership() {
        let (host, local) = fixture();
        let id = stopped_agent(&host, &local);
        let mut linked = task(&host, &local, "Reviewed remote work");
        linked.state = ditch_core::TaskState::InReview;
        linked.assigned_agent_id = Some(id);
        let binary = local.root.join("followup-codex");
        fs::write(&binary, r#"#!/usr/bin/python3
import json, sys
if '--version' in sys.argv: print('codex-cli 0.154.0'); sys.exit(0)
for line in sys.stdin:
 r=json.loads(line); m=r.get('method'); p=r.get('params',{})
 if m=='initialized': continue
 if m=='thread/start': raise Exception('Must resume')
 result={}
 if m=='thread/resume':
  assert p['threadId']=='existing-thread'
  result={'thread':{'id':'existing-thread'}}
 if m=='turn/start': result={'turn':{'id':'followup'}}
 print(json.dumps({'id':r['id'],'result':result}),flush=True)
 if m=='turn/start':
  print(json.dumps({'method':'item/completed','params':{'item':{'type':'agentMessage','text':'Follow-up result'}}}),flush=True)
  print(json.dumps({'method':'turn/completed','params':{'turn':{'status':'completed'}}}),flush=True)
  break
"#).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        {
            let mut s = host.lock().unwrap();
            s.remote_runtime = true;
            s.codex_binary = Some(binary.to_string_lossy().into_owned());
            s.agents.get_mut(&id).unwrap().run.task_id = Some(linked.id);
            s.tasks.insert(linked.id, linked.clone());
        }
        let remote = Project::new_remote(local.id, &local.name, local.root.to_str().unwrap(), Uuid::new_v4(), "followup-host");
        let mut desktop = super::tests::test_runtime();
        desktop.projects.insert(remote.root_key(), remote.clone());
        desktop.store.upsert_project(&remote).unwrap();
        desktop.remote_event_subscriptions.insert("followup-host".into());
        let lose_ack = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let target = host.clone(); let flag = lose_ack.clone(); let count = calls.clone();
        desktop.remote_connections.test_rpc = Some(Arc::new(move |_, _, request| {
            count.fetch_add(1, Ordering::SeqCst);
            let response = handle_request(request, target.clone());
            if flag.load(Ordering::SeqCst) {
                Err(ditch_ssh::SshError::Failed("acknowledgement lost".into()))
            } else { Ok(response) }
        }));
        let desktop = Arc::new(Mutex::new(desktop));
        let reconcile = || {
            let (epoch, snapshot) = { let s = host.lock().unwrap(); (s.instance_id, s.snapshot()) };
            reconcile_remote_snapshot(&desktop, "followup-host", std::slice::from_ref(&remote), epoch, snapshot, 0);
        };
        reconcile();
        for uncertain in [false, true] {
            lose_ack.store(uncertain, Ordering::SeqCst);
            let response = forward_or_prompt_agent(desktop.clone(), id, "Follow-up".into(), AgentExecutionProfile::default());
            if uncertain {
                assert!(matches!(response, ServerResponse::Error(e) if e.code == "remote_unavailable"));
            } else { assert_eq!(response, ServerResponse::Accepted); }
            let before = calls.load(Ordering::SeqCst);
            assert!(matches!(forward_or_prompt_agent(desktop.clone(), id, "Do not duplicate".into(), AgentExecutionProfile::default()), ServerResponse::Error(_)));
            assert_eq!(calls.load(Ordering::SeqCst), before);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let s = host.lock().unwrap();
                if !s.children.contains_key(&id) && !s.writer_leases.contains_key(&id) { break; }
                drop(s);
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(20));
            }
            assert!(desktop.lock().unwrap().remote_write_uncertain.contains_key(&id));
            assert_eq!(host.lock().unwrap().tasks[&linked.id], linked);
            reconcile();
            assert!(!desktop.lock().unwrap().writer_leases.contains_key(&id));
            assert_eq!(desktop.lock().unwrap().tasks[&linked.id], linked);
        }
        let s = host.lock().unwrap();
        let record = &s.agents[&id];
        assert_eq!(record.run.native_session_id.as_deref(), Some("existing-thread"));
        assert_eq!(record.messages.iter().filter(|m| m.role == AgentChatRole::User).count(), 3);
    }

    fn play_pause(
        s: &Arc<Mutex<RuntimeState>>,
        p: &Project,
        paused: bool,
    ) -> ditch_core::BoardExecutionState {
        let current = board_execution(&s.lock().unwrap(), p.id).unwrap();
        let req = TaskRequest {
            project_id: Some(p.id),
            request_id: Uuid::new_v4(),
            operation: TaskOperation::BoardExecution {
                paused: Some(paused),
                expected_revision: Some(current.revision),
            },
        };
        match handle_task_request(s.clone(), req) {
            ServerResponse::TaskResponse(TaskResponse::BoardExecution(value)) => value,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pause_persists_and_play_continues_project_queue_without_changing_tasks() {
        let (s, p) = fixture();
        let first = enqueue(&s, &task(&s, &p, "First"));
        let second = enqueue(&s, &task(&s, &p, "Second"));
        let paused = play_pause(&s, &p, true);
        assert!(paused.paused);
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        assert_eq!(s.lock().unwrap().tasks[&first.id], first);
        assert_eq!(s.lock().unwrap().tasks[&second.id], second);
        let paths = s.lock().unwrap().paths.clone();
        let restarted = Arc::new(Mutex::new(RuntimeState::new(paths, false).unwrap()));
        assert_eq!(
            board_execution(&restarted.lock().unwrap(), p.id).unwrap(),
            paused
        );
        assert!(claim_board_task(&restarted, &p).unwrap().is_none());
        let other = project(&mut restarted.lock().unwrap(), "Independent");
        let independent = enqueue(&restarted, &task(&restarted, &other, "Other project"));
        assert_eq!(
            claim_board_task(&restarted, &other).unwrap().unwrap().id,
            independent.id
        );
        play_pause(&restarted, &p, false);
        assert_eq!(
            claim_board_task(&restarted, &p).unwrap().unwrap().id,
            first.id
        );
        assert_eq!(restarted.lock().unwrap().tasks[&second.id], second);
    }

    #[test]
    fn execution_control_is_revision_checked_and_replay_cannot_toggle_twice() {
        let (s, p) = fixture();
        let pause = TaskRequest {
            project_id: Some(p.id),
            request_id: Uuid::new_v4(),
            operation: TaskOperation::BoardExecution {
                paused: Some(true),
                expected_revision: Some(0),
            },
        };
        let first = handle_task_request(s.clone(), pause.clone());
        assert_eq!(handle_task_request(s.clone(), pause.clone()), first);
        play_pause(&s, &p, false);
        assert_eq!(handle_task_request(s.clone(), pause.clone()), first);
        assert!(!board_execution(&s.lock().unwrap(), p.id).unwrap().paused);
        let mut stale = pause;
        stale.request_id = Uuid::new_v4();
        assert!(matches!(
            handle_task_request(s.clone(), stale),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        let global = TaskRequest {
            project_id: None,
            request_id: Uuid::new_v4(),
            operation: TaskOperation::BoardExecution {
                paused: Some(true),
                expected_revision: Some(0),
            },
        };
        assert!(matches!(
            handle_task_request(s.clone(), global),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
    }

    #[test]
    fn selected_agent_is_durable_locked_until_todo_and_retries_keep_identity() {
        let (s, p) = fixture();
        let t = task(&s, &p, "Bind and resume");
        let first = stopped_agent(&s, &p);
        let second = stopped_agent(&s, &p);
        let req = select_request(&t, Some(first), None);
        let selected = changed(handle_task_request(s.clone(), req.clone()));
        assert_eq!(selected.column(), TaskColumn::Todo);
        assert_eq!(changed(handle_task_request(s.clone(), req)), selected);
        let queued = enqueue(&s, &selected);
        assert!(queued.waiting_in_queue());
        assert_eq!(
            queued.execution_queue.as_ref().unwrap().agent_id,
            Some(first)
        );
        assert!(matches!(
            handle_task_request(s.clone(), select_request(&queued, Some(second), None)),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        let restarted = RuntimeState::new(s.lock().unwrap().paths.clone(), false).unwrap();
        assert_eq!(restarted.tasks[&t.id].continue_agent_id, Some(first));
        assert!(restarted.tasks[&t.id].waiting_in_queue());
        assert_eq!(restarted.agents[&first].run.task_id, Some(t.id));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert_eq!(claimed.assigned_agent_id, Some(first));
        acceptance_block(&s, t.id, "cancelled", "Stopped");
        let blocked = s.lock().unwrap().tasks[&t.id].clone();
        assert!(matches!(
            handle_task_request(s.clone(), select_request(&blocked, None, None)),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        let retry = enqueue(&s, &blocked);
        assert_eq!(
            retry.execution_queue.as_ref().unwrap().agent_id,
            Some(first)
        );
        assert_eq!(
            claim_board_task(&s, &p).unwrap().unwrap().assigned_agent_id,
            Some(first)
        );
        acceptance_block(&s, t.id, "cancelled", "Stopped again");
        let blocked = s.lock().unwrap().tasks[&t.id].clone();
        let todo = changed(handle_task_request(
            s.clone(),
            request(
                &blocked,
                TaskOperation::Move {
                    task_id: t.id,
                    expected_revision: blocked.revision,
                    column: TaskColumn::Todo,
                    before_id: None,
                },
            ),
        ));
        assert_eq!(todo.continue_agent_id, Some(first));
        let changed_agent = changed(handle_task_request(
            s.clone(),
            select_request(&todo, Some(second), None),
        ));
        assert_eq!(changed_agent.continue_agent_id, Some(second));
        assert_eq!(s.lock().unwrap().agents[&first].run.task_id, None);
        let cleared = changed(handle_task_request(
            s.clone(),
            select_request(&changed_agent, None, None),
        ));
        assert!(cleared.continue_agent_id.is_none());
        assert!(cleared.assigned_agent_id.is_none());
        assert_eq!(s.lock().unwrap().agents[&second].run.task_id, None);
    }

    #[test]
    fn transfer_requires_exact_confirmation_and_cannot_steal_a_locked_agent() {
        let (s, p) = fixture();
        let a = task(&s, &p, "Original Todo");
        let b = task(&s, &p, "Another Todo");
        let owner = stopped_agent(&s, &p);
        let a = changed(handle_task_request(
            s.clone(),
            select_request(&a, Some(owner), None),
        ));
        assert!(matches!(
            handle_task_request(s.clone(), select_request(&b, Some(owner), None)),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        let mut stale = a.clone();
        stale.revision -= 1;
        assert!(matches!(
            handle_task_request(s.clone(), select_request(&b, Some(owner), Some(&stale))),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        let b = changed(handle_task_request(
            s.clone(),
            select_request(&b, Some(owner), Some(&a)),
        ));
        let a = s.lock().unwrap().tasks[&a.id].clone();
        assert!(a.assigned_agent_id.is_none() && a.continue_agent_id.is_none());
        let b = enqueue(&s, &b);
        assert!(matches!(
            handle_task_request(s.clone(), select_request(&a, Some(owner), Some(&b))),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert_eq!(s.lock().unwrap().agents[&owner].run.task_id, Some(b.id));
    }

    #[test]
    fn resume_identity_errors_block_without_creating_another_agent() {
        let (s, p) = fixture();
        let t = task(&s, &p, "Account boundary");
        let owner = stopped_agent(&s, &p);
        let selected = changed(handle_task_request(
            s.clone(),
            select_request(&t, Some(owner), None),
        ));
        let t = enqueue(&s, &selected);
        s.lock()
            .unwrap()
            .agents
            .get_mut(&owner)
            .unwrap()
            .run
            .origin_codex_home = Some("different-account".into());
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        let locked = s.lock().unwrap();
        assert_eq!(locked.tasks[&t.id].condition, TaskCondition::Blocked);
        assert_eq!(
            locked.tasks[&t.id]
                .execution_queue
                .as_ref()
                .unwrap()
                .agent_id,
            Some(owner)
        );
        assert_eq!(locked.agents.len(), 1);
    }

    #[test]
    fn queue_persists_project_order_and_replay_without_coordinator() {
        let (s, p) = fixture();
        let second = task(&s, &p, "Created first, queued second");
        let first = task(&s, &p, "Created second, queued first");
        let req = request(
            &first,
            TaskOperation::Enqueue {
                task_id: first.id,
                expected_revision: first.revision,
                execution_profile: AgentExecutionProfile::default(),
            },
        );
        let first = changed(handle_task_request(s.clone(), req.clone()));
        let second = enqueue(&s, &second);
        assert!(first.waiting_in_queue());
        assert_eq!(first.execution_queue.as_ref().unwrap().position, 1);
        assert_eq!(second.execution_queue.as_ref().unwrap().position, 2);
        assert_eq!(changed(handle_task_request(s.clone(), req)), first);
        assert!(first.coordinator_group.is_none());
        let other = project(&mut s.lock().unwrap(), "Second");
        let other = enqueue(&s, &task(&s, &other, "Independent"));
        assert_eq!(other.execution_queue.unwrap().position, 1);
        let mut locked = s.lock().unwrap();
        locked.store.reconcile_tasks().unwrap();
        let stored = locked.store.load_tasks().unwrap();
        assert!(
            stored
                .iter()
                .find(|t| t.id == first.id)
                .unwrap()
                .waiting_in_queue()
        );
        assert!(locked.agents.is_empty());
        let paths = locked.paths.clone();
        drop(locked);
        let restarted = RuntimeState::new(paths, false).unwrap();
        assert!(restarted.tasks[&first.id].waiting_in_queue());
        assert_eq!(
            restarted.tasks[&second.id].execution_queue,
            second.execution_queue
        );
    }

    #[test]
    fn reorder_changes_dispatch_order_but_not_other_projects_or_running_task() {
        let (s, p) = fixture();
        let a = enqueue(&s, &task(&s, &p, "a"));
        let b = enqueue(&s, &task(&s, &p, "b"));
        let moved = changed(handle_task_request(
            s.clone(),
            request(
                &b,
                TaskOperation::ReorderQueue {
                    task_id: b.id,
                    expected_revision: b.revision,
                    before_id: Some(a.id),
                },
            ),
        ));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert_eq!(claimed.id, moved.id);
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        assert!(matches!(
            handle_task_request(
                s.clone(),
                request(
                    &claimed,
                    TaskOperation::ReorderQueue {
                        task_id: claimed.id,
                        expected_revision: claimed.revision,
                        before_id: None,
                    }
                )
            ),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        let other = project(&mut s.lock().unwrap(), "Other");
        let cross = enqueue(&s, &task(&s, &other, "cross"));
        let a = s.lock().unwrap().tasks[&a.id].clone();
        assert!(matches!(
            handle_task_request(
                s.clone(),
                request(
                    &a,
                    TaskOperation::ReorderQueue {
                        task_id: a.id,
                        expected_revision: a.revision,
                        before_id: Some(cross.id),
                    }
                )
            ),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert_eq!(claim_board_task(&s, &other).unwrap().unwrap().id, cross.id);
    }

    #[test]
    fn manual_agents_and_board_coexist_in_both_startup_orders() {
        for board_first in [false, true] {
            let (s, p) = fixture();
            let first = enqueue(&s, &task(&s, &p, "first"));
            let next = enqueue(&s, &task(&s, &p, "next"));
            let claimed = board_first.then(|| claim_board_task(&s, &p).unwrap().unwrap());
            let manual =
                WriterGuard::acquire(&s, AgentId::new(), &p, &AgentExecutionProfile::default())
                    .unwrap();
            let resumed = stopped_agent(&s, &p);
            let followup = WriterGuard::acquire_direct_prompt(
                &s,
                resumed,
                &p,
                &AgentExecutionProfile::default(),
            )
            .unwrap();
            assert!(
                WriterGuard::acquire_direct_prompt(
                    &s,
                    resumed,
                    &p,
                    &AgentExecutionProfile::default()
                )
                .is_err()
            );
            let claimed = claimed.unwrap_or_else(|| claim_board_task(&s, &p).unwrap().unwrap());
            assert_eq!(claimed.id, first.id);
            let owner = claimed.assigned_agent_id.unwrap();
            let board =
                WriterGuard::acquire(&s, owner, &p, &AgentExecutionProfile::default()).unwrap();
            assert!(
                WriterGuard::acquire(&s, owner, &p, &AgentExecutionProfile::default()).is_err()
            );
            assert!(
                WriterGuard::acquire_direct_prompt(
                    &s,
                    owner,
                    &p,
                    &AgentExecutionProfile::default()
                )
                .is_err()
            );
            assert!(claim_board_task(&s, &p).unwrap().is_none());
            assert_eq!(
                s.lock().unwrap().tasks[&next.id].last_reason.as_deref(),
                Some("Waiting for another board task in this project")
            );
            s.lock()
                .unwrap()
                .tasks
                .get_mut(&first.id)
                .unwrap()
                .condition = TaskCondition::Failed;
            assert!(
                claim_board_task(&s, &p).unwrap().is_none(),
                "cleanup still owns the slot"
            );
            drop(board);
            assert_eq!(claim_board_task(&s, &p).unwrap().unwrap().id, next.id);
            assert!(
                s.lock()
                    .unwrap()
                    .writer_leases
                    .contains_key(&manual.agent_id)
            );
            assert!(s.lock().unwrap().writer_leases.contains_key(&resumed));
            drop(followup);
            assert!(
                s.lock()
                    .unwrap()
                    .writer_leases
                    .contains_key(&manual.agent_id)
            );
        }
    }

    #[test]
    fn busy_selected_conversation_waits_without_consuming_authorization() {
        let (s, p) = fixture();
        let owner = stopped_agent(&s, &p);
        let t = task(&s, &p, "resume");
        let selected = changed(handle_task_request(
            s.clone(),
            select_request(&t, Some(owner), None),
        ));
        enqueue(&s, &selected);
        // Activity can arrive after selection/enqueue (for example via reconciliation).
        s.lock().unwrap().agents.get_mut(&owner).unwrap().run.state = AgentState::Working;
        let next = enqueue(&s, &task(&s, &p, "next"));
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        {
            let locked = s.lock().unwrap();
            let waiting = &locked.tasks[&t.id];
            assert!(waiting.waiting_in_queue());
            assert!(waiting.assigned_agent_id.is_none());
            assert!(waiting.started_at.is_none());
            assert_eq!(
                waiting.last_reason.as_deref(),
                Some("Waiting for the selected agent to become idle")
            );
            assert!(locked.tasks[&next.id].waiting_in_queue());
        }
        // An approval wait must also be rechecked, even without a writer lease.
        s.lock().unwrap().agents.get_mut(&owner).unwrap().run.state = AgentState::AwaitingApproval;
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        s.lock().unwrap().agents.get_mut(&owner).unwrap().run.state = AgentState::Completed;
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert_eq!(claimed.id, t.id);
        assert_eq!(claimed.assigned_agent_id, Some(owner));
    }

    #[test]
    fn board_conversation_followup_keeps_board_ownership() {
        let (s, p) = fixture();
        let owner = stopped_agent(&s, &p);
        let t = task(&s, &p, "reviewed");
        let selected = changed(handle_task_request(
            s.clone(),
            select_request(&t, Some(owner), None),
        ));
        enqueue(&s, &selected);
        claim_board_task(&s, &p).unwrap().unwrap();
        {
            let mut locked = s.lock().unwrap();
            let t = locked.tasks.get_mut(&t.id).unwrap();
            t.state = ditch_core::TaskState::InReview;
            t.condition = TaskCondition::Idle;
        }
        let writer =
            WriterGuard::acquire_direct_prompt(&s, owner, &p, &AgentExecutionProfile::default())
                .unwrap();
        enqueue(&s, &task(&s, &p, "next"));
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        drop(writer);
        claim_board_task(&s, &p).unwrap().unwrap();
        assert!(
            WriterGuard::acquire_direct_prompt(&s, owner, &p, &AgentExecutionProfile::default())
                .is_err()
        );
        assert!(
            WriterGuard::acquire_direct_prompt(
                &s,
                stopped_agent(&s, &p),
                &p,
                &AgentExecutionProfile::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn canonical_aliases_share_board_slot_but_not_manual_slot() {
        let (s, p) = fixture();
        let path = p.root.with_file_name("alias");
        std::os::unix::fs::symlink(&p.root, &path).unwrap();
        let alias = Project::new("Alias", path);
        s.lock().unwrap().store.upsert_project(&alias).unwrap();
        s.lock()
            .unwrap()
            .projects
            .insert(alias.root_key(), alias.clone());
        enqueue(&s, &task(&s, &p, "first"));
        let next = enqueue(&s, &task(&s, &alias, "alias"));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert!(claim_board_task(&s, &alias).unwrap().is_none());
        let manual = WriterGuard::acquire(
            &s,
            AgentId::new(),
            &alias,
            &AgentExecutionProfile::default(),
        )
        .unwrap();
        s.lock()
            .unwrap()
            .tasks
            .get_mut(&claimed.id)
            .unwrap()
            .condition = TaskCondition::Failed;
        assert_eq!(claim_board_task(&s, &alias).unwrap().unwrap().id, next.id);
        assert!(
            WriterGuard::acquire(
                &s,
                claimed.assigned_agent_id.unwrap(),
                &p,
                &AgentExecutionProfile::default()
            )
            .is_err()
        );
        drop(manual);
    }

    #[test]
    fn board_slot_survives_acceptance_status_changes_and_cleanup() {
        let (s, p) = fixture();
        enqueue(&s, &task(&s, &p, "first"));
        let next = enqueue(&s, &task(&s, &p, "next"));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        let owner = claimed.assigned_agent_id.unwrap();
        let guard = WriterGuard::acquire(&s, owner, &p, &AgentExecutionProfile::default()).unwrap();
        let (wake, _) = mpsc::channel();
        {
            let mut locked = s.lock().unwrap();
            locked.acceptance_owners.insert(owner);
            locked.acceptance_controls.insert(
                claimed.id,
                AcceptanceControl {
                    owner,
                    wake,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            );
        }
        guard.retain();
        // Preflight cancellation clears the placeholder assignment.
        acceptance_block(&s, claimed.id, "cancelled", "Cancelled");
        assert!(
            s.lock().unwrap().tasks[&claimed.id]
                .assigned_agent_id
                .is_none()
        );
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        assert!(
            WriterGuard::acquire(&s, AgentId::new(), &p, &AgentExecutionProfile::default()).is_ok()
        );
        {
            let mut locked = s.lock().unwrap();
            locked.acceptance_controls.remove(&claimed.id);
            locked.acceptance_owners.remove(&owner);
        }
        assert!(
            claim_board_task(&s, &p).unwrap().is_none(),
            "retained lease owns cleanup"
        );
        s.lock().unwrap().release_writer(owner);
        assert_eq!(claim_board_task(&s, &p).unwrap().unwrap().id, next.id);
    }

    #[test]
    fn restarted_board_process_retains_slot_until_exit() {
        use std::os::unix::process::CommandExt;
        let (s, p) = fixture();
        enqueue(&s, &task(&s, &p, "first"));
        let next = enqueue(&s, &task(&s, &p, "next"));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        let owner = claimed.assigned_agent_id.unwrap();
        let guard = WriterGuard::acquire(&s, owner, &p, &AgentExecutionProfile::default()).unwrap();
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        guard.started(child.id() as i32).unwrap();
        guard.retain();
        let restored = Arc::new(Mutex::new(
            RuntimeState::new(s.lock().unwrap().paths.clone(), false).unwrap(),
        ));
        let blocked = claim_board_task(&restored, &p).unwrap().is_none();
        let manual = WriterGuard::acquire(
            &restored,
            AgentId::new(),
            &p,
            &AgentExecutionProfile::default(),
        )
        .is_ok();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            blocked,
            "reconciled task status must not release a surviving process"
        );
        assert!(manual);
        assert_eq!(
            claim_board_task(&restored, &p).unwrap().unwrap().id,
            next.id
        );
        assert!(!restored.lock().unwrap().writer_leases.contains_key(&owner));
    }

    #[test]
    fn concurrent_queue_claims_have_one_owner_and_restart_never_replays_launch() {
        let (s, p) = fixture();
        enqueue(&s, &task(&s, &p, "a"));
        enqueue(&s, &task(&s, &p, "b"));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let s = s.clone();
                let p = p.clone();
                thread::spawn(move || claim_board_task(&s, &p).unwrap())
            })
            .collect();
        let claimed: Vec<_> = threads
            .into_iter()
            .filter_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(claimed.len(), 1);
        let mut locked = s.lock().unwrap();
        locked.store.reconcile_tasks().unwrap();
        let loaded = locked.store.load_tasks().unwrap();
        assert_eq!(loaded.iter().filter(|t| t.waiting_in_queue()).count(), 1);
        assert_eq!(
            loaded
                .iter()
                .find(|t| t.id == claimed[0].id)
                .unwrap()
                .condition,
            TaskCondition::Blocked
        );
    }

    #[test]
    fn waiting_plan_blocks_head_without_skipping_and_todo_removes_authorization() {
        let (s, p) = fixture();
        let a = enqueue(&s, &task(&s, &p, "a"));
        enqueue(&s, &task(&s, &p, "b"));
        s.lock()
            .unwrap()
            .tasks
            .get_mut(&a.id)
            .unwrap()
            .planning
            .text
            .clear();
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        assert!(
            s.lock()
                .unwrap()
                .tasks
                .values()
                .all(|t| t.assigned_agent_id.is_none())
        );
        let a = s.lock().unwrap().tasks[&a.id].clone();
        let todo = changed(handle_task_request(
            s.clone(),
            request(
                &a,
                TaskOperation::Move {
                    task_id: a.id,
                    expected_revision: a.revision,
                    column: TaskColumn::Todo,
                    before_id: None,
                },
            ),
        ));
        assert_eq!(todo.column(), TaskColumn::Todo);
        assert!(todo.execution_queue.is_none());
        assert_ne!(claim_board_task(&s, &p).unwrap().unwrap().id, todo.id);
    }

    #[test]
    fn failed_launch_is_visible_and_does_not_repeat() {
        let (s, p) = fixture();
        let binary = p.root.join("failed-codex");
        fs::write(&binary, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'codex-cli 0.153.4'; exit 0; fi\nexit 1\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        s.lock().unwrap().codex_binary = Some(binary.to_string_lossy().into_owned());
        let t = enqueue(&s, &task(&s, &p, "No Codex"));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        launch_board_task(&s, p.clone(), claimed);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let locked = s.lock().unwrap();
            if locked.tasks[&t.id].condition == TaskCondition::Blocked
                && !locked.task_live(&locked.tasks[&t.id])
            {
                break;
            }
            assert!(Instant::now() < deadline, "launch failure was not recorded");
            drop(locked);
            thread::sleep(Duration::from_millis(20));
        }
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        assert!(s.lock().unwrap().tasks[&t.id].last_reason.is_some());
    }

    #[test]
    fn saved_plan_survives_unsuccessful_regeneration_and_changed_description() {
        for status in ["canceled", "failed", "interrupted", "completed"] {
            let (s, p) = fixture();
            let t = enqueue(&s, &task(&s, &p, "Saved plan"));
            let mut job = new_task_plan_job(&t, Uuid::new_v4());
            job.status = status.into();
            job.description = "Earlier description".into();
            job.result = Some("An unapplied alternative".into());
            s.lock().unwrap().tasks.get_mut(&t.id).unwrap().planning.job = Some(job);
            let claimed = claim_board_task(&s, &p).unwrap().unwrap();
            assert_eq!(claimed.planning.text, t.planning.text);
            assert_eq!(claimed.description, t.description);
        }
        let (s, p) = fixture();
        let t = enqueue(&s, &task(&s, &p, "Generating"));
        s.lock().unwrap().tasks.get_mut(&t.id).unwrap().planning.job =
            Some(new_task_plan_job(&t, Uuid::new_v4()));
        assert!(claim_board_task(&s, &p).unwrap().is_none());
    }

    #[test]
    fn preflight_failure_releases_placeholder_and_explicit_retry_requeues_once() {
        let (s, p) = fixture();
        let t = enqueue(&s, &task(&s, &p, "Preflight"));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert!(claimed.assigned_agent_id.is_some());
        acceptance_block(&s, t.id, "validator_infrastructure", "Snapshot unavailable");
        let blocked = s.lock().unwrap().tasks[&t.id].clone();
        assert_eq!(blocked.condition, TaskCondition::Blocked);
        assert!(blocked.assigned_agent_id.is_none());
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        let retry = enqueue(&s, &blocked);
        assert!(retry.waiting_in_queue());
        assert_eq!(retry.planning, t.planning);
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert_ne!(claimed.assigned_agent_id, claimed.continue_agent_id);
        assert!(claim_board_task(&s, &p).unwrap().is_none());
    }

    #[test]
    fn review_changes_rejoin_tail_and_keep_feedback_while_waiting() {
        let (s, p) = fixture();
        let first = enqueue(&s, &task(&s, &p, "Review"));
        let next = enqueue(&s, &task(&s, &p, "Already queued"));
        {
            let mut locked = s.lock().unwrap();
            let review = locked.tasks.get_mut(&first.id).unwrap();
            review.state = ditch_core::TaskState::InReview;
            review.condition = TaskCondition::Idle;
        }
        let review = s.lock().unwrap().tasks[&first.id].clone();
        let returned = changed(handle_task_request(
            s.clone(),
            request(
                &review,
                TaskOperation::Transition {
                    task_id: review.id,
                    expected_revision: review.revision,
                    action: TaskAction::RequestChanges {
                        feedback: "Cover empty input".into(),
                    },
                },
            ),
        ));
        assert!(returned.waiting_in_queue());
        assert!(
            returned.execution_queue.as_ref().unwrap().position
                > next.execution_queue.as_ref().unwrap().position
        );
        let next = claim_board_task(&s, &p).unwrap().unwrap();
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        s.lock().unwrap().tasks.get_mut(&next.id).unwrap().condition = TaskCondition::Failed;
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        assert_eq!(claimed.id, first.id);
        assert_eq!(claimed.last_reason.as_deref(), Some("Cover empty input"));
    }

    #[test]
    fn normal_agent_receives_plan_and_completion_advances_queue() {
        let (s, p) = fixture();
        let binary = p.root.join("fake-codex");
        fs::write(&binary, r#"#!/usr/bin/env python3
import json,sys,os,pathlib
if '--version' in sys.argv: print('codex-cli 0.153.4');sys.exit(0)
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 r=json.loads(line);m=r.get('method');i=r.get('id');p=r.get('params',{})
 if m=='initialized':continue
 if m=='initialize':v={'userAgent':'fixture'}
 elif m=='skills/list':v={'data':[{'cwd':str(pathlib.Path.cwd()),'skills':[],'errors':[]}]}
 elif m in ['thread/start','thread/resume']:
  if m=='thread/resume':assert p['threadId']=='board-queue-thread'
  v={'thread':{'id':'board-queue-thread'}}
 elif m=='turn/start':
  send({'id':i,'result':{'turn':{'id':'turn'}}})
  send({'method':'turn/started','params':{'turn':{'id':'turn'}}})
  send({'method':'thread/name/updated','params':{'threadId':'board-queue-thread','threadName':'Normal agent title'}})
  send({'method':'item/completed','params':{'item':{'type':'agentMessage','text':'Implemented and checked'}}})
  send({'method':'turn/completed','params':{'turn':{'status':'completed'}}});os._exit(0)
 else:v={}
 send({'id':i,'result':v})
"#).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        s.lock().unwrap().codex_binary = Some(binary.to_string_lossy().into_owned());
        let t = enqueue(&s, &task(&s, &p, "First work"));
        let next = enqueue(&s, &task(&s, &p, "Next work"));
        let claimed = claim_board_task(&s, &p).unwrap().unwrap();
        let owner = claimed.assigned_agent_id.unwrap();
        play_pause(&s, &p, true);
        launch_board_task(&s, p.clone(), claimed);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let locked = s.lock().unwrap();
            if locked.tasks[&t.id].column() == TaskColumn::InReview
                && !locked.task_live(&locked.tasks[&t.id])
            {
                break;
            }
            drop(locked);
            assert!(
                Instant::now() < deadline,
                "queue task did not finish: {:?}",
                s.lock().unwrap().tasks[&t.id]
            );
            thread::sleep(Duration::from_millis(20));
        }
        {
            let locked = s.lock().unwrap();
            let agent = &locked.agents[&owner].run;
            assert!(agent.coordinator_group.is_none());
            assert_eq!(agent.task_id, Some(t.id));
            assert!(agent.user_title.is_none());
            assert_eq!(agent.codex_title.as_deref(), Some("Normal agent title"));
            assert!(
                agent
                    .current_prompt
                    .as_ref()
                    .unwrap()
                    .contains(&t.planning.text)
            );
            assert_eq!(locked.tasks[&t.id].acceptance.attempts.len(), 1);
            assert!(locked.tasks[&t.id].acceptance.current_submission.is_some());
        }
        let reviewed = s.lock().unwrap().tasks[&t.id].clone();
        assert!(claim_board_task(&s, &p).unwrap().is_none());
        play_pause(&s, &p, false);
        let accepted = changed(handle_task_request(
            s.clone(),
            request(
                &reviewed,
                TaskOperation::Transition {
                    task_id: reviewed.id,
                    expected_revision: reviewed.revision,
                    action: TaskAction::Accept,
                },
            ),
        ));
        assert_eq!(accepted.column(), TaskColumn::Done);
        let history = s.lock().unwrap().agents[&owner].messages.clone();
        assert_eq!(forward_or_prompt_agent(s.clone(), owner, "Follow up after acceptance".into(), AgentExecutionProfile::default()), ServerResponse::Accepted);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let locked = s.lock().unwrap();
            if !locked.children.contains_key(&owner) && !locked.writer_leases.contains_key(&owner) {
                assert_eq!(locked.tasks[&t.id], accepted);
                assert!(locked.agents[&owner].messages.starts_with(&history));
                assert_eq!(locked.agents[&owner].run.task_id, Some(t.id));
                assert_eq!(locked.agents[&owner].run.native_session_id.as_deref(), Some("board-queue-thread"));
                break;
            }
            drop(locked);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
        // An unrelated manual approval wait does not delay the next board task.
        let manual = stopped_agent(&s, &p);
        {
            let mut locked = s.lock().unwrap();
            let agent = &mut locked.agents.get_mut(&manual).unwrap().run;
            agent.state = AgentState::AwaitingApproval;
            agent.can_stop = true;
        }
        assert_eq!(claim_board_task(&s, &p).unwrap().unwrap().id, next.id);
    }

    #[test]
    fn selected_conversation_receives_new_prompt_and_review_changes_resume_same_agent() {
        let (s, p) = fixture();
        let binary = s.lock().unwrap().paths.data_dir.join("resume-codex");
        fs::write(&binary, r#"#!/usr/bin/env python3
import json,sys,os,pathlib
if '--version' in sys.argv: print('codex-cli 0.153.4');sys.exit(0)
trace=pathlib.Path(__file__).with_suffix('.trace')
def send(v):print(json.dumps(v),flush=True)
for line in sys.stdin:
 r=json.loads(line);m=r.get('method');i=r.get('id');p=r.get('params',{})
 if m=='initialized':continue
 with trace.open('a') as f:f.write(json.dumps(r)+'\n')
 if m=='initialize':v={'userAgent':'fixture'}
 elif m=='skills/list':v={'data':[{'cwd':str(pathlib.Path.cwd()),'skills':[],'errors':[]}]}
 elif m=='thread/resume':
  assert p['threadId']=='existing-thread'
  v={'thread':{'id':'existing-thread'}}
 elif m=='thread/start':raise Exception('Must resume existing conversation')
 elif m=='turn/start':
  if sum(json.loads(l).get('method')=='turn/start' for l in trace.read_text().splitlines())>=2:pathlib.Path('done').write_text('checked')
  send({'id':i,'result':{'turn':{'id':'turn'}}})
  send({'method':'turn/started','params':{'turn':{'id':'turn'}}})
  send({'method':'item/completed','params':{'item':{'type':'agentMessage','text':'Implemented and checked'}}})
  send({'method':'turn/completed','params':{'turn':{'status':'completed'}}});os._exit(0)
 else:v={}
 send({'id':i,'result':v})
"#).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        s.lock().unwrap().codex_binary = Some(binary.to_string_lossy().into_owned());
        let mut original = task(&s, &p, "Resume work");
        original.acceptance.config.policy.enabled = true;
        original.acceptance.config.policy.max_attempts = 2;
        original
            .acceptance
            .config
            .criteria
            .push(ditch_core::AcceptanceCriterion {
                id: Uuid::new_v4(),
                label: "Required file".into(),
                required: true,
                kind: ditch_core::CriterionKind::File {
                    path: "done".into(),
                    predicate: ditch_core::FilePredicate::Exists,
                },
            });
        s.lock()
            .unwrap()
            .tasks
            .insert(original.id, original.clone());
        let owner = stopped_agent(&s, &p);
        let selected = changed(handle_task_request(
            s.clone(),
            select_request(&original, Some(owner), None),
        ));
        let mut queued = enqueue(&s, &selected);
        for cycle in 0..2 {
            let claimed = claim_board_task(&s, &p).unwrap().unwrap();
            assert_eq!(claimed.id, queued.id);
            assert_eq!(claimed.assigned_agent_id, Some(owner));
            launch_board_task(&s, p.clone(), claimed);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let locked = s.lock().unwrap();
                let task = &locked.tasks[&original.id];
                if task.column() == TaskColumn::InReview && !locked.task_live(task) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Resume failed: {:?}",
                    task.last_reason
                );
                drop(locked);
                thread::sleep(Duration::from_millis(20));
            }
            let reviewed = s.lock().unwrap().tasks[&original.id].clone();
            assert_eq!(reviewed.assigned_agent_id, Some(owner));
            if cycle == 0 {
                queued = changed(handle_task_request(
                    s.clone(),
                    request(
                        &reviewed,
                        TaskOperation::Transition {
                            task_id: reviewed.id,
                            expected_revision: reviewed.revision,
                            action: TaskAction::RequestChanges {
                                feedback: "Cover empty input".into(),
                            },
                        },
                    ),
                ));
            }
        }
        let locked = s.lock().unwrap();
        assert_eq!(locked.agents.len(), 1);
        let record = &locked.agents[&owner];
        assert_eq!(locked.tasks[&original.id].acceptance.attempts.len(), 3);
        assert!(
            locked.tasks[&original.id]
                .acceptance
                .attempts
                .iter()
                .all(|a| a.agent_id == owner)
        );
        assert_eq!(
            record.run.native_session_id.as_deref(),
            Some("existing-thread")
        );
        assert_eq!(record.run.user_title.as_deref(), Some("My agent"));
        assert_eq!(record.run.codex_title.as_deref(), Some("Original title"));
        assert_eq!(record.messages[0].text, "Earlier conversation");
        assert!(
            record
                .run
                .current_prompt
                .as_ref()
                .unwrap()
                .contains(&original.description)
        );
        assert!(
            record
                .run
                .current_prompt
                .as_ref()
                .unwrap()
                .contains(&original.planning.text)
        );
        assert!(
            record
                .run
                .current_prompt
                .as_ref()
                .unwrap()
                .contains("Cover empty input")
        );
        let trace = fs::read_to_string(binary.with_extension("trace")).unwrap();
        let requests: Vec<serde_json::Value> = trace
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "thread/resume")
                .count(),
            3
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "turn/start")
                .count(),
            3
        );
        assert!(!requests.iter().any(|r| r["method"] == "thread/start"));
    }
}
