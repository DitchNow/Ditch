mod skill_tests {
    use super::*;
    const FAKE: &str = r#"#!/usr/bin/env python3
import sys,json,pathlib,os
if '--version' in sys.argv: print('codex-cli 0.153.4');sys.exit(0)
roots=[];disabled=set();cwd=pathlib.Path.cwd()
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line);m=v.get('method');p=v.get('params',{});i=v.get('id');result={}
 if m=='initialize': result={'userAgent':'test/0.153.4'}
 elif m=='initialized': continue
 elif m=='skills/extraRoots/set':
  if (cwd/'no-extra').exists():send({'id':i,'error':{'code':-32601,'message':'extra roots unsupported'}});continue
  roots=p['extraRoots']
 elif m=='skills/list':
  groups=[]
  for directory in p['cwds']:
   skills=[]
   for root in roots+[str(pathlib.Path(directory)/'.agents/skills')]:
    for manifest in pathlib.Path(root).rglob('SKILL.md'):
     text=manifest.read_text();fields={}
     for row in text.splitlines()[1:]:
      if row=='---':break
      if ':' in row:
       k,val=row.split(':',1);fields[k]=val.strip()
     if not fields.get('name') or not fields.get('description'):continue
     extra={};meta=manifest.parent/'SKILL.json'
     if meta.exists():extra=json.loads(meta.read_text())
     skills.append(dict(name=fields['name'],description=fields['description'],path=str(manifest.resolve()),scope='user',enabled=str(manifest) not in disabled,**extra))
   groups.append(dict(cwd=directory,skills=skills,errors=[]))
  result={'data':groups}
 elif m=='skills/config/write':
  if p['enabled']:disabled.discard(p['path'])
  else:disabled.add(p['path'])
 elif m in ['thread/start','thread/resume']:
  with (cwd/'rpc.jsonl').open('a') as f:f.write(json.dumps(v)+'\n')
  result={'thread':{'id':p.get('threadId','fixture-thread')}}
  send({'id':i,'result':result});send({'id':i,'result':result});continue
 elif m=='turn/start':
  with (cwd/'rpc.jsonl').open('a') as f:f.write(json.dumps(v)+'\n')
  send({'id':i,'result':{'turn':{'id':'fixture-turn'}}})
  send({'method':'turn/started','params':{'threadId':p['threadId'],'turn':{'id':'fixture-turn'}}})
  if p['input'][0]['text']=='wait':continue
  send({'method':'item/agentMessage/delta','params':{'delta':'Test '}})
  send({'method':'item/agentMessage/delta','params':{'delta':'result'}})
  send({'method':'item/completed','params':{'item':{'type':'agentMessage','text':'Test result'}}})
  send({'method':'turn/completed','params':{'turn':{'status':'completed'}}})
  os._exit(0)
 else:send({'id':i,'error':{'code':-32601,'message':'unsupported'}});continue
 send({'id':i,'result':result})
