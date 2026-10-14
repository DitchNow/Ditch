mod acceptance_tests {
    use super::*;
    use ditch_core::{AcceptanceConfig, FilePredicate, LoopPolicy, TaskDraft, TaskPriority};
    const WORKER: &str = r#"#!/usr/bin/env python3
import json,sys,pathlib,os,time
if '--version' in sys.argv:print('codex-cli 0.153.4');sys.exit(0)
root=pathlib.Path.cwd()
def send(v):print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line);m=v.get('method');i=v.get('id');p=v.get('params',{})
 if m=='initialized':continue
 if m=='initialize':result={'userAgent':'fixture'}
 elif m=='skills/list':result={'data':[{'cwd':str(root),'skills':[],'errors':[]}]}
 elif m=='skills/extraRoots/set':result={}
 elif m in ['thread/start','thread/resume']:result={'thread':{'id':p.get('threadId','acceptance-thread')}}
 elif m=='turn/start':
  send({'id':i,'result':{'turn':{'id':'turn'}}})
  send({'method':'turn/started','params':{'turn':{'id':'turn'}}})
  if (root/'permission').exists():
   send({'id':99,'method':'item/commandExecution/requestApproval','params':{'threadId':'acceptance-thread','turnId':'turn','itemId':'command','command':'fixture approval','cwd':str(root)}});continue
  if (root/'wait').exists():continue
  count=int((root/'count').read_text())+1 if (root/'count').exists() else 1
  if not (root/'no-change').exists():(root/'count').write_text(str(count))
  if (root/'pass-after').exists() and count>=int((root/'pass-after').read_text()):(root/'done').write_text('done')
  send({'method':'thread/tokenUsage/updated','params':{'threadId':'acceptance-thread','turnId':'turn','tokenUsage':{'last':{'totalTokens':10},'total':{'totalTokens':10}}}})
  send({'method':'item/completed','params':{'item':{'type':'agentMessage','text':'Fixture final summary'}}})
  send({'method':'turn/completed','params':{'turn':{'status':'completed'}}});os._exit(0)
 elif m=='command/exec':
  if p['command'][0]=='timeout':time.sleep(10)
  if p['command'][0]=='wait-validation':time.sleep(1)
  if p['command'][0]=='missing':send({'id':i,'error':{'message':'executable not found'}});continue
  result={'exitCode':0 if p['command'][0]=='pass' else 1,'stdout':'password=fixture-secret\nsafe output','stderr':''}
 else:result={}
 send({'id':i,'result':result})
