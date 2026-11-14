// Included in the shared runtime tests for both editions.
fn naming_fixture(mode: &str) -> (Arc<Mutex<RuntimeState>>, AgentId, PathBuf) {
    let mut runtime = test_runtime();
    let id = recovery_agent(&mut runtime);
    let root = runtime.paths.data_dir.clone();
    fs::write(root.join("mode"), mode).unwrap();
    let binary = root.join("fake-codex");
    let script = r#"#!/usr/bin/python3
import sys, json, pathlib, time
root = pathlib.Path(__file__).parent
def emit(v): print(json.dumps(v), flush=True)
for line in sys.stdin:
    r = json.loads(line); method = r.get('method'); p = r.get('params', {})
    with open(root / 'calls.jsonl', 'a') as f: f.write(json.dumps(r) + '\n')
    if 'id' not in r: continue
    mode = (root / 'mode').read_text()
    result = {}
    if method == 'config/read': result = {'config': {'mcp_servers': {'private': {'enabled': True}}}}
    elif method == 'thread/read': result = {'thread': {'name': 'Existing Codex name' if mode == 'existing' else None}}
    elif method == 'model/list': result = {'data': [{'model': 'fixture', 'supportedReasoningEfforts': [{'reasoningEffort': 'low'}]}]}
    elif method == 'thread/start':
        assert p['ephemeral'] and p['sandbox'] == 'read-only'
        assert p['config']['features']['shell_tool'] is False
        assert p['config']['mcp_servers']['private']['enabled'] is False
        assert p['cwd'] == str(root / 'session-naming')
        if (root / 'oss-provider').exists():
            expected = (root / 'oss-provider').read_text()
            assert p['modelProvider'] == expected
            assert p['model'] == 'qwen3.5:9b'
            assert 'model_context_window=65536' in sys.argv
            result = {'thread':{'id':'internal-name-thread','ephemeral':True,'modelProvider': 'openai' if mode=='wrong-provider' else expected},'model': 'wrong-model' if mode=='wrong-model' else p['model']}
        else:
            result = {'thread': {'id': 'internal-name-thread', 'ephemeral': mode != 'unsupported'}, 'model': 'fixture'}
    elif method == 'turn/start':
        assert p['threadId'] == 'internal-name-thread'
        assert p['effort'] == ('none' if (root / 'oss-provider').exists() else 'low') and p['outputSchema']['required'] == ['title']
        assert p['input'][0]['text'] == 'original prompt'
        (root / 'generating').touch()
        if mode == 'delayed':
            while not (root / 'continue').exists(): time.sleep(0.01)
        if mode == 'fail':
            emit({'id': r['id'], 'error': {'code': -1, 'message': 'fixture failure'}}); continue
        emit({'id': r['id'], 'result': {'turn': {'id': 'naming-turn'}}})
        if mode == 'tool':
            emit({'id': 'tool-request', 'method': 'item/commandExecution/requestApproval', 'params': {'threadId': 'internal-name-thread'}}); continue
        text = 'invalid json' if mode == 'malformed' else json.dumps({'title': 'Fix login validation'})
        emit({'method': 'item/completed', 'params': {'threadId': 'different-thread', 'item': {'type': 'agentMessage', 'text': 'invalid'}}})
        emit({'method': 'item/completed', 'params': {'threadId': 'internal-name-thread', 'item': {'type': 'agentMessage', 'text': text}}})
        emit({'method': 'turn/completed', 'params': {'threadId': 'internal-name-thread', 'turn': {'status': 'completed'}}})
        continue
    elif method == 'thread/name/set':
        assert p['threadId'] == 'saved-thread'
        if mode == 'save-fails':
            emit({'id': r['id'], 'error': {'code': -1, 'message': 'save failed'}}); continue
        (root / 'saved-name').write_text(p['name'])
    emit({'id': r['id'], 'result': result})
"#;
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    runtime.codex_binary = Some(binary.to_string_lossy().into_owned());
    // Keep these tests entirely outside the user's Codex home.
    runtime.agents.get_mut(&id).unwrap().run.origin_codex_home =
        Some(root.join("fake-home").to_string_lossy().into_owned());
    (Arc::new(Mutex::new(runtime)), id, root)
}