"#;
    fn fixture() -> (Arc<Mutex<RuntimeState>>, Project, SkillSource) {
        let mut runtime = super::tests::test_runtime();
        let root = runtime.paths.data_dir.join("project");
        fs::create_dir_all(&root).unwrap();
        let mut project = Project::new("Skills", fs::canonicalize(&root).unwrap());
        project.git_policy = ProjectGitPolicy::AllowOutsideGit;
        runtime.store.upsert_project(&project).unwrap();
        runtime.projects.insert(project.root_key(), project.clone());
        let binary = runtime.paths.data_dir.join("fake-codex");
        fs::write(&binary, FAKE).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        runtime.codex_binary = Some(binary.to_string_lossy().into());
        let source_root = runtime.paths.data_dir.join("library");
        fs::create_dir_all(&source_root).unwrap();
        for name in ["one", "two"] {
            fs::create_dir_all(source_root.join(name)).unwrap();
            fs::write(
                source_root.join(name).join("SKILL.md"),
                "---\nname: same-name\ndescription: Safe fixture\n---\nInstructions\n",
            )
            .unwrap();
        }
        fs::write(source_root.join("setup.sh"), "touch SHOULD_NOT_RUN").unwrap();
        let source = SkillSource {
            id: Uuid::new_v4(),
            name: "Fixture library".into(),
            location: source_root.to_string_lossy().into(),
            reference: "HEAD".into(),
            seeded: false,
        };
        let mut catalog = SkillCatalog::default();
        catalog.sources.push(source.clone());
        save_skill_catalog(&mut runtime, &catalog).unwrap();
        (Arc::new(Mutex::new(runtime)), project, source)
    }
    fn op(
        state: &Arc<Mutex<RuntimeState>>,
        project: &Project,
        operation: SkillOperation,
    ) -> Result<SkillResponse, String> {
        skill_operation(
            state,
            SkillRequest {
                project_id: Some(project.id),
                operation,
            },
        )
    }
    fn prepare(
        state: &Arc<Mutex<RuntimeState>>,
        project: &Project,
        source: &SkillSource,
    ) -> SkillInstallPlan {
        let SkillResponse::Plan(plan) = op(
            state,
            project,
            SkillOperation::PrepareInstall {
                source_id: source.id,
                relative_path: "one".into(),
                allow_network: false,
            },
        )
        .unwrap() else {
            panic!()
        };
        *plan
    }
    fn binding(entry: &SkillEntry) -> SkillBinding {
        SkillBinding {
            identity: entry.identity.clone(),
            name: entry.name.clone(),
            path: entry.path.clone(),
            content_hash: entry.content_hash.clone(),
            revision: entry.revision.clone(),
            origin: TaskActor::User,
            reason: None,
            created_at: Utc::now(),
        }
    }
    #[test]
    fn selective_install_updates_rollback_recovery_and_removal() {
        let (state, project, source) = fixture();
        let plan = prepare(&state, &project, &source);
        assert!(
            skill_catalog(&state.lock().unwrap())
                .unwrap()
                .installed
                .is_empty()
        );
        assert!(!plan.destination.exists());
        assert_eq!(plan.included_paths, vec!["SKILL.md"]);
        assert!(plan.entry.license.is_none());
        let first = confirm_skill_plan(&state, &project, plan.id).unwrap();
        assert_eq!(
            first,
            confirm_skill_plan(&state, &project, plan.id).unwrap()
        );
        fs::write(
            Path::new(&source.location).join("one/SKILL.md"),
            "---\nname: same-name\ndescription: Revised fixture\n---\nVersion two\n",
        )
        .unwrap();
        let next = prepare(&state, &project, &source);
        assert_eq!(next.prior_revision, Some(first.content_hash.clone()));
        // A crash after filesystem promotion but before committing metadata is recoverable.
        let staged = skill_root(&state.lock().unwrap())
            .join("plans")
            .join(next.id.to_string())
            .join("skill");
        fs::create_dir_all(next.destination.parent().unwrap()).unwrap();
        fs::rename(staged, &next.destination).unwrap();
        let second = confirm_skill_plan(&state, &project, next.id).unwrap();
        assert_ne!(first.content_hash, second.content_hash);
        assert!(validate_selected_skills(&state, &project, &[binding(&first)]).is_ok());
        op(
            &state,
            &project,
            SkillOperation::Rollback {
                identity: first.identity.clone(),
                content_hash: first.content_hash.clone(),
            },
        )
        .unwrap();
        let restored = skill_catalog(&state.lock().unwrap()).unwrap();
        assert_eq!(
            restored.installed[0].versions[restored.installed[0].current]
                .entry
                .content_hash,
            first.content_hash
        );
        op(
            &state,
            &project,
            SkillOperation::SetEnabled {
                identity: first.identity.clone(),
                enabled: false,
            },
        )
        .unwrap();
        assert!(validate_selected_skills(&state, &project, &[binding(&first)]).is_err());
        op(
            &state,
            &project,
            SkillOperation::Remove {
                identity: first.identity.clone(),
            },
        )
        .unwrap();
        assert!(!first.path.exists());
        assert!(!project.root.join("SHOULD_NOT_RUN").exists());
    }
    #[test]
    fn discovery_duplicate_names_pagination_and_hash_pins() {
        let (state, project, source) = fixture();
        let (entries, errors) = discover_skills(&state, &project).unwrap();
        assert!(errors.is_empty());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, entries[1].name);
        assert_ne!(entries[0].identity, entries[1].identity);
        let (first, next) = page(&entries, 0, 1);
        assert_eq!(first.len(), 1);
        assert_eq!(next, Some(1));
        assert!(page(&entries, usize::MAX, 100).0.is_empty());
        let selected = binding(&entries[0]);
        assert!(
            validate_selected_skills(&state, &project, std::slice::from_ref(&selected)).is_ok()
        );
        fs::write(
            &selected.path,
            "---\nname: same-name\ndescription: Changed\n---\nChanged instructions",
        )
        .unwrap();
        assert!(
            validate_selected_skills(&state, &project, &[selected])
                .unwrap_err()
                .contains("changed")
        );
        assert!(fetch_skill_source(&state, &SkillCatalog::default().sources[0], false).is_err());
        assert!(
            source_skill_entry(
                &source,
                &FetchedSkills {
                    root: PathBuf::from(&source.location),
                    git: false,
                    revision: None,
                    temporary: false
                },
                "../project"
            )
            .is_err()
        );
    }
    #[test]
    fn bindings_are_durable_revision_checked_and_block_removal() {
        let (state, project, source) = fixture();
        let plan = prepare(&state, &project, &source);
        let entry = confirm_skill_plan(&state, &project, plan.id).unwrap();
        let response = handle_task_request(
            Arc::clone(&state),
            TaskRequest {
                project_id: Some(project.id),
                request_id: Uuid::new_v4(),
                operation: TaskOperation::Create {
                    draft: ditch_core::TaskDraft {
                        skills: vec![],
                        title: "Bound task".into(),
                        description: String::new(),
                        acceptance_criteria: vec![],
                        priority: ditch_core::TaskPriority::Normal,
                    },
                },
            },
        );
        let ServerResponse::TaskResponse(TaskResponse::Changed(task)) = response else {
            panic!()
        };
        let bound = op(
            &state,
            &project,
            SkillOperation::Bind {
                task_id: task.id,
                expected_revision: task.revision,
                skills: vec![binding(&entry)],
            },
        )
        .unwrap();
        let SkillResponse::Bound(bound) = bound else {
            panic!()
        };
        assert_eq!(bound.skills[0].content_hash, entry.content_hash);
        assert!(
            op(
                &state,
                &project,
                SkillOperation::Bind {
                    task_id: task.id,
                    expected_revision: task.revision,
                    skills: vec![]
                }
            )
            .is_err()
        );
        assert!(
            op(
                &state,
                &project,
                SkillOperation::Remove {
                    identity: entry.identity.clone()
                }
            )
            .is_err()
        );
        let paths = state.lock().unwrap().paths.clone();
        drop(state);
        let restored = RuntimeState::new(paths, false).unwrap();
        assert_eq!(restored.tasks[&task.id].skills, bound.skills);
    }
    #[test]
    fn invalid_skill_payloads_cannot_escape_or_execute() {
        let good = SkillFile {
            path: "SKILL.md".into(),
            content_base64: base64::engine::general_purpose::STANDARD
                .encode("---\nname: good\ndescription: Good fixture\n---\nText"),
            executable: false,
        };
        for path in [
            "../escape",
            "/absolute",
            "a/../../escape",
            "a\\escape",
            ".git/config",
            "a\nfile",
        ] {
            let mut bad = good.clone();
            bad.path = path.into();
            assert!(skill_files::validate_files(&[good.clone(), bad]).is_err());
        }
        assert!(skill_files::validate_files(&[good.clone(), good.clone()]).is_err());
        let mut bad = good.clone();
        bad.content_base64 = base64::engine::general_purpose::STANDARD.encode("no frontmatter");
        assert!(skill_files::validate_files(&[bad]).is_err());
        let root = PathBuf::from("/tmp").join(format!("dsv-{}", Uuid::new_v4()));
        skill_files::write_tree(&root, &[good]).unwrap();
        std::os::unix::fs::symlink("/tmp", root.join("escape")).unwrap();
        assert!(skill_files::read_tree(&root).is_err());
        fs::remove_file(root.join("escape")).unwrap();
        let path = root.join("huge");
        fs::File::create(&path)
            .unwrap()
            .set_len((skill_files::MAX_FILE + 1) as u64)
            .unwrap();
        assert!(skill_files::read_tree(&root).is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::net::UnixListener::bind(root.join("special")).unwrap();
        assert!(skill_files::read_tree(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn local_and_remote_policies_honor_explicit_user_presets() {
        let root = Path::new("/project");
        for preset in [
            AgentApprovalPreset::Ask,
            AgentApprovalPreset::ApproveForMe,
            AgentApprovalPreset::FullAccess,
        ] {
            let profile = AgentExecutionProfile {
                approval: preset.clone(),
                ..Default::default()
            };
            let (approval, sandbox, policy) =
                codex_app_server::execution_policy(&profile, root, false);
            if preset == AgentApprovalPreset::FullAccess {
                assert_eq!(sandbox, "danger-full-access");
                assert_eq!(policy["type"], "dangerFullAccess");
            } else {
                assert_eq!(sandbox, "workspace-write");
                assert_eq!(policy["writableRoots"], serde_json::json!(["/project"]));
                assert_eq!(policy["excludeSlashTmp"], true);
                assert_eq!(policy["excludeTmpdirEnvVar"], true);
                assert_eq!(
                    approval,
                    if preset == AgentApprovalPreset::Ask {
                        "on-request"
                    } else {
                        "never"
                    }
                );
            }
            assert_eq!(
                codex_app_server::execution_policy(&profile, root, true),
                (approval, sandbox, policy)
            );
        }
    }
    #[test]
    fn unavailable_app_server_falls_back_without_claiming_skills() {
        let (state, project, _) = fixture();
        let binary = state.lock().unwrap().codex_binary.clone().unwrap();
        fs::write(
            &binary,
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo codex-cli 0.153.4; exit 0; fi\nif [ \"$1\" = app-server ]; then exit 1; fi\nexit 0\n",
        )
        .unwrap();
        let response = start_codex_session(
            Arc::clone(&state),
            project.name.clone(),
            project.root.to_string_lossy().into(),
            "work".into(),
            CodexLaunchMode::Exec,
            AgentExecutionProfile {
                transport: ditch_core::AgentTransport::AppServer,
                ..Default::default()
            },
        );
        let ServerResponse::AgentStarted(run) = response else {
            panic!("{response:?}")
        };
        assert_eq!(
            run.execution_profile.transport,
            ditch_core::AgentTransport::Legacy
        );
        assert!(run.execution_profile.skills.is_empty());
    }

    #[test]
    fn local_turn_records_explicit_skill_and_drains_exit_race() {
        let (state, project, _source) = fixture();
        let (entries, _) = discover_skills(&state, &project).unwrap();
        let skill = binding(&entries[0]);
        let profile = AgentExecutionProfile {
            transport: ditch_core::AgentTransport::AppServer,
            skills: vec![skill.clone()],
            ..Default::default()
        };
        let response = start_codex_session(
            Arc::clone(&state),
            project.name.clone(),
            project.root.to_string_lossy().into(),
            "work".into(),
            CodexLaunchMode::Exec,
            profile,
        );
        let ServerResponse::AgentStarted(run) = response else {
            panic!("{response:?}")
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let locked = state.lock().unwrap();
            if !locked.children.contains_key(&run.id) {
                break;
            }
            drop(locked);
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let locked = state.lock().unwrap();
        let record = &locked.agents[&run.id];
        assert_eq!(record.run.state, AgentState::Completed);
        assert_eq!(record.run.execution_profile.skills, vec![skill.clone()]);
        assert!(record.messages.iter().any(|m| m.text == "Test result"));
        assert!(locked.writer_leases.is_empty());
        drop(locked);
        let requests = fs::read_to_string(project.root.join("rpc.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap())
            .collect::<Vec<_>>();
        let turns = requests
            .iter()
            .filter(|v| v["method"] == "turn/start")
            .collect::<Vec<_>>();
        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0]["params"]["input"][1]["path"],
            serde_json::json!(skill.path)
        );
        assert_eq!(
            turns[0]["params"]["sandboxPolicy"]["type"],
            "workspaceWrite"
        );
        let response = prompt_agent(
            Arc::clone(&state),
            run.id,
            "wait".into(),
            AgentExecutionProfile::default(),
        );
        assert_eq!(response, ServerResponse::Accepted);
        assert!(matches!(
            stop_agent(Arc::clone(&state), run.id),
            ServerResponse::Accepted
        ));
        assert_eq!(
            state.lock().unwrap().agents[&run.id].run.state,
            AgentState::Interrupted
        );
    }
    #[test]
    fn remote_import_requires_matching_hash_and_keeps_targets_separate() {
        let (state, project, source) = fixture();
        let files = skill_files::read_tree(&Path::new(&source.location).join("one")).unwrap();
        let hash = skill_files::validate_files(&files).unwrap().0;
        let request = SkillOperation::Import {
            resolved_revision: None,
            license: None,
            source: source.clone(),
            relative_path: "one".into(),
            content_hash: hash.clone(),
            files: files.clone(),
        };
        assert!(
            op(&state, &project, request.clone())
                .unwrap_err()
                .contains("SSH")
        );
        state.lock().unwrap().remote_runtime = true;
        let mut corrupted = files.clone();
        corrupted[0].content_base64 = base64::engine::general_purpose::STANDARD
            .encode("---\nname: same-name\ndescription: Wrong revision\n---\nOther content");
        assert!(
            op(
                &state,
                &project,
                SkillOperation::Import {
                    resolved_revision: None,
                    license: None,
                    source,
                    relative_path: "one".into(),
                    content_hash: hash.clone(),
                    files: corrupted
                }
            )
            .unwrap_err()
            .contains("checksum")
        );
        let SkillResponse::Installed(entry) = op(&state, &project, request).unwrap() else {
            panic!()
        };
        assert_eq!(entry.content_hash, hash);
        assert!(entry.path.starts_with(skill_root(&state.lock().unwrap())));
    }

    #[test]
    fn declared_missing_dependencies_block_assignment() {
        let (state, project, source) = fixture();
        fs::write(Path::new(&source.location).join("one/SKILL.json"),r#"{"dependencies":{"tools":[{"type":"env_var","value":"DITCH_PHASE2_TEST_ABSENT_TOKEN"}]}}"#).unwrap();
        let (entries, _) = discover_skills(&state, &project).unwrap();
        let skill = entries
            .iter()
            .find(|e| !e.missing_dependencies.is_empty())
            .unwrap();
        assert!(
            validate_selected_skills(&state, &project, &[binding(skill)])
                .unwrap_err()
                .contains("dependency")
        );
    }

    #[test]
    fn git_collection_reads_only_selected_regular_blobs() {
        let (state, project, source) = fixture();
        let root = PathBuf::from(&source.location);
        skill_files::git(&["init", "--quiet"], &root, 4096).unwrap();
        skill_files::git(&["add", "one", "two", "setup.sh"], &root, 4096).unwrap();
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
            &root,
            4096,
        )
        .unwrap();
        let fetched = FetchedSkills {
            root,
            git: true,
            revision: Some("HEAD".into()),
            temporary: false,
        };
        assert_eq!(source_skill_paths(&fetched).unwrap(), vec!["one", "two"]);
        let (entry, files) = source_skill_entry(&source, &fetched, "one").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "SKILL.md");
        assert_eq!(entry.name, "same-name");
        assert!(!project.root.join("SHOULD_NOT_RUN").exists());
        assert!(
            skill_catalog(&state.lock().unwrap())
                .unwrap()
                .installed
                .is_empty()
        );
    }

    #[test]
    #[ignore = "requires DITCH_TEST_CODEX pointing to an installed compatible Codex CLI"]
    fn installed_codex_recognizes_managed_fixture_and_enforces_workspace() {
        let (state, project, source) = fixture();
        let home = state
            .lock()
            .unwrap()
            .paths
            .data_dir
            .join("isolated-codex-home");
        fs::create_dir_all(&home).unwrap();
        {
            let mut locked = state.lock().unwrap();
            locked.codex_binary = Some(std::env::var("DITCH_TEST_CODEX").unwrap());
            locked.codex_home = Some(home.clone());
        }
        let plan = prepare(&state, &project, &source);
        let installed = confirm_skill_plan(&state, &project, plan.id).unwrap();
        assert!(
            discover_skills(&state, &project)
                .unwrap()
                .0
                .iter()
                .any(|e| e.identity == installed.identity && e.recognized)
        );
        assert!(
            !home.join("config.toml").exists(),
            "Extra roots must not rewrite Codex config"
        );
        let policy = codex_app_server::execution_policy(
            &AgentExecutionProfile::default(),
            &project.root,
            false,
        )
        .2;
        let result=skill_client(&state,&project).unwrap().call("command/exec",serde_json::json!({"command":["/bin/sh","-c","touch allowed; touch ../outside-marker"],"cwd":project.root,"sandboxPolicy":policy,"timeoutMs":5000})).unwrap();
        assert!(project.root.join("allowed").exists());
        assert!(
            !project
                .root
                .parent()
                .unwrap()
                .join("outside-marker")
                .exists()
        );
        assert_ne!(result["exitCode"], 0);
    }

    #[test]
    fn unsupported_extra_roots_preserve_previous_install() {
        let (state, project, source) = fixture();
        let plan = prepare(&state, &project, &source);
        confirm_skill_plan(&state, &project, plan.id).unwrap();
        fs::write(project.root.join("no-extra"), "").unwrap();
        assert!(
            op(
                &state,
                &project,
                SkillOperation::PrepareInstall {
                    source_id: source.id,
                    relative_path: "two".into(),
                    allow_network: false
                }
            )
            .is_err()
        );
        assert_eq!(
            skill_catalog(&state.lock().unwrap())
                .unwrap()
                .installed
                .len(),
            1
        );
    }
}
