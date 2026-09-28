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
    #[test]
    fn backlog_admission_and_board_membership_do_not_launch_work() {
        let (state, project) = fixture();
        let task = changed(handle_task_request(state.clone(), req(&project, TaskOperation::CreateBacklog { draft: draft("New scope") })));
        assert_eq!(task.column(), TaskColumn::Backlog);
        let remote = Project::new_remote(ProjectId::new(), "Child", "/work", Uuid::new_v4(), "child");
        {
            let mut s = state.lock().unwrap();
            s.store.upsert_project(&remote).unwrap();
            s.projects.insert(remote.root_key(), remote.clone());
        }
        let members = req(&project, TaskOperation::BoardProjects { members: Some(vec![remote.id,remote.id]) });
        assert_eq!(handle_task_request(state.clone(), members), ServerResponse::TaskResponse(TaskResponse::BoardProjects(vec![remote.id])));
        assert_eq!(handle_task_request(state.clone(), req(&project, TaskOperation::BoardProjects { members: None })), ServerResponse::TaskResponse(TaskResponse::BoardProjects(vec![remote.id])));
        let remote_task = changed(handle_task_request(state.clone(), req(&remote, TaskOperation::CreateBacklog { draft: draft("SSH scope") })));
        assert!(remote_task.remote_pending);
        let remote_task = changed(handle_task_request(state.clone(), req(&remote, TaskOperation::Move { task_id: remote_task.id, expected_revision: remote_task.revision, column: TaskColumn::Todo, before_id: None })));
        assert_eq!(remote_task.column(), TaskColumn::Todo);
        let mut snapshot = state.lock().unwrap().snapshot();
        snapshot.tasks.clear();
        reconcile_remote_snapshot(&state, "child", std::slice::from_ref(&remote), Uuid::new_v4(), snapshot, 0);
        let s = state.lock().unwrap();
        assert!(s.tasks.contains_key(&remote_task.id));
        assert!(s.agents.is_empty());
    }
    #[test]
    fn ssh_board_materialization_preserves_approved_scope_and_revision() {
        let (state, project) = fixture();
        let mut task = Task::new(project.id, draft("Approved scope"), 1024).unwrap();
        task.remote_pending = true;
        task.coordinator_group = Some(Uuid::new_v4());
        task.revision = 7;
        let request = req(&project, TaskOperation::CreateIdentified {
            task_id: task.id, coordinator_group: task.coordinator_group, continue_agent_id: None,
            draft: draft("Approved scope"), board_task: Some(Box::new(task.clone())),
        });
        let remote = changed(handle_task_request(state.clone(), request.clone()));
        task.remote_pending = false;
        assert_eq!(task, remote);
        assert_eq!(changed(handle_task_request(state.clone(), request)), task);
        assert!(state.lock().unwrap().agents.is_empty());
    }
    #[test]
    fn community_ssh_board_materialization_does_not_require_coordinator_ownership() {
        let (state, project) = fixture();
        let mut task = Task::new(project.id, draft("Manual scope"), 1024).unwrap();
        task.remote_pending = true;
        task.revision = 4;
        let request = req(&project, TaskOperation::CreateIdentified {
            task_id: task.id, coordinator_group: None, continue_agent_id: None,
            draft: draft("Manual scope"), board_task: Some(Box::new(task.clone())),
        });
        let remote = changed(handle_task_request(state.clone(), request));
        task.remote_pending = false;
        assert_eq!(remote, task);
        assert!(remote.coordinator_group.is_none());
        assert!(state.lock().unwrap().agents.is_empty());
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
    fn manual_agents_are_independent_and_coordinator_projects_are_serialized() {
        let (state, project) = fixture();
        let profile = AgentExecutionProfile::default();
        let group = Some(Uuid::new_v4());
        let first = WriterGuard::acquire_for_group(&state, AgentId::new(), &project, &profile, group).unwrap();
        let manual = WriterGuard::acquire(&state, AgentId::new(), &project, &profile).unwrap();
        let full = AgentExecutionProfile { approval: AgentApprovalPreset::FullAccess, ..profile.clone() };
        let second_manual = WriterGuard::acquire(&state, AgentId::new(), &project, &full).unwrap();
        let mut alias = project.clone();
        alias.id = ProjectId::new();
        assert!(WriterGuard::acquire_for_group(&state, AgentId::new(), &alias, &profile, group).is_err());
        let symlink = project.root.with_file_name("alias");
        std::os::unix::fs::symlink(&project.root, &symlink).unwrap();
        alias.root = symlink;
        assert!(WriterGuard::acquire_for_group(&state, AgentId::new(), &alias, &profile, group).is_err());
        alias.root = project.root.join("nested");
        fs::create_dir_all(&alias.root).unwrap();
        let separate = WriterGuard::acquire_for_group(&state, AgentId::new(), &alias, &full, group).unwrap();
        drop((first, manual, second_manual, separate));
        assert!(state.lock().unwrap().writer_leases.is_empty());
    }

    #[test]
    fn remote_and_full_access_reservations_are_scoped_to_coordinator_projects() {
        let (state, local) = fixture();
        let host = Uuid::new_v4();
        let group = Some(Uuid::new_v4());
        let remote = Project::new_remote(ProjectId::new(), "Remote", "/srv/one", host, "host");
        let other = Project::new_remote(ProjectId::new(), "Other", "/srv/two", host, "host");
        let profile = AgentExecutionProfile { approval: AgentApprovalPreset::FullAccess, ..Default::default() };
        let one = WriterGuard::acquire_for_group(&state, AgentId::new(), &remote, &profile, group).unwrap();
        assert!(WriterGuard::acquire_for_group(&state, AgentId::new(), &remote, &profile, group).is_err());
        let two = WriterGuard::acquire_for_group(&state, AgentId::new(), &other, &profile, group).unwrap();
        let three = WriterGuard::acquire_for_group(&state, AgentId::new(), &local, &profile, group).unwrap();
        let manual = WriterGuard::acquire(&state, AgentId::new(), &remote, &profile).unwrap();
        drop((one, two, three, manual));
        assert!(state.lock().unwrap().writer_leases.is_empty());
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
    fn restart_keeps_a_coordinator_project_reserved_until_process_exit() {
        let (state, project) = fixture();
        let id = AgentId::new();
        let group = Some(Uuid::new_v4());
        let writer = WriterGuard::acquire_for_group(&state, id, &project, &AgentExecutionProfile::default(), group).unwrap();
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
        let result = WriterGuard::acquire_for_group(
            &restored, AgentId::new(), &project, &AgentExecutionProfile::default(), group,
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
    fn concurrent_claims_have_exactly_one_coordinator_project_owner() {
        let (state, project) = fixture();
        let group = Some(Uuid::new_v4());
        let barrier = Arc::new(std::sync::Barrier::new(17));
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::new();
        for _ in 0..16 {
            let state = Arc::clone(&state);
            let project = project.clone();
            let barrier = Arc::clone(&barrier);
            let tx = tx.clone();
            workers.push(thread::spawn(move || {
                let writer = WriterGuard::acquire_for_group(
                    &state, AgentId::new(), &project, &AgentExecutionProfile::default(), group,
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
