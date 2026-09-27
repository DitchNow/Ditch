mod task_tests {
    use super::*;
    use ditch_core::{TaskDraft, TaskPriority};
    fn fixture() -> (Arc<Mutex<RuntimeState>>, Project) {
        let mut runtime = super::tests::test_runtime();
        let root = runtime.paths.data_dir.join("project");
        fs::create_dir_all(&root).unwrap();
        let mut project = Project::new("Task project", root);
        project.git_policy = ProjectGitPolicy::AllowOutsideGit;
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        (Arc::new(Mutex::new(runtime)), project)
    }
    fn draft(title: &str) -> TaskDraft {
        TaskDraft {
            skills: vec![],
            title: title.into(),
            description: "Implement safely".into(),
            acceptance_criteria: vec!["Reviewed by user".into()],
            priority: TaskPriority::Normal,
        }
    }
    fn req(project: &Project, operation: TaskOperation) -> TaskRequest {
        TaskRequest {
            project_id: Some(project.id),
            request_id: Uuid::new_v4(),
            operation,
        }
    }
    fn changed(response: ServerResponse) -> Task {
        match response {
            ServerResponse::TaskResponse(TaskResponse::Changed(t)) => t,
            other => panic!("Expected task: {other:?}"),
        }
    }
    fn wait_acceptance(state: &Arc<Mutex<RuntimeState>>, id: TaskId) -> Task {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let locked = state.lock().unwrap();
            if !locked.acceptance_controls.contains_key(&id) {
                return locked.tasks[&id].clone();
            }
            drop(locked);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn create(state: &Arc<Mutex<RuntimeState>>, project: &Project) -> Task {
        changed(handle_task_request(
            Arc::clone(state),
            req(
                project,
                TaskOperation::Create {
                    draft: draft("Task"),
                },
            ),
        ))
    }
    #[test]
    fn idempotent_creation_revisions_history_and_restart_converge() {
        let (state, project) = fixture();
        let request = req(
            &project,
            TaskOperation::Create {
                draft: draft("One"),
            },
        );
        let one = changed(handle_task_request(Arc::clone(&state), request.clone()));
        assert_eq!(
            one,
            changed(handle_task_request(Arc::clone(&state), request.clone()))
        );
        let mut conflicting = request.clone();
        conflicting.operation = TaskOperation::Create {
            draft: draft("Different"),
        };
        assert!(matches!(
            handle_task_request(Arc::clone(&state), conflicting),
            ServerResponse::TaskResponse(TaskResponse::Error(TaskError {
                code: TaskErrorCode::IdempotencyConflict,
                ..
            }))
        ));
        let updated = changed(handle_task_request(
            Arc::clone(&state),
            req(
                &project,
                TaskOperation::Update {
                    task_id: one.id,
                    expected_revision: one.revision,
                    draft: draft("Updated"),
                },
            ),
        ));
        assert!(matches!(
            handle_task_request(
                Arc::clone(&state),
                req(
                    &project,
                    TaskOperation::Update {
                        task_id: one.id,
                        expected_revision: one.revision,
                        draft: draft("Stale")
                    }
                )
            ),
            ServerResponse::TaskResponse(TaskResponse::Error(TaskError {
                code: TaskErrorCode::RevisionConflict,
                ..
            }))
        ));
        assert_eq!(
            updated,
            changed(handle_task_request(Arc::clone(&state), request))
        );
        let locked = state.lock().unwrap();
        let paths = locked.paths.clone();
        assert_eq!(locked.snapshot().tasks, vec![updated.clone()]);
        assert_eq!(locked.store.task_detail(&updated).unwrap().history.len(), 2);
        drop(locked);
        drop(state);
        let restored = RuntimeState::new(paths, false).unwrap();
        assert_eq!(restored.snapshot().tasks, vec![updated]);
    }
    #[test]
    fn board_moves_require_review_and_only_untouched_drafts_can_be_deleted() {
        let (state, project) = fixture();
        let task = create(&state, &project);
        assert!(matches!(
            handle_task_request(
                Arc::clone(&state),
                req(
                    &project,
                    TaskOperation::Move {
                        task_id: task.id,
                        expected_revision: task.revision,
                        column: TaskColumn::Done,
                        before_id: None
                    }
                )
            ),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        let task = changed(handle_task_request(
            Arc::clone(&state),
            req(
                &project,
                TaskOperation::Transition {
                    task_id: task.id,
                    expected_revision: task.revision,
                    action: TaskAction::Start,
                },
            ),
        ));
        let task = changed(handle_task_request(
            Arc::clone(&state),
            req(
                &project,
                TaskOperation::Transition {
                    task_id: task.id,
                    expected_revision: task.revision,
                    action: TaskAction::Submit {
                        summary: "Work reviewed".into(),
                    },
                },
            ),
        ));
        let task = wait_acceptance(&state, task.id);
        let accepted = changed(handle_task_request(
            Arc::clone(&state),
            req(
                &project,
                TaskOperation::Transition {
                    task_id: task.id,
                    expected_revision: task.revision,
                    action: TaskAction::Accept,
                },
            ),
        ));
        assert_eq!(accepted.column(), TaskColumn::Done);
        assert!(matches!(
            handle_task_request(
                Arc::clone(&state),
                req(
                    &project,
                    TaskOperation::Delete {
                        task_id: accepted.id,
                        expected_revision: accepted.revision
                    }
                )
            ),
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        assert!(
            state
                .lock()
                .unwrap()
                .store
                .delete_project(project.id)
                .is_err()
        );
        let draft = create(&state, &project);
        let delete = req(
            &project,
            TaskOperation::Delete {
                task_id: draft.id,
                expected_revision: 1,
            },
        );
        let first = handle_task_request(Arc::clone(&state), delete.clone());
        assert_eq!(first, handle_task_request(Arc::clone(&state), delete));
        assert!(!state.lock().unwrap().tasks.contains_key(&draft.id));
    }
    #[test]
    fn same_and_nested_workspaces_conflict_but_separate_projects_can_run() {
        let (state, project) = fixture();
        let profile = AgentExecutionProfile::default();
        let first = WriterGuard::acquire(&state, AgentId::new(), &project, &profile).unwrap();
        let mut alias = project.clone();
        alias.id = ProjectId::new();
        assert!(WriterGuard::acquire(&state, AgentId::new(), &alias, &profile).is_err());
        fs::create_dir_all(project.root.join("nested")).unwrap();
        alias.root = project.root.join("nested");
        assert!(WriterGuard::acquire(&state, AgentId::new(), &alias, &profile).is_err());
        alias.root = project.root.with_file_name("other");
        fs::create_dir_all(&alias.root).unwrap();
        let second = WriterGuard::acquire(&state, AgentId::new(), &alias, &profile).unwrap();
        let full = AgentExecutionProfile {
            approval: AgentApprovalPreset::FullAccess,
            ..profile.clone()
        };
        assert!(WriterGuard::acquire(&state, AgentId::new(), &alias, &full).is_err());
        drop(first);
        drop(second);
        let exclusive = WriterGuard::acquire(&state, AgentId::new(), &alias, &full).unwrap();
        assert!(WriterGuard::acquire(&state, AgentId::new(), &project, &profile).is_err());
        drop(exclusive);
        let symlink = project.root.with_file_name("alias");
        std::os::unix::fs::symlink(&project.root, &symlink).unwrap();
        alias.root = symlink;
        let first = WriterGuard::acquire(&state, AgentId::new(), &project, &profile).unwrap();
        assert!(WriterGuard::acquire(&state, AgentId::new(), &alias, &profile).is_err());
        drop(first);
    }
    #[test]
    fn reorder_persists_and_events_match_snapshot() {
        let (state, project) = fixture();
        let first = create(&state, &project);
        let second = create(&state, &project);
        let (tx, rx) = mpsc::sync_channel(128);
        state.lock().unwrap().subscribers.push(tx);
        let moved = changed(handle_task_request(
            Arc::clone(&state),
            req(
                &project,
                TaskOperation::Move {
                    task_id: second.id,
                    expected_revision: second.revision,
                    column: TaskColumn::Todo,
                    before_id: Some(first.id),
                },
            ),
        ));
        assert!(moved.order_key < first.order_key);
        let line = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let event: Value = serde_json::from_str(&line).unwrap();
        let projected: Task =
            serde_json::from_value(event.pointer("/body/event/TaskChanged").unwrap().clone())
                .unwrap();
        assert_eq!(projected, moved);
        let tasks = state.lock().unwrap().store.load_tasks().unwrap();
        assert_eq!(tasks[0], moved);
    }
    #[test]
    fn completed_linked_worker_submits_once_and_keeps_relational_task_link() {
        let (state, project) = fixture();
        let binary = project.root.join("fake-codex");
        fs::write(&binary,"#!/bin/sh\ncase \"$1\" in --version) echo codex-cli-test; exit 0;; esac\ncat >/dev/null\nprintf 'run\\n' >> runs.txt\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"test-thread\"}' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"Implemented and tested\"}}' '{\"type\":\"turn.completed\"}'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        state.lock().unwrap().codex_binary = Some(binary.to_string_lossy().into_owned());
        let task = create(&state, &project);
        let request = req(
            &project,
            TaskOperation::Start {
                task_id: task.id,
                expected_revision: task.revision,
                execution_profile: AgentExecutionProfile::default(),
            },
        );
        let launched = changed(handle_task_request(Arc::clone(&state), request.clone()));
        let agent_id = launched.assigned_agent_id.unwrap();
        let stopped = wait_acceptance(&state, task.id);
        assert_eq!(stopped.condition, TaskCondition::Blocked);
        assert!(
            stopped.acceptance.submissions.is_empty(),
            "Agent assertion alone must not submit"
        );
        handle_task_request(
            Arc::clone(&state),
            req(
                &project,
                TaskOperation::Revalidate {
                    task_id: task.id,
                    expected_revision: stopped.revision,
                },
            ),
        );
        wait_acceptance(&state, task.id);
        handle_task_request(Arc::clone(&state), request);
        assert_eq!(
            fs::read_to_string(project.root.join("runs.txt")).unwrap(),
            "run\n"
        );
        let mut locked = state.lock().unwrap();
        let reviewed = locked.tasks[&task.id].clone();
        assert_eq!(reviewed.column(), TaskColumn::InReview);
        assert_eq!(
            reviewed.review_summary.as_deref(),
            Some("Implemented and tested")
        );
        assert!(locked.writer_leases.is_empty());
        assert_eq!(
            locked
                .store
                .load()
                .unwrap()
                .agents
                .iter()
                .find(|a| a.run.id == agent_id)
                .unwrap()
                .run
                .task_id,
            Some(task.id)
        );
        locked
            .store
            .with_extension_connection("task_test", |db| {
                let id: String = db.query_row(
                    "SELECT task_id FROM agents WHERE id=?1",
                    rusqlite::params![agent_id.0.to_string()],
                    |r| r.get(0),
                )?;
                assert_eq!(id, task.id.0.to_string());
                Ok(())
            })
            .unwrap();
        let before = locked.store.task_detail(&reviewed).unwrap().history.len();
        locked.sync_agent_task(agent_id);
        assert_eq!(
            locked.store.task_detail(&reviewed).unwrap().history.len(),
            before
        );
        drop(locked);
        let submission = reviewed.acceptance.submissions[0].clone();
        handle_task_request(
            Arc::clone(&state),
            req(
                &project,
                TaskOperation::Transition {
                    task_id: task.id,
                    expected_revision: reviewed.revision,
                    action: TaskAction::RequestChanges {
                        feedback: "Add coverage".into(),
                    },
                },
            ),
        );
        let returned = wait_acceptance(&state, task.id);
        let mut locked = state.lock().unwrap();
        locked.sync_agent_task(agent_id);
        assert_eq!(
            locked.tasks[&task.id], returned,
            "Old completion cannot affect a new attempt"
        );
        assert_eq!(returned.acceptance.submissions[0], submission);
        assert_eq!(returned.column(), TaskColumn::InProgress);
    }
    #[test]
    fn crashed_queued_task_recovers_blocked_and_never_done() {
        let (state, project) = fixture();
        let task = create(&state, &project);
        prepare_task_request(
            &state,
            &req(
                &project,
                TaskOperation::Start {
                    task_id: task.id,
                    expected_revision: 1,
                    execution_profile: AgentExecutionProfile::default(),
                },
            ),
        )
        .unwrap();
        let paths = state.lock().unwrap().paths.clone();
        drop(state);
        let restored = RuntimeState::new(paths, false).unwrap();
        let task = &restored.tasks[&task.id];
        assert_eq!(task.column(), TaskColumn::InProgress);
        assert_eq!(task.condition, TaskCondition::Blocked);
    }
    #[test]
    fn restart_keeps_a_surviving_process_group_exclusive_until_it_exits() {
        let (state, project) = fixture();
        let id = AgentId::new();
        let writer =
            WriterGuard::acquire(&state, id, &project, &AgentExecutionProfile::default()).unwrap();
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        writer.started(child.id() as i32).unwrap();
        writer.retain();
        let paths = state.lock().unwrap().paths.clone();
        drop(state);
        let restored = Arc::new(Mutex::new(RuntimeState::new(paths, false).unwrap()));
        let result = WriterGuard::acquire(
            &restored,
            AgentId::new(),
            &project,
            &AgentExecutionProfile::default(),
        );
        let blocked = result.is_err();
        drop(result);
        {
            let mut locked = restored.lock().unwrap();
            locked.release_writer(id);
            assert!(
                locked.writer_leases.contains_key(&id),
                "live process group retains its lease during cleanup"
            );
        }
        child.wait().unwrap();
        assert!(blocked, "a surviving writer must prevent another launch");
        assert!(
            WriterGuard::acquire(
                &restored,
                AgentId::new(),
                &project,
                &AgentExecutionProfile::default()
            )
            .is_ok()
        );
    }
    #[test]
    fn concurrent_claims_have_exactly_one_workspace_owner() {
        let (state, project) = fixture();
        let barrier = Arc::new(std::sync::Barrier::new(17));
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..16 {
            let state = Arc::clone(&state);
            let project = project.clone();
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            workers.push(thread::spawn(move || {
                let writer = WriterGuard::acquire(
                    &state,
                    AgentId::new(),
                    &project,
                    &AgentExecutionProfile::default(),
                );
                tx.send(writer.is_ok()).unwrap();
                barrier.wait();
                drop(writer);
            }));
        }
        let successes = (0..16)
            .filter(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .count();
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(successes, 1);
        assert!(state.lock().unwrap().writer_leases.is_empty());
    }
    #[test]
    fn remote_task_events_are_scoped_and_stale_poll_results_do_not_regress_them() {
        let (state, _) = fixture();
        let project = Project::new_remote(
            ProjectId::new(),
            "Remote",
            "/work",
            Uuid::new_v4(),
            "remote",
        );
        state
            .lock()
            .unwrap()
            .projects
            .insert(project.root_key(), project.clone());
        let mut task = Task::new(project.id, draft("Remote work"), 1024).unwrap();
        forward_remote_event(&state, "other-host", ServerEvent::TaskChanged(task.clone()));
        assert!(state.lock().unwrap().tasks.is_empty());
        let stale = state.lock().unwrap().snapshot();
        task.touch();
        forward_remote_event(&state, "remote", ServerEvent::TaskChanged(task.clone()));
        reconcile_remote_snapshot(
            &state,
            "remote",
            std::slice::from_ref(&project),
            Uuid::new_v4(),
            stale,
            0,
        );
        assert_eq!(state.lock().unwrap().tasks[&task.id].revision, 2);
        forward_remote_event(
            &state,
            "remote",
            ServerEvent::TaskDeleted {
                task_id: task.id,
                project_id: project.id,
            },
        );
        assert!(state.lock().unwrap().tasks.is_empty());
    }
}
