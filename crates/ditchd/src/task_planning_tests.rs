mod task_planning_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fixture(mode: &str) -> (Arc<Mutex<RuntimeState>>, Project, Task) {
        let mut runtime = super::tests::test_runtime();
        let root = runtime.paths.data_dir.join("planning-project");
        fs::create_dir_all(&root).unwrap();
        let project = Project::new("Planning", root.clone());
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let binary = root.join("fake-codex");
        fs::write(&binary, format!(r##"#!/usr/bin/python3
import json, sys, pathlib, time
root = pathlib.Path.cwd()
mode = {mode:?}
def emit(v):
    print(json.dumps(v), flush=True)
for line in sys.stdin:
    r = json.loads(line)
    method, p = r.get('method'), r.get('params', {{}})
    if method == 'initialize':
        emit({{'id': r['id'], 'result': {{'userAgent': 'test'}}}})
    elif method == 'config/read':
        emit({{'id': r['id'], 'result': {{'config': {{'mcp_servers': {{'untrusted': {{}}}}}}}}}})
    elif method == 'thread/start':
        assert p['sandbox'] == 'read-only'
        assert p['approvalPolicy'] == 'never'
        assert p['ephemeral'] is True
        assert p['config']['mcp_servers']['untrusted']['enabled'] is False
        assert p['config']['features']['hooks'] is False
        emit({{'id':r['id'], 'result':{{'thread':{{'id':'plan-thread','ephemeral': mode != 'old'}}}}}})
    elif method == 'turn/start':
        assert p['sandboxPolicy'] == {{'type':'readOnly'}}
        assert p['approvalPolicy'] == 'never'
        text = p['input'][0]['text']
        assert 'Original requirement' in text and 'Do not edit anything.' in text
        (root / 'started').write_text(text)
        emit({{'id':r['id'], 'result':{{'turn':{{'id':'turn-1'}}}}}})
        if mode == 'wait':
            while not (root / 'finish').exists(): time.sleep(.02)
        if mode == 'tool':
            emit({{'id':'interaction','method':'item/commandExecution/requestApproval','params':{{}}}})
        elif mode == 'malformed':
            emit({{'method':'item/completed','params':{{'threadId':'plan-thread','item':{{'type':'agentMessage','text':'not a plan object'}}}}}})
        else:
            emit({{'method':'item/completed','params':{{'threadId':'plan-thread','item':{{'type':'commandExecution','command':'rg --files'}}}}}})
            emit({{'method':'item/completed','params':{{'threadId':'plan-thread','item':{{'type':'agentMessage','text':json.dumps({{'plan':'## Implementation\nInspect current code, implement, and verify.'}})}}}}}})
        emit({{'method':'turn/completed','params':{{'threadId':'plan-thread','turn':{{'status':'completed'}}}}}})
"##)).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        runtime.codex_binary = Some(binary.to_string_lossy().into_owned());
        let state = Arc::new(Mutex::new(runtime));
        let task = changed(call(
            &state,
            &project,
            TaskOperation::CreateBacklog {
                draft: ditch_core::TaskDraft {
                    issue_kind: Default::default(),
                    skills: vec![],
                    title: "Plan this task".into(),
                    description: "Original requirement".into(),
                    acceptance_criteria: vec![],
                    priority: ditch_core::TaskPriority::Normal,
                },
            },
        ));
        (state, project, task)
    }
    fn call(state: &Arc<Mutex<RuntimeState>>, p: &Project, op: TaskOperation) -> ServerResponse {
        handle_task_request(
            state.clone(),
            TaskRequest {
                project_id: Some(p.id),
                request_id: Uuid::new_v4(),
                operation: op,
            },
        )
    }
    fn changed(r: ServerResponse) -> Task {
        match r {
            ServerResponse::TaskResponse(TaskResponse::Changed(t)) => t,
            r => panic!("{r:?}"),
        }
    }
    fn start(state: &Arc<Mutex<RuntimeState>>, p: &Project, t: &Task) -> Task {
        changed(call(
            state,
            p,
            TaskOperation::GeneratePlan {
                task_id: t.id,
                expected_revision: t.revision,
                model: None,
            },
        ))
    }
    fn wait(state: &Arc<Mutex<RuntimeState>>, id: TaskId, done: impl Fn(&Task) -> bool) -> Task {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let t = state.lock().unwrap().tasks[&id].clone();
            if done(&t) {
                return t;
            }
            assert!(Instant::now() < deadline, "timed out: {t:?}");
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn finished(state: &Arc<Mutex<RuntimeState>>, id: TaskId) -> Task {
        wait(state, id, |t| {
            t.planning.job.as_ref().unwrap().status != "running"
        })
    }

    #[test]
    fn backlog_to_todo_starts_one_durable_plan_for_moves_and_prioritize() {
        for use_move in [true, false] {
            let (s, p, t) = fixture("success");
            let operation = if use_move {
                TaskOperation::Move {
                    task_id: t.id,
                    expected_revision: t.revision,
                    column: TaskColumn::Todo,
                    before_id: None,
                }
            } else {
                TaskOperation::Transition {
                    task_id: t.id,
                    expected_revision: t.revision,
                    action: TaskAction::Prioritize,
                }
            };
            let req = TaskRequest {
                project_id: Some(p.id),
                request_id: Uuid::new_v4(),
                operation,
            };
            let started = changed(handle_task_request(s.clone(), req.clone()));
            assert_eq!(started.column(), TaskColumn::Todo);
            assert_eq!(started.planning.job.as_ref().unwrap().status, "running");
            assert_eq!(started.description, t.description);
            let done = finished(&s, t.id);
            assert_eq!(done.planning.job.as_ref().unwrap().status, "completed");
            assert!(done.planning.text.contains("Implementation"));
            assert_eq!(changed(handle_task_request(s.clone(), req)), done);
            let locked = s.lock().unwrap();
            assert_eq!(locked.store.load_tasks().unwrap()[0], done);
            assert!(locked.agents.is_empty());
            assert!(locked.writer_leases.is_empty());
        }
    }

    #[test]
    fn other_todo_entries_reorders_and_invalid_moves_do_not_plan() {
        let (s, p, t) = fixture("success");
        let invalid = call(
            &s,
            &p,
            TaskOperation::Move {
                task_id: t.id,
                expected_revision: t.revision,
                column: TaskColumn::Todo,
                before_id: Some(TaskId::new()),
            },
        );
        assert!(matches!(
            invalid,
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert!(s.lock().unwrap().tasks[&t.id].planning.job.is_none());
        for (state, operation) in [
            (
                ditch_core::TaskState::Ready,
                TaskOperation::Move {
                    task_id: t.id,
                    expected_revision: 1,
                    column: TaskColumn::Todo,
                    before_id: None,
                },
            ),
            (
                ditch_core::TaskState::Running,
                TaskOperation::Transition {
                    task_id: t.id,
                    expected_revision: 1,
                    action: TaskAction::ReturnToTodo {
                        reason: "User returned task".into(),
                    },
                },
            ),
            (
                ditch_core::TaskState::Accepted,
                TaskOperation::Transition {
                    task_id: t.id,
                    expected_revision: 1,
                    action: TaskAction::Reopen,
                },
            ),
            (
                ditch_core::TaskState::InReview,
                TaskOperation::Transition {
                    task_id: t.id,
                    expected_revision: 1,
                    action: TaskAction::RequestChanges {
                        feedback: "Needs revision".into(),
                    },
                },
            ),
        ] {
            let mut task = t.clone();
            task.state = state;
            s.lock().unwrap().tasks.insert(t.id, task);
            let changed = changed(call(&s, &p, operation));
            assert!(changed.planning.job.is_none());
        }
        assert!(!p.root.join("started").exists());
    }

    #[test]
    fn automatic_planning_preserves_existing_plan_and_only_explicit_generate_replaces_it() {
        let (s, p, t) = fixture("wait");
        let active = start(&s, &p, &t);
        let moved = changed(call(
            &s,
            &p,
            TaskOperation::Transition {
                task_id: t.id,
                expected_revision: active.revision,
                action: TaskAction::Prioritize,
            },
        ));
        assert_eq!(moved.planning.job, active.planning.job);
        fs::write(p.root.join("finish"), "").unwrap();
        let done = finished(&s, t.id);
        let backlog = changed(call(
            &s,
            &p,
            TaskOperation::Transition {
                task_id: t.id,
                expected_revision: done.revision,
                action: TaskAction::ReturnToBacklog,
            },
        ));
        assert_eq!(backlog.planning.job, done.planning.job);
        let promoted = changed(call(
            &s,
            &p,
            TaskOperation::Transition {
                task_id: t.id,
                expected_revision: backlog.revision,
                action: TaskAction::Prioritize,
            },
        ));
        assert_eq!(promoted.planning, done.planning);
        let regenerated = start(&s, &p, &promoted);
        assert_ne!(
            regenerated.planning.job.as_ref().unwrap().id,
            done.planning.job.as_ref().unwrap().id
        );
        assert_eq!(regenerated.planning.text, done.planning.text);
        assert_eq!(finished(&s, t.id).planning.job.unwrap().status, "completed");
    }

    #[test]
    fn promotion_preserves_handwritten_and_separately_retained_plans() {
        for retained in [false, true] {
            let (s, p, mut t) = fixture("success");
            if retained {
                let mut job = new_task_plan_job(&t, Uuid::new_v4());
                job.status = "completed".into();
                job.result = Some("A generated plan awaiting review".into());
                t.planning.job = Some(job);
            } else {
                t.planning.text = "My edited implementation plan".into();
            }
            s.lock().unwrap().tasks.insert(t.id, t.clone());
            let promoted = changed(call(
                &s,
                &p,
                TaskOperation::Move {
                    task_id: t.id,
                    expected_revision: t.revision,
                    column: TaskColumn::Todo,
                    before_id: None,
                },
            ));
            assert_eq!(promoted.planning, t.planning);
            assert!(!p.root.join("started").exists());
        }
    }

    #[test]
    fn missing_codex_does_not_block_move_and_records_retryable_failure() {
        let (s, p, t) = fixture("success");
        s.lock().unwrap().codex_binary = None;
        let moved = changed(call(
            &s,
            &p,
            TaskOperation::Transition {
                task_id: t.id,
                expected_revision: t.revision,
                action: TaskAction::Prioritize,
            },
        ));
        assert_eq!(moved.column(), TaskColumn::Todo);
        assert_eq!(moved.planning.job.as_ref().unwrap().status, "failed");
        assert!(
            moved
                .planning
                .job
                .as_ref()
                .unwrap()
                .error
                .as_ref()
                .unwrap()
                .contains("Set up Codex")
        );
        s.lock().unwrap().codex_binary =
            Some(p.root.join("fake-codex").to_string_lossy().into_owned());
        start(&s, &p, &moved);
        assert_eq!(finished(&s, t.id).planning.job.unwrap().status, "completed");
    }

    #[test]
    fn background_plan_persists_without_agent_or_board_transition_and_replay_does_not_restart() {
        let (s, p, t) = fixture("success");
        let req = TaskRequest {
            project_id: Some(p.id),
            request_id: Uuid::new_v4(),
            operation: TaskOperation::GeneratePlan {
                task_id: t.id,
                expected_revision: t.revision,
                model: None,
            },
        };
        let started = changed(handle_task_request(s.clone(), req.clone()));
        assert_eq!(started.planning.job.unwrap().status, "running");
        let done = finished(&s, t.id);
        assert_eq!(done.description, t.description);
        assert_eq!(done.state, t.state);
        assert_eq!(done.condition, t.condition);
        assert!(done.planning.text.contains("Implementation"));
        assert_eq!(changed(handle_task_request(s.clone(), req)), done);
        let locked = s.lock().unwrap();
        assert!(locked.agents.is_empty());
        assert!(locked.children.is_empty());
        assert!(locked.writer_leases.is_empty());
        assert_eq!(locked.store.load_tasks().unwrap()[0], done);
    }

    #[test]
    fn concurrent_edits_survive_generation_and_duplicate_generation_is_rejected() {
        let (s, p, t) = fixture("wait");
        let active = start(&s, &p, &t);
        wait(&s, t.id, |_| p.root.join("started").exists());
        assert!(matches!(
            call(
                &s,
                &p,
                TaskOperation::GeneratePlan {
                    task_id: t.id,
                    expected_revision: active.revision,
                    model: None
                }
            ),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        changed(call(
            &s,
            &p,
            TaskOperation::UpdateDescription {
                task_id: t.id,
                expected_description: t.description.clone(),
                expected_plan: String::new(),
                description: "New user input".into(),
                plan: "My handwritten plan".into(),
            },
        ));
        fs::write(p.root.join("finish"), "").unwrap();
        let done = finished(&s, t.id);
        assert_eq!(done.description, "New user input");
        assert_eq!(done.planning.text, "My handwritten plan");
        assert!(
            done.planning
                .job
                .unwrap()
                .result
                .unwrap()
                .contains("Implementation")
        );
    }

    #[test]
    fn cancellation_does_not_allow_late_result_to_replace_plan() {
        let (s, p, t) = fixture("wait");
        let active = start(&s, &p, &t);
        wait(&s, t.id, |_| p.root.join("started").exists());
        let id = active.planning.job.unwrap().id;
        changed(call(
            &s,
            &p,
            TaskOperation::CancelPlan {
                task_id: t.id,
                job_id: id,
            },
        ));
        fs::write(p.root.join("finish"), "").unwrap();
        thread::sleep(Duration::from_millis(300));
        let done = finished(&s, t.id);
        assert_eq!(done.planning.job.unwrap().status, "canceled");
        assert!(done.planning.text.is_empty());
    }

    #[test]
    fn restart_marks_unfinished_job_interrupted_and_keeps_original_text() {
        let (s, p, t) = fixture("wait");
        start(&s, &p, &t);
        wait(&s, t.id, |_| p.root.join("started").exists());
        let paths = s.lock().unwrap().paths.clone();
        let restarted = RuntimeState::new(paths, false).unwrap();
        assert_eq!(
            restarted.tasks[&t.id].planning.job.as_ref().unwrap().status,
            "interrupted"
        );
        assert_eq!(restarted.tasks[&t.id].description, t.description);
        let id = s.lock().unwrap().tasks[&t.id]
            .planning
            .job
            .as_ref()
            .unwrap()
            .id;
        call(
            &s,
            &p,
            TaskOperation::CancelPlan {
                task_id: t.id,
                job_id: id,
            },
        );
    }

    #[test]
    fn old_codex_tools_and_malformed_output_fail_without_overwriting_input() {
        for mode in ["old", "tool", "malformed"] {
            let (s, p, t) = fixture(mode);
            start(&s, &p, &t);
            let done = finished(&s, t.id);
            assert_eq!(done.planning.job.unwrap().status, "failed");
            assert_eq!(done.description, t.description);
            assert!(done.planning.text.is_empty());
        }
    }

    #[test]
    fn failed_result_save_keeps_result_available_without_permanent_spinner() {
        let (s, p, t) = fixture("wait");
        start(&s, &p, &t);
        wait(&s, t.id, |_| p.root.join("started").exists());
        s.lock().unwrap().store.with_extension_transaction("planning_test", |tx| {
            tx.execute_batch("CREATE TRIGGER fail_plan_save BEFORE UPDATE ON tasks BEGIN SELECT RAISE(ABORT, 'fixture disk failure'); END;")?;
            Ok(())
        }).unwrap();
        fs::write(p.root.join("finish"), "").unwrap();
        let failed = finished(&s, t.id);
        assert_eq!(failed.planning.job.as_ref().unwrap().status, "failed");
        assert!(failed.planning.text.is_empty());
        let result = failed
            .planning
            .job
            .as_ref()
            .unwrap()
            .result
            .clone()
            .unwrap();
        s.lock()
            .unwrap()
            .store
            .with_extension_transaction("planning_test", |tx| {
                tx.execute_batch("DROP TRIGGER fail_plan_save;")?;
                Ok(())
            })
            .unwrap();
        let saved = changed(call(
            &s,
            &p,
            TaskOperation::UpdateDescription {
                task_id: t.id,
                expected_description: t.description.clone(),
                expected_plan: String::new(),
                description: t.description.clone(),
                plan: result.clone(),
            },
        ));
        assert_eq!(saved.planning.text, result);
        assert_eq!(s.lock().unwrap().store.load_tasks().unwrap()[0], saved);
    }

    #[test]
    fn description_conflict_and_review_integrity_are_enforced() {
        let (s, p, t) = fixture("success");
        let op = TaskOperation::UpdateDescription {
            task_id: t.id,
            expected_description: "stale".into(),
            expected_plan: String::new(),
            description: "overwrite".into(),
            plan: String::new(),
        };
        assert!(matches!(
            call(&s, &p, op),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        s.lock().unwrap().tasks.get_mut(&t.id).unwrap().state = ditch_core::TaskState::InReview;
        assert!(matches!(
            call(
                &s,
                &p,
                TaskOperation::GeneratePlan {
                    task_id: t.id,
                    expected_revision: t.revision,
                    model: None
                }
            ),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert!(!p.root.join("started").exists());
    }
}