"#;
    fn fixture(kind: CriterionKind) -> (Arc<Mutex<RuntimeState>>, Project, Task) {
        let mut runtime = super::tests::test_runtime();
        let root = runtime.paths.data_dir.join("project");
        fs::create_dir_all(&root).unwrap();
        let mut project = Project::new("Acceptance", fs::canonicalize(root).unwrap());
        project.git_policy = ProjectGitPolicy::AllowOutsideGit;
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let binary = runtime.paths.data_dir.join("acceptance-codex");
        fs::write(&binary, WORKER).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        runtime.codex_binary = Some(binary.to_string_lossy().into());
        let mut task = Task::new(
            project.id,
            TaskDraft {
                skills: vec![],
                title: "Check work".into(),
                description: "Fixture".into(),
                acceptance_criteria: vec!["Human review".into()],
                priority: TaskPriority::Normal,
            },
            1024,
        )
        .unwrap();
        task.acceptance.config = AcceptanceConfig {
            policy: LoopPolicy {
                enabled: true,
                deadline_seconds: 15,
                ..Default::default()
            },
            criteria: vec![AcceptanceCriterion {
                id: Uuid::new_v4(),
                label: "Required check".into(),
                required: true,
                kind,
            }],
        };
        runtime
            .store
            .save_task_changes(
                &[(
                    task.clone(),
                    task.audit(TaskActor::User, "create", None, None),
                )],
                None,
                None,
            )
            .unwrap();
        runtime.tasks.insert(task.id, task.clone());
        (Arc::new(Mutex::new(runtime)), project, task)
    }
    fn request(
        state: &Arc<Mutex<RuntimeState>>,
        task: &Task,
        operation: TaskOperation,
    ) -> ServerResponse {
        handle_task_request(
            Arc::clone(state),
            TaskRequest {
                project_id: Some(task.project_id),
                request_id: Uuid::new_v4(),
                operation,
            },
        )
    }
    fn start(state: &Arc<Mutex<RuntimeState>>, task: &Task) {
        let response = request(
            state,
            task,
            TaskOperation::Start {
                task_id: task.id,
                expected_revision: task.revision,
                execution_profile: AgentExecutionProfile {
                    transport: ditch_core::AgentTransport::AppServer,
                    ..Default::default()
                },
            },
        );
        assert!(
            matches!(
                response,
                ServerResponse::TaskResponse(TaskResponse::Changed(_))
            ),
            "{response:?}"
        );
    }
    fn wait(state: &Arc<Mutex<RuntimeState>>, id: TaskId) -> Task {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let locked = state.lock().unwrap();
            if !locked.acceptance_controls.contains_key(&id) {
                return locked.tasks[&id].clone();
            }
            drop(locked);
            assert!(Instant::now() < end, "loop did not stop");
            thread::sleep(Duration::from_millis(30));
        }
    }
    fn file() -> CriterionKind {
        CriterionKind::File {
            path: "done".into(),
            predicate: FilePredicate::Exists,
        }
    }
    #[test]
    fn failed_check_corrects_then_submits_immutable_evidence() {
        let (state, project, task) = fixture(file());
        fs::write(project.root.join("pass-after"), "2").unwrap();
        start(&state, &task);
        let task = wait(&state, task.id);
        assert_eq!(
            task.column(),
            TaskColumn::InReview,
            "{:?}",
            task.last_reason
        );
        assert_eq!(task.acceptance.attempts.len(), 2);
        assert_eq!(task.acceptance.submissions.len(), 1);
        assert!(task.acceptance.attempts[1].prompt.contains("failed"));
        assert!(task.acceptance.attempts[1].provider_usage.is_some());
        assert_ne!(
            task.acceptance.attempts[0].id,
            task.acceptance.attempts[1].id
        );
        assert!(state.lock().unwrap().writer_leases.is_empty());
        let response = request(
            &state,
            &task,
            TaskOperation::Transition {
                task_id: task.id,
                expected_revision: task.revision,
                action: TaskAction::Accept,
            },
        );
        assert!(
            matches!(response,ServerResponse::TaskResponse(TaskResponse::Changed(ref t)) if t.column()==TaskColumn::Done),
            "{response:?}"
        );
    }
    #[test]
    fn first_pass_and_stale_acceptance_and_request_changes() {
        let (state, project, task) = fixture(file());
        fs::write(project.root.join("pass-after"), "1").unwrap();
        start(&state, &task);
        let task = wait(&state, task.id);
        assert_eq!(task.acceptance.attempts.len(), 1);
        let submission = task.acceptance.submissions[0].clone();
        fs::write(project.root.join("external"), "human change").unwrap();
        let response = request(
            &state,
            &task,
            TaskOperation::Transition {
                task_id: task.id,
                expected_revision: task.revision,
                action: TaskAction::Accept,
            },
        );
        assert!(matches!(
            response,
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
        request(
            &state,
            &task,
            TaskOperation::Transition {
                task_id: task.id,
                expected_revision: task.revision,
                action: TaskAction::RequestChanges {
                    feedback: "Fix this explicitly".into(),
                },
            },
        );
        let task = wait(&state, task.id);
        assert_eq!(task.acceptance.submissions[0], submission);
        assert!(
            task.acceptance
                .attempts
                .last()
                .unwrap()
                .prompt
                .contains("Fix this explicitly")
        );
    }
    #[test]
    fn identical_failure_and_no_change_stop_before_budget() {
        let (state, _, task) = fixture(file());
        start(&state, &task);
        let task = wait(&state, task.id);
        assert!(task.last_reason.unwrap().contains("identical_failure"));
        assert_eq!(task.acceptance.attempts.len(), 2);
        let (state, project, mut task) = fixture(file());
        fs::write(project.root.join("no-change"), "").unwrap();
        task.acceptance.config.policy.require_change = true;
        state.lock().unwrap().tasks.insert(task.id, task.clone());
        start(&state, &task);
        let task = wait(&state, task.id);
        assert!(task.last_reason.unwrap().contains("no_change"));
        assert_eq!(task.acceptance.attempts.len(), 1);
    }
    #[test]
    fn budget_exhaustion_never_marks_done() {
        let (state, _, mut task) = fixture(file());
        task.acceptance.config.policy.stop_on_identical_failure = false;
        state.lock().unwrap().tasks.insert(task.id, task.clone());
        start(&state, &task);
        let task = wait(&state, task.id);
        assert_eq!(task.acceptance.attempts.len(), 3);
        assert_eq!(task.column(), TaskColumn::InProgress);
        assert!(task.last_reason.unwrap().contains("budget_exhausted"));
    }
    #[test]
    fn command_confirmation_timeout_failure_and_redaction() {
        for name in ["pass", "missing", "timeout"] {
            let command = CommandCheck {
                argv: vec![name.into()],
                cwd: ".".into(),
                timeout_seconds: 1,
                expected_exit: 0,
            };
            let (state, _, task) = fixture(CriterionKind::Command(command));
            start(&state, &task);
            let task = wait(&state, task.id);
            let evidence = &task.acceptance.attempts[0].validators[0];
            assert!(!evidence.output.contains("fixture-secret"));
            if name == "pass" {
                assert_eq!(task.column(), TaskColumn::InReview);
            } else {
                assert_eq!(evidence.status, CheckStatus::InfrastructureError);
                assert_eq!(task.acceptance.attempts.len(), 1);
            }
        }
        let (state, _, task) = fixture(file());
        let config = AcceptanceConfig {
            criteria: vec![AcceptanceCriterion {
                id: Uuid::new_v4(),
                label: "Command".into(),
                required: true,
                kind: CriterionKind::Command(CommandCheck {
                    argv: vec!["pass".into()],
                    cwd: ".".into(),
                    timeout_seconds: 1,
                    expected_exit: 0,
                }),
            }],
            ..Default::default()
        };
        let response = request(
            &state,
            &task,
            TaskOperation::ConfigureAcceptance {
                task_id: task.id,
                expected_revision: task.revision,
                config,
                confirmed_commands: vec![],
            },
        );
        assert!(matches!(
            response,
            ServerResponse::TaskResponse(TaskResponse::Error(_))
        ));
    }
    #[test]
    fn cancellation_and_duplicate_start_keep_one_attempt() {
        let (state, project, task) = fixture(file());
        fs::write(project.root.join("wait"), "").unwrap();
        let req = TaskRequest {
            project_id: Some(project.id),
            request_id: Uuid::new_v4(),
            operation: TaskOperation::Start {
                task_id: task.id,
                expected_revision: task.revision,
                execution_profile: AgentExecutionProfile {
                    transport: ditch_core::AgentTransport::AppServer,
                    ..Default::default()
                },
            },
        };
        handle_task_request(Arc::clone(&state), req.clone());
        handle_task_request(Arc::clone(&state), req);
        thread::sleep(Duration::from_millis(250));
        let current = state.lock().unwrap().tasks[&task.id].clone();
        request(
            &state,
            &current,
            TaskOperation::CancelLoop {
                task_id: task.id,
                expected_revision: current.revision,
            },
        );
        let task = wait(&state, task.id);
        assert!(task.acceptance.attempts.len() <= 1);
        assert_eq!(task.column(), TaskColumn::InProgress);
        assert!(task.last_reason.unwrap().contains("cancelled"));
    }
    #[test]
    fn snapshot_paths_limits_and_sensitive_output() {
        let (state, project, _) = fixture(file());
        let first = acceptance_evidence::capture(&project.root).unwrap();
        fs::write(project.root.join("external"), "change").unwrap();
        assert_ne!(
            first.fingerprint,
            acceptance_evidence::capture(&project.root)
                .unwrap()
                .fingerprint
        );
        std::os::unix::fs::symlink("/tmp", project.root.join("escape")).unwrap();
        assert!(acceptance_evidence::contained(&project.root, "escape/test").is_err());
        assert!(acceptance_evidence::contained(&project.root, "../outside").is_err());
        assert_eq!(
            acceptance_evidence::redact("password=secret\nokay"),
            "[sensitive line redacted]\nokay"
        );
        drop(state);
    }
    fn wait_phase(state: &Arc<Mutex<RuntimeState>>, id: TaskId, phase: AttemptResult) {
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            if state.lock().unwrap().tasks[&id]
                .acceptance
                .attempts
                .last()
                .is_some_and(|a| a.result == phase)
            {
                return;
            }
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn validation_cancel_and_external_change_stop_dispatch() {
        for cancel in [false, true] {
            let (state, project, task) = fixture(CriterionKind::Command(CommandCheck {
                argv: vec!["wait-validation".into()],
                cwd: ".".into(),
                timeout_seconds: 5,
                expected_exit: 1,
            }));
            start(&state, &task);
            wait_phase(&state, task.id, AttemptResult::Validating);
            if cancel {
                let current = state.lock().unwrap().tasks[&task.id].clone();
                request(
                    &state,
                    &current,
                    TaskOperation::CancelLoop {
                        task_id: task.id,
                        expected_revision: current.revision,
                    },
                );
            } else {
                fs::write(project.root.join("human-edit"), "external").unwrap();
            }
            let task = wait(&state, task.id);
            assert!(task.last_reason.unwrap().contains(if cancel {
                "cancelled"
            } else {
                "workspace_changed"
            }));
            assert!(task.acceptance.submissions.is_empty());
            assert_eq!(task.acceptance.attempts.len(), 1);
        }
    }
    #[test]
    fn permission_denial_stops_and_does_not_retry() {
        let (state, project, task) = fixture(file());
        fs::write(project.root.join("permission"), "").unwrap();
        start(&state, &task);
        let end = Instant::now() + Duration::from_secs(10);
        let permission = loop {
            let locked = state.lock().unwrap();
            if let Some(id) = locked.app_server_permission_agents.keys().next() {
                break *id;
            }
            drop(locked);
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(15));
        };
        respond_permission_for_target(
            Arc::clone(&state),
            permission,
            codex_app_server::PermissionDecision::Deny,
        );
        let task = wait(&state, task.id);
        assert_eq!(task.acceptance.attempts.len(), 1);
        assert!(task.last_reason.unwrap().contains("cancelled"));
        assert!(!task.acceptance.attempts[0].approvals.is_empty());
    }
    #[test]
    fn validation_tracks_its_process_without_blocking_manual_agents() {
        let (state, project, task) = fixture(CriterionKind::Command(CommandCheck {
            argv: vec!["wait-validation".into()],
            cwd: ".".into(),
            timeout_seconds: 5,
            expected_exit: 1,
        }));
        start(&state, &task);
        wait_phase(&state, task.id, AttemptResult::Validating);
        assert!(
            WriterGuard::acquire(
                &state,
                AgentId::new(),
                &project,
                &AgentExecutionProfile::default()
            )
            .is_ok()
        );
        let root = project.root.parent().unwrap().join("other-project");
        fs::create_dir_all(&root).unwrap();
        let other = Project::new("Other", root);
        let guard = WriterGuard::acquire(
            &state,
            AgentId::new(),
            &other,
            &AgentExecutionProfile::default(),
        )
        .unwrap();
        drop(guard);
        assert!(
            WriterGuard::acquire(
                &state,
                AgentId::new(),
                &other,
                &AgentExecutionProfile {
                    approval: AgentApprovalPreset::FullAccess,
                    ..Default::default()
                }
            )
            .is_ok()
        );
        wait(&state, task.id);
    }
    #[test]
    fn recovery_at_each_active_boundary_preserves_completed_validator_evidence() {
        let (state, project, task) = fixture(file());
        fs::write(project.root.join("pass-after"), "1").unwrap();
        start(&state, &task);
        let completed = wait(&state, task.id);
        for phase in [
            AttemptResult::Queued,
            AttemptResult::Working,
            AttemptResult::Validating,
        ] {
            let mut task = completed.clone();
            task.condition = TaskCondition::Running;
            task.acceptance.attempts[0].result = phase;
            let checks = task.acceptance.attempts[0].validators.clone();
            let mut locked = state.lock().unwrap();
            locked
                .store
                .save_task_changes(
                    &[(
                        task.clone(),
                        task.audit(TaskActor::Daemon, "fixture_boundary", None, None),
                    )],
                    None,
                    None,
                )
                .unwrap();
            locked.store.reconcile_tasks().unwrap();
            let recovered = locked
                .store
                .load_tasks()
                .unwrap()
                .into_iter()
                .find(|t| t.id == task.id)
                .unwrap();
            assert_eq!(recovered.condition, TaskCondition::Blocked);
            assert_eq!(
                recovered.acceptance.attempts[0].result,
                AttemptResult::Interrupted
            );
            assert_eq!(recovered.acceptance.attempts[0].validators, checks);
            assert!(recovered.acceptance.current_submission.is_none());
            assert_eq!(fs::read_to_string(project.root.join("count")).unwrap(), "1");
        }
    }
    #[test]
    fn resume_retry_keeps_execution_profile_and_hashes() {
        let (state, project, mut task) = fixture(file());
        task.acceptance.config.policy.retry_mode = RetryMode::Resume;
        state.lock().unwrap().tasks.insert(task.id, task.clone());
        fs::write(project.root.join("pass-after"), "2").unwrap();
        start(&state, &task);
        let task = wait(&state, task.id);
        assert_eq!(
            task.column(),
            TaskColumn::InReview,
            "{:?}",
            task.last_reason
        );
        assert_eq!(
            task.acceptance.attempts[0].execution_profile,
            task.acceptance.attempts[1].execution_profile
        );
        assert_eq!(
            task.acceptance.attempts[0].thread_id,
            task.acceptance.attempts[1].thread_id
        );
    }
    #[test]
    #[ignore = "requires DITCH_TEST_CODEX; no model turn or user configuration changes"]
    fn installed_codex_validator_respects_attempt_sandbox() {
        let binary = std::env::var("DITCH_TEST_CODEX").unwrap();
        for outside in [false, true] {
            let (state, project, mut task) = fixture(file());
            let path = if outside {
                project.root.parent().unwrap().join("forbidden-write")
            } else {
                project.root.join("allowed-write")
            };
            task.acceptance.config.criteria[0].kind = CriterionKind::Command(CommandCheck {
                argv: vec!["/usr/bin/touch".into(), path.to_string_lossy().into()],
                cwd: ".".into(),
                timeout_seconds: 10,
                expected_exit: if outside { 1 } else { 0 },
            });
            task.state = ditch_core::TaskState::Running;
            task.condition = TaskCondition::Queued;
            task.assigned_agent_id = Some(AgentId::new());
            task.review_summary = Some("Explicit fixture validation".into());
            {
                let mut locked = state.lock().unwrap();
                locked.codex_binary = Some(binary.clone());
                let home = locked.paths.data_dir.join("isolated-code-home");
                fs::create_dir_all(&home).unwrap();
                locked.codex_home = Some(home);
                locked.tasks.insert(task.id, task.clone());
            }
            begin_acceptance(
                Arc::clone(&state),
                task.clone(),
                project,
                AgentExecutionProfile {
                    transport: ditch_core::AgentTransport::AppServer,
                    ..Default::default()
                },
                true,
            );
            let result = wait(&state, task.id);
            assert_eq!(
                result.acceptance.attempts[0].validators[0].status,
                CheckStatus::Passed,
                "{:?}",
                result.acceptance.attempts[0].validators
            );
            assert_eq!(path.exists(), !outside);
        }
    }

    #[test]
    #[ignore = "requires DITCH_TEST_CODEX; temporary command only"]
    fn installed_validator_cancellation_leaves_no_command_process() {
        let (state, project, mut task) = fixture(CriterionKind::Command(CommandCheck {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "echo $$ > validator.pid; exec /bin/sleep 20".into(),
            ],
            cwd: ".".into(),
            timeout_seconds: 20,
            expected_exit: 0,
        }));
        task.state = ditch_core::TaskState::Running;
        task.condition = TaskCondition::Queued;
        task.assigned_agent_id = Some(AgentId::new());
        task.review_summary = Some("Fixture validation".into());
        {
            let mut locked = state.lock().unwrap();
            locked.codex_binary = Some(std::env::var("DITCH_TEST_CODEX").unwrap());
            let home = locked.paths.data_dir.join("isolated-home");
            fs::create_dir_all(&home).unwrap();
            locked.codex_home = Some(home);
            locked.tasks.insert(task.id, task.clone());
        }
        begin_acceptance(
            Arc::clone(&state),
            task.clone(),
            project.clone(),
            AgentExecutionProfile::default(),
            true,
        );
        let end = Instant::now() + Duration::from_secs(10);
        let path = project.root.join("validator.pid");
        while !path.exists() {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(20));
        }
        let pid = fs::read_to_string(path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        let current = state.lock().unwrap().tasks[&task.id].clone();
        request(
            &state,
            &current,
            TaskOperation::CancelLoop {
                task_id: task.id,
                expected_revision: current.revision,
            },
        );
        let task = wait(&state, task.id);
        assert!(task.last_reason.unwrap().contains("cancelled"));
        assert!(!process_group_exists(pid));
        assert!(state.lock().unwrap().writer_leases.is_empty());
    }
    #[test]
    fn git_evidence_includes_staged_binary_changes_without_external_diff() {
        let (_, project, _) = fixture(file());
        let root = &project.root;
        skill_files::git(&["init", "--quiet"], root, 4096).unwrap();
        fs::write(root.join("file.txt"), "base").unwrap();
        fs::write(root.join("binary.bin"), [0, 255, 1]).unwrap();
        skill_files::git(&["add", "file.txt", "binary.bin"], root, 4096).unwrap();
        skill_files::git(
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture",
            ],
            root,
            4096,
        )
        .unwrap();
        let before = acceptance_evidence::capture(root).unwrap();
        assert!(before.head.is_some());
        assert!(before.dirty_paths.is_empty());
        fs::write(root.join("file.txt"), "changed").unwrap();
        fs::write(root.join("binary.bin"), [0, 255, 2]).unwrap();
        skill_files::git(&["add", "file.txt"], root, 4096).unwrap();
        let after = acceptance_evidence::capture(root).unwrap();
        assert_ne!(before.fingerprint, after.fingerprint);
        assert!(after.diff_stat.contains("file.txt"));
        assert!(
            acceptance_evidence::diff(root)
                .unwrap()
                .contains("Binary files")
        );
    }
    #[test]
    fn approved_preset_receipt_prevents_conflicting_replay() {
        let (state, project, _) = fixture(file());
        let mut request = TaskRequest {
            project_id: Some(project.id),
            request_id: Uuid::new_v4(),
            operation: TaskOperation::ApproveTestPreset {
                name: "Tests".into(),
                command: CommandCheck {
                    argv: vec!["pass".into()],
                    cwd: ".".into(),
                    timeout_seconds: 2,
                    expected_exit: 0,
                },
            },
        };
        let first = handle_task_request(Arc::clone(&state), request.clone());
        assert!(
            matches!(
                first,
                ServerResponse::TaskResponse(TaskResponse::TestPresets(_))
            ),
            "{first:?}"
        );
        assert_eq!(
            handle_task_request(Arc::clone(&state), request.clone()),
            first
        );
        if let TaskOperation::ApproveTestPreset { command, .. } = &mut request.operation {
            command.argv = vec!["different".into()];
        }
        assert!(matches!(
            handle_task_request(state, request),
            ServerResponse::TaskResponse(TaskResponse::Error(TaskError {
                code: TaskErrorCode::IdempotencyConflict,
                ..
            }))
        ));
    }
    #[test]
    fn force_kill_during_validation_cancels_the_attempt() {
        let (state, _, task) = fixture(CriterionKind::Command(CommandCheck {
            argv: vec!["wait-validation".into()],
            cwd: ".".into(),
            timeout_seconds: 5,
            expected_exit: 1,
        }));
        start(&state, &task);
        wait_phase(&state, task.id, AttemptResult::Validating);
        let owner = state.lock().unwrap().acceptance_controls[&task.id].owner;
        force_kill_agent(Arc::clone(&state), owner);
        let stopped = wait(&state, task.id);
        assert!(stopped.last_reason.unwrap().contains("cancelled"));
        assert!(stopped.acceptance.submissions.is_empty());
    }
}
