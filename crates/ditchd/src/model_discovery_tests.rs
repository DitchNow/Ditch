// Included in both editions' runtime tests.
#[test]
fn model_discovery_uses_project_and_codex_home_on_both_targets() {
    for remote in [false, true] {
        let mut runtime = test_runtime();
        runtime.remote_runtime = remote;
        let root = runtime.paths.data_dir.join("model-project");
        let home = runtime.paths.data_dir.join("model-home");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let home = fs::canonicalize(home).unwrap();
        let binary = root.join("fake-codex");
        fs::write(&binary, r#"#!/usr/bin/python3
import json, os, pathlib, sys
if '--version' in sys.argv:
    print('codex-cli 0.154.0'); sys.exit(0)
assert sys.argv[1:] == ['app-server']
assert pathlib.Path.cwd().name == 'model-project'
assert pathlib.Path(os.environ['CODEX_HOME']).name == 'model-home'
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'initialized': continue
    if method == 'initialize':
        result = {}
    else:
        assert method == 'model/list'  # Discovery must never start a turn.
        cursor = request['params']['cursor']
        result = {'data': [{'id': 'project-model' if cursor is None else 'second-model',
                            'displayName': 'Project model', 'isDefault': cursor is None}],
                  'nextCursor': 'page-two' if cursor is None else None}
    print(json.dumps({'id': request['id'], 'result': result}), flush=True)
"#).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        runtime.codex_binary = Some(binary.to_string_lossy().into_owned());
        runtime.codex_home = Some(home);
        let project = Project::new("Model fixture", root);
        runtime.projects.insert(project.root_key(), project.clone());
        let state = Arc::new(Mutex::new(runtime));
        let response = handle_request(ClientRequest::ListAgentModels {
            provider: AgentProvider::Codex, project_id: Some(project.id),
        }, state.clone());
        let ServerResponse::AgentModels(models) = response else { panic!("{response:?}"); };
        assert_eq!(models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["project-model", "second-model"]);
        assert!(models[0].is_default);
        let missing = handle_request(ClientRequest::ListAgentModels {
            provider: AgentProvider::Codex, project_id: Some(ProjectId::new()),
        }, state);
        assert!(matches!(missing, ServerResponse::Error(ProtocolError { code, .. }) if code == "project_not_found"));
    }
}
