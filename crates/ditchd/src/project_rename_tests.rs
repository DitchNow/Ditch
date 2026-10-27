// Shared by both daemon editions.
fn rename_fixture() -> (Arc<Mutex<RuntimeState>>, Project) {
    let state = Arc::new(Mutex::new(test_runtime()));
    let root = state.lock().unwrap().paths.data_dir.join("project");
    fs::create_dir_all(&root).unwrap();
    let mut project = Project::new("Original", root);
    project.git_policy = ProjectGitPolicy::AllowOutsideGit;
    register_local_project(&state, project.clone()).unwrap();
    (state, project)
}

fn rename_request(
    state: &Arc<Mutex<RuntimeState>>,
    project: &Project,
    name: &str,
) -> ServerResponse {
    handle_request(
        ClientRequest::RenameProject {
            project_id: project.id,
            name: name.into(),
        },
        state.clone(),
    )
}

fn reject_project_updates(state: &Arc<Mutex<RuntimeState>>) -> rusqlite::Connection {
    let db = rusqlite::Connection::open(&state.lock().unwrap().paths.database_path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_project_rename BEFORE UPDATE ON projects BEGIN SELECT RAISE(FAIL, 'injected write failure'); END;").unwrap();
    db
}

#[test]
fn project_rename_persists_only_the_name_and_survives_restart() {
    let (state, original) = rename_fixture();
    let (tx, rx) = mpsc::sync_channel(20);
    state.lock().unwrap().subscribers.push(tx);
    let mut expected = original.clone();
    expected.name = "Renamed project".into();
    assert_eq!(
        rename_request(&state, &original, "  Renamed project  "),
        ServerResponse::ProjectRenamed(expected.clone())
    );
    assert!(rx.try_recv().unwrap().contains("ProjectChanged"));
    assert_eq!(load_project_config(&original.root).unwrap(), expected);
    assert!(!original.root.join(".git").exists());
    let paths = state.lock().unwrap().paths.clone();
    drop(state);
    let restarted = RuntimeState::new(paths, false).unwrap();
    assert_eq!(restarted.projects[&original.root_key()], expected);
}

#[test]
fn project_rename_rejects_invalid_input_and_skips_unchanged_names() {
    let (state, project) = rename_fixture();
    let (tx, rx) = mpsc::sync_channel(20);
    state.lock().unwrap().subscribers.push(tx);
    assert!(
        matches!(rename_request(&state, &project, " \n\t "), ServerResponse::Error(ProtocolError { code, .. }) if code == "invalid_project_name")
    );
    let mut unknown = project.clone();
    unknown.id = ProjectId::new();
    assert!(
        matches!(rename_request(&state, &unknown, "Name"), ServerResponse::Error(ProtocolError { code, .. }) if code == "project_not_found")
    );
    let _db = reject_project_updates(&state);
    assert_eq!(
        rename_request(&state, &project, " Original "),
        ServerResponse::ProjectRenamed(project.clone())
    );
    assert!(rx.try_recv().is_err());
    assert_eq!(load_project_config(&project.root).unwrap(), project);
}

#[test]
fn project_rename_database_failure_restores_metadata_and_emits_no_change() {
    let (state, project) = rename_fixture();
    let (tx, rx) = mpsc::sync_channel(20);
    state.lock().unwrap().subscribers.push(tx);
    let _db = reject_project_updates(&state);
    assert!(
        matches!(rename_request(&state, &project, "Failed"), ServerResponse::Error(ProtocolError { code, .. }) if code == "project_rename_failed")
    );
    assert_eq!(load_project_config(&project.root).unwrap(), project);
    assert_eq!(
        state.lock().unwrap().store.load().unwrap().projects,
        vec![project.clone()]
    );
    assert_eq!(project_by_id(&state, project.id).unwrap(), project);
    assert!(rx.try_recv().is_err());
}

#[test]
fn project_rename_metadata_failure_leaves_database_and_runtime_unchanged() {
    let (state, project) = rename_fixture();
    fs::remove_file(project.ditch_dir().join("project.json")).unwrap();
    fs::create_dir(project.ditch_dir().join("project.json")).unwrap();
    assert!(matches!(
        rename_request(&state, &project, "Failed"),
        ServerResponse::Error(_)
    ));
    assert_eq!(
        state.lock().unwrap().store.load().unwrap().projects,
        vec![project.clone()]
    );
    assert_eq!(project_by_id(&state, project.id).unwrap(), project);
}

#[test]
fn project_rename_does_not_interrupt_active_sessions() {
    let mut runtime = test_runtime();
    let agent_id = recovery_agent(&mut runtime);
    let run = runtime.agents[&agent_id].run.clone();
    let project = runtime
        .projects
        .values()
        .find(|p| p.id == run.project_id)
        .unwrap()
        .clone();
    ensure_project_metadata(&project).unwrap();
    save_project_config(&project).unwrap();
    let state = Arc::new(Mutex::new(runtime));
    assert!(matches!(
        rename_request(&state, &project, "While running"),
        ServerResponse::ProjectRenamed(_)
    ));
    assert_eq!(state.lock().unwrap().agents[&agent_id].run, run);
}

fn remote_rename_fixture() -> (Arc<Mutex<RuntimeState>>, Arc<Mutex<RuntimeState>>, Project) {
    let (owner, local) = rename_fixture();
    let client = Arc::new(Mutex::new(test_runtime()));
    let remote = ssh_remote::wrap_remote_project(local, "rename-host", Uuid::new_v4());
    let authority = owner.clone();
    {
        let mut runtime = client.lock().unwrap();
        runtime.store.upsert_project(&remote).unwrap();
        runtime.projects.insert(remote.root_key(), remote.clone());
        runtime.remote_connections.test_rpc = Some(Arc::new(move |alias, _, request| {
            assert_eq!(alias, "rename-host");
            Ok(handle_request(request, authority.clone()))
        }));
    }
    (client, owner, remote)
}

#[test]
fn project_rename_remote_forwarding_preserves_identity_and_owner_persistence() {
    let (client, owner, remote) = remote_rename_fixture();
    let mut expected = remote.clone();
    expected.name = "Remote name".into();
    assert_eq!(
        rename_request(&client, &remote, "Remote name"),
        ServerResponse::ProjectRenamed(expected.clone())
    );
    assert_eq!(project_by_id(&client, remote.id).unwrap(), expected);
    let local = project_by_id(&owner, remote.id).unwrap();
    assert!(!local.is_remote());
    assert_eq!(load_project_config(&local.root).unwrap(), local);
    assert_eq!(local.name, "Remote name");
    let paths = client.lock().unwrap().paths.clone();
    drop(client);
    assert_eq!(
        RuntimeState::new(paths, false).unwrap().projects[&expected.root_key()],
        expected
    );
}

#[test]
fn project_rename_remote_offline_and_unsupported_do_not_mutate() {
    let (client, owner, remote) = remote_rename_fixture();
    let mut status = owner.lock().unwrap().runtime_status();
    status.capabilities.retain(|c| c != "project_rename_v1");
    client.lock().unwrap().remote_connections.test_rpc = Some(Arc::new(move |_, _, request| {
        assert!(matches!(request, ClientRequest::RuntimeStatus));
        Ok(ServerResponse::RuntimeStatus(status.clone()))
    }));
    assert!(
        matches!(rename_request(&client, &remote, "Unsupported"), ServerResponse::Error(ProtocolError { code, message }) if code == "remote_rename_unsupported" && message.contains("Remote Runtime Setup"))
    );
    client.lock().unwrap().remote_connections.test_rpc = Some(Arc::new(|_, _, _| {
        Err(ditch_ssh::SshError::Failed("Offline".into()))
    }));
    assert!(
        matches!(rename_request(&client, &remote, "Offline"), ServerResponse::Error(ProtocolError { code, .. }) if code == "remote_unavailable")
    );
    assert_eq!(project_by_id(&client, remote.id).unwrap(), remote);
    assert_eq!(project_by_id(&owner, remote.id).unwrap().name, remote.name);
}

#[test]
fn project_rename_remote_events_are_host_isolated_and_reconnect_recovers_names() {
    let (client, owner, remote) = remote_rename_fixture();
    let mut changed = project_by_id(&owner, remote.id).unwrap();
    changed.name = "Event name".into();
    forward_remote_event(
        &client,
        "other-host",
        ServerEvent::ProjectChanged(changed.clone()),
    );
    assert_eq!(project_by_id(&client, remote.id).unwrap(), remote);
    let mut wrong_root = changed.clone();
    wrong_root.root = "/wrong/root".into();
    forward_remote_event(
        &client,
        "rename-host",
        ServerEvent::ProjectChanged(wrong_root),
    );
    assert_eq!(project_by_id(&client, remote.id).unwrap(), remote);
    forward_remote_event(&client, "rename-host", ServerEvent::ProjectChanged(changed));
    assert_eq!(
        project_by_id(&client, remote.id).unwrap().name,
        "Event name"
    );
    rename_request(&owner, &remote, "Missed while offline");
    let mut snapshot = owner.lock().unwrap().snapshot();
    snapshot
        .projects
        .push(Project::new("Unrelated", "/unrelated"));
    let epoch = snapshot.event_epoch;
    reconcile_remote_snapshot(
        &client,
        "rename-host",
        &[remote.clone()],
        epoch,
        snapshot,
        0,
    );
    let mut expected = remote.clone();
    expected.name = "Missed while offline".into();
    assert_eq!(project_by_id(&client, remote.id).unwrap(), expected);
    assert_eq!(client.lock().unwrap().projects.len(), 1);
    assert_eq!(
        client.lock().unwrap().store.load().unwrap().projects,
        vec![expected]
    );
}

#[test]
fn project_rename_remote_success_cache_failure_is_explicit_and_recoverable() {
    let (client, owner, remote) = remote_rename_fixture();
    let db = reject_project_updates(&client);
    assert!(
        matches!(rename_request(&client, &remote, "Saved remotely"), ServerResponse::Error(ProtocolError { code, .. }) if code == "remote_project_cache_failed")
    );
    assert_eq!(project_by_id(&client, remote.id).unwrap(), remote);
    assert_eq!(
        project_by_id(&owner, remote.id).unwrap().name,
        "Saved remotely"
    );
    db.execute_batch("DROP TRIGGER reject_project_rename")
        .unwrap();
    assert!(matches!(
        rename_request(&client, &remote, "Saved remotely"),
        ServerResponse::ProjectRenamed(_)
    ));
    assert_eq!(
        project_by_id(&client, remote.id).unwrap().name,
        "Saved remotely"
    );
}

#[test]
fn project_rename_remote_response_cannot_change_identity_or_resurrect_removed_projects() {
    let (client, owner, remote) = remote_rename_fixture();
    let mut incoming = project_by_id(&owner, remote.id).unwrap();
    incoming.id = ProjectId::new();
    assert!(
        cache_remote_project_name(&mut client.lock().unwrap(), "rename-host", &incoming).is_err()
    );
    incoming.id = remote.id;
    client.lock().unwrap().projects.clear();
    assert!(
        cache_remote_project_name(&mut client.lock().unwrap(), "rename-host", &incoming).is_err()
    );
    assert!(client.lock().unwrap().projects.is_empty());
}

#[test]
fn project_rename_response_for_another_registered_project_is_rejected() {
    let (client, owner, remote) = remote_rename_fixture();
    let other = Project::new("Other", "/other");
    let cached = ssh_remote::wrap_remote_project(other.clone(), "rename-host", Uuid::new_v4());
    let status = owner.lock().unwrap().runtime_status();
    {
        let mut runtime = client.lock().unwrap();
        runtime.store.upsert_project(&cached).unwrap();
        runtime.projects.insert(cached.root_key(), cached.clone());
        runtime.remote_connections.test_rpc = Some(Arc::new(move |_, _, request| {
            Ok(match request {
                ClientRequest::RuntimeStatus => ServerResponse::RuntimeStatus(status.clone()),
                ClientRequest::RenameProject { .. } => {
                    let mut changed = other.clone();
                    changed.name = "Wrong project".into();
                    ServerResponse::ProjectRenamed(changed)
                }
                _ => panic!("Unexpected request"),
            })
        }));
    }
    assert!(
        matches!(rename_request(&client, &remote, "Requested"), ServerResponse::Error(ProtocolError { code, .. }) if code == "remote_project_identity_mismatch")
    );
    assert_eq!(project_by_id(&client, remote.id).unwrap(), remote);
    assert_eq!(project_by_id(&client, cached.id).unwrap(), cached);
}