fn naming_calls(root: &Path, method: &str) -> usize {
    fs::read_to_string(root.join("calls.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|line| serde_json::from_str::<Value>(line).unwrap()["method"] == method)
        .count()
}

fn allow_naming_retry(state: &Arc<Mutex<RuntimeState>>, id: AgentId) {
    let mut locked = state.lock().unwrap();
    let key = name_receipt_key(id);
    let mut receipt: SessionNameReceipt =
        serde_json::from_str(&locked.store.setting(&key).unwrap().unwrap()).unwrap();
    receipt.retry_at = 0;
    locked
        .store
        .set_setting(&key, &serde_json::to_string(&receipt).unwrap())
        .unwrap();
}

#[test]
fn session_naming_generates_and_saves_without_creating_an_agent_or_touching_work() {
    let (state, id, root) = naming_fixture("success");
    let before = state.lock().unwrap().agents[&id].run.clone();
    process_session_name_job(&state);
    {
        let locked = state.lock().unwrap();
        assert_eq!(locked.agents.len(), 1);
        assert_eq!(locked.projects.len(), 1);
        assert!(locked.tasks.is_empty());
        assert!(locked.children.is_empty());
        assert!(locked.pending_permissions.is_empty());
        assert!(locked.attention.is_empty());
        let record = &locked.agents[&id];
        assert_eq!(
            record.run.codex_title.as_deref(),
            Some("Fix login validation")
        );
        assert_eq!(record.run.native_session_id, before.native_session_id);
        assert_eq!(record.run.state, before.state);
        assert_eq!(record.run.can_stop, before.can_stop);
        assert_eq!(record.messages.len(), 1);
        assert_eq!(locked.store.load().unwrap().agents.len(), 1);
    }
    assert_eq!(
        fs::read_to_string(root.join("saved-name")).unwrap(),
        "Fix login validation"
    );
    process_session_name_job(&state);
    assert_eq!(naming_calls(&root, "turn/start"), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oss_session_never_claims_an_openai_naming_job() {
    let (state, id, root) = naming_fixture("success");
    state
        .lock()
        .unwrap()
        .agents
        .get_mut(&id)
        .unwrap()
        .run
        .execution_profile
        .inference_endpoint_id = Some(Uuid::new_v4());
    assert!(claim_session_name_job(&state).is_none());
    assert_eq!(naming_calls(&root, "turn/start"), 0);
    fs::remove_dir_all(root).unwrap();
}

fn oss_naming_fixture(
    mode: &str,
) -> (
    Arc<Mutex<RuntimeState>>,
    AgentId,
    PathBuf,
    ollama::ResolvedOllamaProvider,
) {
    let (state, id, root) = naming_fixture(mode);
    let endpoint_id = Uuid::new_v4();
    let provider = ollama::ResolvedOllamaProvider {
        endpoint: ditch_core::ollama::OllamaEndpoint {
            id: endpoint_id,
            display_name: "GPU".into(),
            api_root_url: "http://127.0.0.1:1".into(),
            auth_mode: "none".into(),
            has_credentials: false,
            allow_private_http: true,
            enabled: true,
            created_at: Utc::now(),
            models: vec![],
            reachable: None,
            error: None,
            model_status: Default::default(),
        },
        token: None,
        model: "qwen3.5:9b".into(),
        context_window: 65536,
    };
    fs::write(root.join("oss-provider"), provider.endpoint.provider_key()).unwrap();
    {
        let mut locked = state.lock().unwrap();
        let run = &mut locked.agents.get_mut(&id).unwrap().run;
        run.execution_profile.model = Some(provider.model.clone());
        run.execution_profile.inference_endpoint_id = Some(endpoint_id);
        run.can_stop = false;
        run.state = AgentState::Completed;
        run.finished_at = Some(Utc::now());
        run.provisional_title = Some("original prompt".into());
    }
    (state, id, root, provider)
}

#[test]
fn oss_session_naming_uses_bound_provider_on_local_and_remote_owner() {
    for remote in [false, true] {
        let (state, id, root, provider) = oss_naming_fixture("success");
        {
            let mut locked = state.lock().unwrap();
            locked.remote_runtime = remote;
            // A desktop's replica must never claim its SSH owner's naming work.
            locked
                .projects
                .values_mut()
                .next()
                .unwrap()
                .execution_target = ProjectExecutionTarget::Remote {
                remote_machine_id: Uuid::new_v4(),
                ssh_host_alias: "fixture-owner".into(),
            };
        }
        assert!(claim_session_name_job(&state).is_none());
        state
            .lock()
            .unwrap()
            .projects
            .values_mut()
            .next()
            .unwrap()
            .execution_target = ProjectExecutionTarget::Local;
        let mut job = claim_session_name_job(&state).unwrap();
        generate_session_name_with_provider(&state, &mut job, Some(&provider)).unwrap();
        let run = state.lock().unwrap().agents[&id].run.clone();
        assert_eq!(run.display_name(), "Fix login validation");
        assert!(run.provisional_title.is_none());
        assert_eq!(run.native_session_id.as_deref(), Some("saved-thread"));
        assert_eq!(state.lock().unwrap().agents[&id].messages.len(), 1);
        assert_eq!(naming_calls(&root, "model/list"), 0);
        assert_eq!(naming_calls(&root, "turn/start"), 1);
        assert!(claim_session_name_job(&state).is_none());
        assert_eq!(
            state.lock().unwrap().store.load().unwrap().agents[0]
                .run
                .codex_title,
            run.codex_title
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn oss_session_naming_waits_and_never_falls_back_to_openai() {
    let (state, id, root, _) = oss_naming_fixture("success");
    state
        .lock()
        .unwrap()
        .agents
        .get_mut(&id)
        .unwrap()
        .run
        .can_stop = true;
    assert!(claim_session_name_job(&state).is_none());
    state
        .lock()
        .unwrap()
        .agents
        .get_mut(&id)
        .unwrap()
        .run
        .can_stop = false;
    let mut job = claim_session_name_job(&state).unwrap();
    // Endpoint is intentionally absent from the store.
    assert!(generate_session_name(&state, &mut job).is_err());
    assert_eq!(naming_calls(&root, "initialize"), 0);
    assert_eq!(
        state.lock().unwrap().agents[&id].run.display_name(),
        "original prompt"
    );
    assert!(generate_session_name_with_provider(&state, &mut job, None).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oss_session_naming_preserves_legacy_title_until_explicit_regeneration() {
    let (state, id, root, provider) = oss_naming_fixture("existing");
    state
        .lock()
        .unwrap()
        .agents
        .get_mut(&id)
        .unwrap()
        .run
        .codex_title = Some("Legacy title".into());
    assert!(claim_session_name_job(&state).is_none());
    assert!(matches!(
        rename_agent(state.clone(), id, None),
        ServerResponse::Accepted
    ));
    assert_eq!(
        state.lock().unwrap().agents[&id].run.display_name(),
        "Legacy title"
    );
    let mut job = claim_session_name_job(&state).unwrap();
    generate_session_name_with_provider(&state, &mut job, Some(&provider)).unwrap();
    assert_eq!(
        state.lock().unwrap().agents[&id].run.display_name(),
        "Fix login validation"
    );
    assert_eq!(naming_calls(&root, "thread/read"), 0);
    assert!(claim_session_name_job(&state).is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oss_session_naming_rejects_provider_model_and_generation_changes() {
    for mode in ["wrong-provider", "wrong-model"] {
        let (state, _, root, provider) = oss_naming_fixture(mode);
        let mut job = claim_session_name_job(&state).unwrap();
        assert!(generate_session_name_with_provider(&state, &mut job, Some(&provider)).is_err());
        assert_eq!(naming_calls(&root, "turn/start"), 0);
        fs::remove_dir_all(root).unwrap();
    }
    for change in ["model", "endpoint", "regenerate"] {
        let (state, id, root, provider) = oss_naming_fixture("success");
        let mut job = claim_session_name_job(&state).unwrap();
        match change {
            "model" => {
                state
                    .lock()
                    .unwrap()
                    .agents
                    .get_mut(&id)
                    .unwrap()
                    .run
                    .execution_profile
                    .model = Some("another-model".into())
            }
            "endpoint" => {
                state
                    .lock()
                    .unwrap()
                    .agents
                    .get_mut(&id)
                    .unwrap()
                    .run
                    .execution_profile
                    .inference_endpoint_id = Some(Uuid::new_v4())
            }
            _ => {
                rename_agent(state.clone(), id, None);
            }
        }
        generate_session_name_with_provider(&state, &mut job, Some(&provider)).unwrap();
        assert_eq!(naming_calls(&root, "initialize"), 0);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn oss_session_naming_reuses_result_after_save_failure() {
    let (state, id, root, provider) = oss_naming_fixture("save-fails");
    let mut job = claim_session_name_job(&state).unwrap();
    assert!(generate_session_name_with_provider(&state, &mut job, Some(&provider)).is_err());
    allow_naming_retry(&state, id);
    fs::write(root.join("mode"), "success").unwrap();
    let mut job = claim_session_name_job(&state).unwrap();
    generate_session_name_with_provider(&state, &mut job, Some(&provider)).unwrap();
    assert_eq!(naming_calls(&root, "turn/start"), 1);
    assert_eq!(naming_calls(&root, "thread/name/set"), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oss_session_naming_preserves_manual_edit_during_generation() {
    let (state, id, root, provider) = oss_naming_fixture("delayed");
    let mut job = claim_session_name_job(&state).unwrap();
    let worker_state = state.clone();
    let worker = thread::spawn(move || {
        generate_session_name_with_provider(&worker_state, &mut job, Some(&provider)).unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("generating").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    rename_agent(state.clone(), id, Some("My OSS session".into()));
    fs::write(root.join("continue"), "").unwrap();
    worker.join().unwrap();
    assert_eq!(
        state.lock().unwrap().agents[&id].run.display_name(),
        "My OSS session"
    );
    assert_eq!(naming_calls(&root, "thread/name/set"), 0);
    assert_eq!(naming_calls(&root, "model/list"), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn oss_session_naming_failure_keeps_fallback_across_restart_and_explicit_retry() {
    let (state, id, root, _) = oss_naming_fixture("success");
    // An unavailable endpoint must exhaust bounded retries without another provider.
    process_session_name_job(&state);
    allow_naming_retry(&state, id);
    process_session_name_job(&state);
    process_session_name_job(&state);
    {
        let locked = state.lock().unwrap();
        assert!(locked.agents[&id].run.naming_failed);
        assert_eq!(locked.agents[&id].run.display_name(), "original prompt");
        let restarted = Arc::new(Mutex::new(
            RuntimeState::new(locked.paths.clone(), false).unwrap(),
        ));
        assert!(claim_session_name_job(&restarted).is_none());
        assert_eq!(
            restarted.lock().unwrap().agents[&id].run.display_name(),
            "original prompt"
        );
    }
    rename_agent(state.clone(), id, None);
    let job = claim_session_name_job(&state).unwrap();
    assert!(job.receipt.force_generation);
    assert_eq!(job.receipt.attempts, 1);
    assert_eq!(naming_calls(&root, "initialize"), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_naming_provisional_title_is_backward_compatible() {
    let (state, id, root, _) = oss_naming_fixture("success");
    let run = state.lock().unwrap().agents[&id].run.clone();
    let mut saved = serde_json::to_value(&run).unwrap();
    saved.as_object_mut().unwrap().remove("provisional_title");
    let old: AgentRun = serde_json::from_value(saved).unwrap();
    assert!(old.provisional_title.is_none());
    assert_eq!(old.display_name(), "Naming session…");
    assert_eq!(run.display_name(), "original prompt");
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "Set DITCH_OLLAMA_TEST_URL and DITCH_OLLAMA_TEST_MODEL; two synthetic live turns"]
fn live_oss_session_naming_preserves_main_conversation() {
    use serde_json::json;
    let (state, id, root, mut provider) = oss_naming_fixture("success");
    provider.endpoint.api_root_url = std::env::var("DITCH_OLLAMA_TEST_URL").unwrap();
    provider.model = std::env::var("DITCH_OLLAMA_TEST_MODEL").unwrap();
    let status = ollama::verify_model(&provider.endpoint, None, &provider.model).unwrap();
    provider.context_window = status.context_window;
    provider
        .endpoint
        .model_status
        .insert(provider.model.clone(), status);
    let binary = std::env::var("DITCH_OLLAMA_TEST_CODEX").unwrap_or_else(|_| "codex".into());
    let home = root.join("live-home");
    fs::create_dir_all(&home).unwrap();
    let mut client = session_namer::open_with_provider(
        &binary,
        &root.join("original"),
        Some(&home),
        Some(&provider),
    )
    .unwrap();
    client.graceful = true;
    let started=client.call("thread/start",json!({"cwd":root.join("original"),"model":provider.model,
        "modelProvider":provider.endpoint.provider_key(),"sandbox":"read-only","approvalPolicy":"never",
        "config":{"web_search":"disabled","features":{"apps":false}},
        "baseInstructions":"Reply only READY. Do not use tools."})).unwrap();
    let thread = started["thread"]["id"].as_str().unwrap().to_owned();
    client.call("turn/start",json!({"threadId":thread,"effort":"none","input":[{"type":"text","text":"Fix login validation so empty passwords are rejected."}]})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let line = client
            .incoming
            .as_ref()
            .unwrap()
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
            .unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["method"] == "turn/completed" {
            assert_eq!(value["params"]["turn"]["status"], "completed");
            break;
        }
    }
    let before = client
        .call(
            "thread/read",
            json!({"threadId":thread,"includeTurns":true}),
        )
        .unwrap()["thread"]["turns"]
        .clone();
    drop(client);
    {
        let mut locked = state.lock().unwrap();
        locked.codex_binary = Some(binary.clone());
        locked
            .store
            .insert_ollama_endpoint(&provider.endpoint)
            .unwrap();
        let run = &mut locked.agents.get_mut(&id).unwrap().run;
        run.execution_profile.model = Some(provider.model.clone());
        run.origin_codex_home = Some(home.to_string_lossy().into_owned());
        run.native_session_id = Some(thread.clone());
    }
    let mut job = claim_session_name_job(&state).unwrap();
    job.prompt = "Fix login validation so empty passwords are rejected.".into();
    generate_session_name(&state, &mut job).unwrap();
    let title = state.lock().unwrap().agents[&id]
        .run
        .codex_title
        .clone()
        .unwrap();
    assert!(!title.is_empty() && title.encode_utf16().count() <= 240);
    let mut client = session_namer::open_with_provider(
        &binary,
        &root.join("original"),
        Some(&home),
        Some(&provider),
    )
    .unwrap();
    client.graceful = true;
    let after = client
        .call(
            "thread/read",
            json!({"threadId":thread,"includeTurns":true}),
        )
        .unwrap();
    assert_eq!(
        after["thread"]["turns"], before,
        "Naming changed the main conversation"
    );
    assert_eq!(after["thread"]["name"], title);
    let listed = client
        .call(
            "thread/list",
            json!({"limit":100,"modelProviders":[provider.endpoint.provider_key()]}),
        )
        .unwrap();
    assert_eq!(
        listed["data"].as_array().unwrap().len(),
        1,
        "Naming created a stored thread"
    );
    assert_eq!(state.lock().unwrap().agents.len(), 1);
    println!("Selected OSS model generated: {title}; main conversation unchanged");
    drop(client);
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_naming_failure_is_bounded_and_can_be_retried_from_rename() {
    let (state, id, root) = naming_fixture("fail");
    process_session_name_job(&state);
    process_session_name_job(&state); // retry backoff
    assert_eq!(naming_calls(&root, "turn/start"), 1);
    allow_naming_retry(&state, id);
    process_session_name_job(&state);
    process_session_name_job(&state);
    assert_eq!(naming_calls(&root, "turn/start"), 2);
    {
        let locked = state.lock().unwrap();
        assert_eq!(locked.agents[&id].run.display_name(), "Unnamed session");
        assert!(locked.store.load().unwrap().agents[0].run.naming_failed);
        assert_eq!(locked.agents[&id].run.state, AgentState::Working);
        let restarted = Arc::new(Mutex::new(
            RuntimeState::new(locked.paths.clone(), false).unwrap(),
        ));
        assert!(claim_session_name_job(&restarted).is_none());
        assert!(restarted.lock().unwrap().agents[&id].run.naming_failed);
    }
    fs::write(root.join("mode"), "success").unwrap();
    assert!(matches!(
        rename_agent(state.clone(), id, None),
        ServerResponse::Accepted
    ));
    process_session_name_job(&state);
    assert_eq!(
        state.lock().unwrap().agents[&id].run.display_name(),
        "Fix login validation"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_naming_reuses_generated_result_when_saving_needs_a_retry() {
    let (state, id, root) = naming_fixture("save-fails");
    process_session_name_job(&state);
    assert!(state.lock().unwrap().agents[&id].run.codex_title.is_none());
    fs::write(root.join("mode"), "success").unwrap();
    allow_naming_retry(&state, id);
    process_session_name_job(&state);
    assert_eq!(naming_calls(&root, "turn/start"), 1);
    assert_eq!(naming_calls(&root, "thread/name/set"), 2);
    assert!(state.lock().unwrap().agents[&id].run.codex_title.is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_naming_preserves_manual_edit_during_generation() {
    let (state, id, root) = naming_fixture("delayed");
    let worker_state = state.clone();
    let worker = thread::spawn(move || process_session_name_job(&worker_state));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("generating").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    rename_agent(state.clone(), id, Some("My manual name".into()));
    fs::write(root.join("continue"), "").unwrap();
    worker.join().unwrap();
    assert_eq!(
        state.lock().unwrap().agents[&id].run.display_name(),
        "My manual name"
    );
    assert_eq!(naming_calls(&root, "thread/name/set"), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_naming_uses_existing_codex_name_without_a_model_request() {
    let (state, id, root) = naming_fixture("existing");
    process_session_name_job(&state);
    assert_eq!(
        state.lock().unwrap().agents[&id].run.display_name(),
        "Existing Codex name"
    );
    assert_eq!(naming_calls(&root, "turn/start"), 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_naming_rejects_tools_malformed_output_and_non_ephemeral_servers() {
    for mode in ["tool", "malformed", "unsupported"] {
        let (state, id, root) = naming_fixture(mode);
        process_session_name_job(&state);
        assert!(state.lock().unwrap().agents[&id].run.codex_title.is_none());
        assert_eq!(naming_calls(&root, "thread/name/set"), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
