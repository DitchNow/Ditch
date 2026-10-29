mod remote_task_handoff_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Shared = Arc<Mutex<RuntimeState>>;

    // Two independent databases connected through serialized protocol messages.
    // No SSH server, real Codex account, project, or user credentials are used.
    fn fixture() -> (Shared, Shared, Project, Task, Arc<AtomicUsize>) {
        let (host, mut local, _) = task_planning_tests::fixture("success");
        local.git_policy = ProjectGitPolicy::AllowOutsideGit;
        {
            let mut s = host.lock().unwrap();
            s.projects.insert(local.root_key(), local.clone());
            s.store.upsert_project(&local).unwrap();
            s.remote_runtime = true;
        }
        let mut runtime = super::tests::test_runtime();
        runtime.codex_binary = None;
        let project = Project::new_remote(
            local.id,
            "SSH board",
            local.root.to_str().unwrap(),
            Uuid::new_v4(),
            "board-fixture",
        );
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        runtime
            .remote_event_subscriptions
            .insert("board-fixture".into());
        let mode = Arc::new(AtomicUsize::new(0));
        let flag = mode.clone();
        let remote = host.clone();
        runtime.remote_connections.test_rpc = Some(Arc::new(move |_, _, request| {
            if flag.load(Ordering::SeqCst) == 2 {
                return Err(ditch_ssh::SshError::Failed("offline".into()));
            }
            let request: ClientRequest =
                serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
            let response = match request {
                ClientRequest::RuntimeStatus => {
                    let mut status = remote.lock().unwrap().runtime_status();
                    if flag.load(Ordering::SeqCst) == 3 {
                        status
                            .capabilities
                            .retain(|c| c != BOARD_HANDOFF_CAPABILITY);
                    }
                    if flag.load(Ordering::SeqCst) == 4 {
                        status.capabilities.retain(|c| c != "task_releases_v1");
                    }
                    ServerResponse::RuntimeStatus(status)
                }
                ClientRequest::TaskRequest(request) => {
                    let transfer = matches!(
                        request.operation,
                        TaskOperation::MaterializeBoardTask { .. }
                    );
                    let response = handle_task_request(remote.clone(), request);
                    if transfer
                        && flag
                            .compare_exchange(1, 0, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                    {
                        return Err(ditch_ssh::SshError::Failed("acknowledgment lost".into()));
                    }
                    response
                }
                _ => panic!("unexpected fixture RPC"),
            };
            Ok(serde_json::from_slice(&serde_json::to_vec(&response).unwrap()).unwrap())
        }));
        let mac = Arc::new(Mutex::new(runtime));
        let task = changed(handle_task_request(
            mac.clone(),
            request(
                &project,
                TaskOperation::CreateBacklog {
                    draft: ditch_core::TaskDraft {
                        issue_kind: ditch_core::TaskIssueKind::Bug,
                        skills: vec![],
                        title: "Remote work".into(),
                        description: "Original requirement".into(),
                        acceptance_criteria: vec![],
                        priority: ditch_core::TaskPriority::Normal,
                    },
                },
            ),
        ));
        (mac, host, project, task, mode)
    }


    #[test]
    fn ssh_release_capability_and_owner_response_projection() {
        let (mac, host, project, _, mode) = fixture();
        let mut done = Task::new(
            project.id,
            ditch_core::TaskDraft {
                issue_kind: Default::default(),
                skills: vec![],
                title: "Released".into(),
                description: String::new(),
                acceptance_criteria: vec![],
                priority: ditch_core::TaskPriority::Normal,
            },
            1024,
        )
        .unwrap();
        done.state = ditch_core::TaskState::Accepted;
        done.completed_at = Some(Utc::now());
        {
            let mut s = host.lock().unwrap();
            s.store
                .save_task_changes(
                    &[(
                        done.clone(),
                        done.audit(TaskActor::User, "fixture", None, None),
                    )],
                    None,
                    None,
                )
                .unwrap();
            s.tasks.insert(done.id, done.clone());
        }
        mac.lock().unwrap().tasks.insert(done.id, done.clone());
        let request = request(
            &project,
            TaskOperation::AssignRelease {
                tasks: vec![ditch_core::TaskRevision {
                    task_id: done.id,
                    expected_revision: done.revision,
                }],
                destination: ditch_core::ReleaseDestination::New("SSH preview".into()),
            },
        );
        mode.store(4, Ordering::SeqCst);
        let response = handle_task_request(mac.clone(), request.clone());
        assert!(matches!(
            response,
            ServerResponse::TaskResponse(TaskResponse::Error(TaskError {
                code: TaskErrorCode::Unsupported,
                ..
            }))
        ));
        assert_eq!(host.lock().unwrap().tasks[&done.id], done);
        mode.store(0, Ordering::SeqCst);
        let response = handle_task_request(mac.clone(), request.clone());
        let ServerResponse::TaskResponse(TaskResponse::ReleaseBatch { tasks, releases }) = response
        else {
            panic!("{response:?}")
        };
        assert_eq!(tasks[0].completed_at, done.completed_at);
        assert_eq!(mac.lock().unwrap().tasks[&done.id], tasks[0]);
        assert_eq!(mac.lock().unwrap().releases[&releases[0].id], releases[0]);
        assert_eq!(
            host.lock().unwrap().store.load_releases().unwrap(),
            releases
        );
        assert!(
            mac.lock()
                .unwrap()
                .store
                .load_releases()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            handle_task_request(mac, request),
            ServerResponse::TaskResponse(TaskResponse::ReleaseBatch { tasks, releases })
        );
    }

    fn request(project: &Project, operation: TaskOperation) -> TaskRequest {
        TaskRequest {
            project_id: Some(project.id),
            request_id: Uuid::new_v4(),
            operation,
        }
    }

    fn changed(response: ServerResponse) -> Task {
        match response {
            ServerResponse::TaskResponse(TaskResponse::Changed(task)) => task,
            other => panic!("{other:?}"),
        }
    }

    fn prioritize(project: &Project, task: &Task) -> TaskRequest {
        request(
            project,
            TaskOperation::Transition {
                task_id: task.id,
                expected_revision: task.revision,
                action: TaskAction::Prioritize,
            },
        )
    }

    fn wait_plan(host: &Shared, id: TaskId) -> Task {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let task = host.lock().unwrap().tasks[&id].clone();
            if task
                .planning
                .job
                .as_ref()
                .is_some_and(|j| j.status != "running")
            {
                return task;
            }
            assert!(Instant::now() < deadline, "remote plan did not finish");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn remote_promotion_plans_on_host_and_queue_and_review_project_back_to_mac() {
        let (mac, host, project, task, _) = fixture();
        let promoted = changed(handle_task_request(
            mac.clone(),
            prioritize(&project, &task),
        ));
        assert!(!promoted.remote_pending);
        assert_eq!(promoted.column(), TaskColumn::Todo);
        let done = wait_plan(&host, task.id);
        assert_eq!(done.planning.job.as_ref().unwrap().status, "completed");
        assert!(project.root.join("started").exists());
        assert!(mac.lock().unwrap().codex_binary.is_none());
        forward_remote_event(
            &mac,
            "board-fixture",
            ServerEvent::TaskChanged(done.clone()),
        );
        assert_eq!(mac.lock().unwrap().tasks[&task.id], done);
        let backlog = changed(handle_task_request(
            mac.clone(),
            request(
                &project,
                TaskOperation::Transition {
                    task_id: done.id,
                    expected_revision: done.revision,
                    action: TaskAction::ReturnToBacklog,
                },
            ),
        ));
        let ready = changed(handle_task_request(
            mac.clone(),
            prioritize(&project, &backlog),
        ));
        assert_eq!(
            ready.planning, done.planning,
            "returning to Todo must not regenerate a saved plan"
        );
        let binary = project.root.join("execution-codex");
        fs::write(&binary, r#"#!/usr/bin/env python3
import json,sys,os,pathlib
if '--version' in sys.argv: print('codex-cli 0.153.4');sys.exit(0)
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 r=json.loads(line);m=r.get('method');i=r.get('id');p=r.get('params',{})
 if m=='initialized':continue
 if m=='initialize':v={'userAgent':'fixture'}
 elif m=='skills/list':v={'data':[{'cwd':str(pathlib.Path.cwd()),'skills':[],'errors':[]}]}
 elif m=='thread/start':v={'thread':{'id':'ssh-board-thread'}}
 elif m=='turn/start':
  send({'id':i,'result':{'turn':{'id':'turn'}}})
  send({'method':'turn/started','params':{'turn':{'id':'turn'}}})
  send({'method':'thread/name/updated','params':{'threadId':'ssh-board-thread','threadName':'Remote task agent'}})
  send({'method':'item/completed','params':{'item':{'type':'agentMessage','text':'Implemented and checked'}}})
  send({'method':'turn/completed','params':{'turn':{'status':'completed'}}});os._exit(0)
 else:v={}
 send({'id':i,'result':v})
"#).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        host.lock().unwrap().codex_binary = Some(binary.to_string_lossy().into_owned());
        let (tx, events) = mpsc::sync_channel(2048);
        host.lock().unwrap().subscribers.push(tx);
        let queued = changed(handle_task_request(
            mac.clone(),
            request(
                &project,
                TaskOperation::Enqueue {
                    task_id: done.id,
                    expected_revision: ready.revision,
                    execution_profile: AgentExecutionProfile::default(),
                },
            ),
        ));
        assert_eq!(queued.condition, ditch_core::TaskCondition::Queued);
        let local = host
            .lock()
            .unwrap()
            .projects
            .values()
            .find(|p| p.id == project.id)
            .unwrap()
            .clone();
        assert!(claim_board_task(&mac, &project).unwrap().is_none());
        let paused = handle_task_request(
            mac.clone(),
            request(
                &project,
                TaskOperation::BoardExecution {
                    paused: Some(true),
                    expected_revision: Some(0),
                },
            ),
        );
        assert!(
            matches!(paused, ServerResponse::TaskResponse(TaskResponse::BoardExecution(ref v)) if v.paused)
        );
        assert!(claim_board_task(&host, &local).unwrap().is_none());
        handle_task_request(
            mac.clone(),
            request(
                &project,
                TaskOperation::BoardExecution {
                    paused: Some(false),
                    expected_revision: Some(1),
                },
            ),
        );
        let claimed = claim_board_task(&host, &local).unwrap().unwrap();
        assert_eq!(claimed.id, task.id);
        assert!(claim_board_task(&host, &local).unwrap().is_none());
        let owner = claimed.assigned_agent_id.unwrap();
        launch_board_task(&host, local.clone(), claimed);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            for frame in events.try_iter() {
                let value: Value = serde_json::from_str(&frame).unwrap();
                let event: ServerEvent =
                    serde_json::from_value(value["body"]["event"].clone()).unwrap();
                forward_remote_event(&mac, "board-fixture", event);
            }
            if mac.lock().unwrap().tasks[&task.id].column() == TaskColumn::InReview {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "SSH task did not reach review: {:?}",
                host.lock().unwrap().tasks[&task.id]
            );
            thread::sleep(Duration::from_millis(20));
        }
        let s = mac.lock().unwrap();
        let agent = &s.agents[&owner].run;
        assert_eq!(agent.task_id, Some(task.id));
        assert_eq!(agent.project_id, project.id);
        assert_eq!(agent.codex_title.as_deref(), Some("Remote task agent"));
        assert!(
            agent
                .current_prompt
                .as_ref()
                .unwrap()
                .contains(&done.description)
        );
        assert!(
            agent
                .current_prompt
                .as_ref()
                .unwrap()
                .contains(&done.planning.text)
        );
        drop(s);
        assert_eq!(
            mac.lock().unwrap().tasks[&task.id].column(),
            TaskColumn::InReview
        );
        assert!(mac.lock().unwrap().children.is_empty());
    }

    #[test]
    fn lost_handoff_ack_replays_immutable_receipt_after_restart_and_local_edit() {
        let (mac, host, project, task, mode) = fixture();
        mode.store(1, Ordering::SeqCst);
        assert!(matches!(
            handle_task_request(mac.clone(), prioritize(&project, &task)),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert!(host.lock().unwrap().tasks.contains_key(&task.id));
        assert!(mac.lock().unwrap().tasks[&task.id].remote_pending);
        // Reconstruct the Mac runtime: ownership and frozen payload are durable.
        {
            let mut s = mac.lock().unwrap();
            let mut restored = RuntimeState::new(s.paths.clone(), false).unwrap();
            restored.codex_binary = None;
            restored.remote_connections = s.remote_connections.clone();
            restored
                .remote_event_subscriptions
                .insert("board-fixture".into());
            *s = restored;
        }
        let edited = changed(handle_task_request(
            mac.clone(),
            request(
                &project,
                TaskOperation::UpdateDescription {
                    task_id: task.id,
                    expected_description: task.description.clone(),
                    expected_plan: String::new(),
                    description: "Original requirement, amended while offline".into(),
                    plan: "Saved plan".into(),
                },
            ),
        ));
        let action = prioritize(&project, &edited);
        let promoted = changed(handle_task_request(mac.clone(), action.clone()));
        assert_eq!(promoted.description, edited.description);
        assert_eq!(promoted.planning.text, "Saved plan");
        assert!(promoted.planning.job.is_none());
        assert_eq!(changed(handle_task_request(mac.clone(), action)), promoted);
        assert!(!promoted.remote_pending);
        assert_eq!(
            host.lock()
                .unwrap()
                .tasks
                .values()
                .filter(|t| t.id == task.id)
                .count(),
            1
        );
        assert!(host.lock().unwrap().agents.is_empty());
    }

    #[test]
    fn offline_or_old_runtime_never_creates_remote_state_or_local_planning_failure() {
        for failure in [2, 3] {
            let (mac, host, project, task, mode) = fixture();
            mode.store(failure, Ordering::SeqCst);
            let response = handle_task_request(mac.clone(), prioritize(&project, &task));
            assert!(matches!(
                response,
                ServerResponse::TaskResponse(TaskResponse::Error(_))
            ));
            assert_eq!(mac.lock().unwrap().tasks[&task.id], task);
            assert!(!host.lock().unwrap().tasks.contains_key(&task.id));
            assert!(task.planning.job.is_none());
        }
    }

    #[test]
    fn legacy_partial_transfer_recovers_without_reusing_old_request_id() {
        let (mac, host, project, mut task, _) = fixture();
        task.state = ditch_core::TaskState::Ready;
        task.touch();
        // Simulate the old successful remote creation whose Mac acknowledgment failed.
        let legacy = TaskRequest {
            project_id: Some(project.id),
            request_id: task.id.0,
            operation: TaskOperation::CreateIdentified {
                task_id: task.id,
                coordinator_group: None,
                continue_agent_id: None,
                draft: ditch_core::TaskDraft {
                    issue_kind: task.issue_kind,
                    skills: vec![],
                    title: task.title.clone(),
                    description: task.description.clone(),
                    acceptance_criteria: vec![],
                    priority: task.priority,
                },
                board_task: Some(Box::new(task.clone())),
            },
        };
        changed(handle_task_request(host.clone(), legacy));
        let mut job = new_task_plan_job(&task, Uuid::new_v4());
        job.status = "failed".into();
        job.error = Some("Connect to this project's runtime to generate a plan.".into());
        task.planning.job = Some(job);
        task.touch();
        {
            let mut s = mac.lock().unwrap();
            s.store
                .save_task_changes(
                    &[(
                        task.clone(),
                        task.audit(TaskActor::User, "fixture", None, None),
                    )],
                    None,
                    None,
                )
                .unwrap();
            s.tasks.insert(task.id, task.clone());
        }
        let generated = changed(handle_task_request(
            mac.clone(),
            request(
                &project,
                TaskOperation::GeneratePlan {
                    task_id: task.id,
                    expected_revision: task.revision,
                    model: None,
                },
            ),
        ));
        assert!(!generated.remote_pending);
        assert_eq!(
            wait_plan(&host, task.id).planning.job.unwrap().status,
            "completed"
        );
    }

    #[test]
    fn pending_projection_ignores_early_remote_events_and_conflicting_remote_work() {
        let (mac, host, project, task, mode) = fixture();
        let (tx, rx) = mpsc::sync_channel(10);
        mac.lock().unwrap().subscribers.push(tx);
        let mut remote = task.clone();
        remote.remote_pending = false;
        remote.touch();
        forward_remote_event(
            &mac,
            "board-fixture",
            ServerEvent::TaskChanged(remote.clone()),
        );
        assert_eq!(mac.lock().unwrap().tasks[&task.id], task);
        assert!(rx.try_recv().is_err());
        mode.store(1, Ordering::SeqCst);
        let _ = handle_task_request(mac.clone(), prioritize(&project, &task));
        host.lock()
            .unwrap()
            .tasks
            .get_mut(&task.id)
            .unwrap()
            .description = "Different remote scope".into();
        let response = handle_task_request(mac.clone(), prioritize(&project, &task));
        assert!(matches!(
            response,
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert_eq!(
            host.lock().unwrap().tasks[&task.id].description,
            "Different remote scope"
        );
        assert!(mac.lock().unwrap().tasks[&task.id].remote_pending);
    }

    #[test]
    fn materialization_receipt_stays_immutable_and_cas_rejects_stale_replacement() {
        let (_, host, project, task, _) = fixture();
        let transfer = new_board_handoff(&project, task.clone(), None);
        let response = handle_task_request(host.clone(), transfer.clone());
        let ServerResponse::TaskResponse(TaskResponse::Materialized(receipt)) = &response else {
            panic!("{response:?}")
        };
        let updated = changed(handle_task_request(
            host.clone(),
            request(
                &project,
                TaskOperation::UpdateDescription {
                    task_id: task.id,
                    expected_description: task.description.clone(),
                    expected_plan: String::new(),
                    description: "New remote edit".into(),
                    plan: String::new(),
                },
            ),
        ));
        assert_eq!(handle_task_request(host.clone(), transfer), response);
        let replacement = new_board_handoff(&project, task.clone(), Some(*receipt.clone()));
        assert!(matches!(
            handle_task_request(host.clone(), replacement),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert_eq!(host.lock().unwrap().tasks[&task.id], updated);
    }
}
