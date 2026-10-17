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
        result = {'thread': {'id': 'internal-name-thread', 'ephemeral': mode != 'unsupported'}, 'model': 'fixture'}
    elif method == 'turn/start':
        assert p['threadId'] == 'internal-name-thread'
        assert p['effort'] == 'low' and p['outputSchema']['required'] == ['title']
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
