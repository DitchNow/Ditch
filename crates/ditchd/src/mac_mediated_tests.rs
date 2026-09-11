use super::*;
use crate::{
    ProjectRootKey,
    tests::{recovery_agent, test_runtime},
};
use ditch_core::{AgentId, PermissionActionKind, PermissionRequest};
use ditch_protocol::ClientRequest;

type Calls = Arc<Mutex<Vec<(String, Uuid, ClientRequest)>>>;
fn fixture() -> (
    Arc<Mutex<RuntimeState>>,
    RemoteCommand,
    AgentId,
    Uuid,
    Calls,
) {
    let mut runtime = test_runtime();
    let agent = recovery_agent(&mut runtime);
    let project_id = runtime.agents[&agent].run.project_id;
    let local = runtime
        .projects
        .values()
        .find(|p| p.id == project_id)
        .unwrap()
        .clone();
    runtime.projects.remove(&local.root_key());
    let remote = crate::ssh_remote::wrap_remote_project(local, "fixture-host", Uuid::new_v4());
    runtime.store.upsert_project(&remote).unwrap();
    runtime.projects.insert(remote.root_key(), remote);
    runtime
        .remote_agent_hosts
        .insert(agent, "fixture-host".into());
    runtime
        .remote_event_subscriptions
        .insert("fixture-host".into());
    runtime
        .remote_connection_states
        .insert("fixture-host".into(), "online".into());
    let request_id = Uuid::new_v4();
    runtime.pending_permissions.insert(
        request_id,
        PermissionRequest {
            questions: vec![],
            id: request_id,
            project_id,
            agent_id: Some(agent),
            action: PermissionActionKind::RunCommand,
            summary: "Review command".into(),
            target: "fixture".into(),
            command: Some("pwd".into()),
            created_at: Utc::now(),
            expires_at: None,
        },
    );
    runtime
        .remote_permission_hosts
        .insert(request_id, "fixture-host".into());
    let owner = Uuid::new_v4();
    let machine = Uuid::new_v4();
    let device = Uuid::new_v4();
    let identity = MachineIdentity::generate(machine);
    runtime
        .store
        .upsert_remote_machine(&RemoteMachineRecord {
            machine_id: machine,
            owner_id: Some(owner),
            name: "Mac".into(),
            signing_public_key: identity.signing_public_key(),
            agreement_public_key: identity.agreement_public_key(),
            key_version: 1,
            enabled: true,
            projection_epoch: Uuid::new_v4(),
            projection_sequence: 0,
        })
        .unwrap();
    runtime
        .store
        .upsert_remote_device(&RemoteDeviceRecord {
            device_id: device,
            name: "Phone".into(),
            signing_public_key: identity.signing_public_key(),
            agreement_public_key: identity.agreement_public_key(),
            key_version: 1,
            state: "active".into(),
            last_seen_at: None,
        })
        .unwrap();
    let calls: Calls = Arc::new(Mutex::new(vec![]));
    let capture = calls.clone();
    runtime.remote_connections.test_rpc = Some(Arc::new(move |alias, id, request| {
        capture
            .lock()
            .unwrap()
            .push((alias.to_owned(), id, request));
        Ok(ServerResponse::Accepted)
    }));
    let command = RemoteCommand {
        protocol_version: 1,
        command_id: Uuid::new_v4(),
        idempotency_key: Uuid::new_v4().to_string(),
        owner_id: owner,
        machine_id: machine,
        device_id: device,
        command_type: RemoteCommandType::SessionPrompt,
        created_at: Utc::now().timestamp_millis(),
        expires_at: (Utc::now() + chrono::Duration::minutes(2)).timestamp_millis(),
        confirmation_class: ConfirmationClass::None,
        payload: ditch_remote::EncryptedPayload {
            key_version: 1,
            nonce: String::new(),
            ciphertext: String::new(),
        },
    };
    (
        Arc::new(Mutex::new(runtime)),
        command,
        agent,
        request_id,
        calls,
    )
}

#[test]
fn mobile_actions_dispatch_through_mac_to_owning_ssh_host() {
    for kind in [
        RemoteCommandType::SessionStart,
        RemoteCommandType::SessionPrompt,
        RemoteCommandType::SessionStop,
        RemoteCommandType::SessionForceKill,
        RemoteCommandType::QuerySessionTranscript,
        RemoteCommandType::QueryApproval,
        RemoteCommandType::ApprovalRespond,
        RemoteCommandType::AttentionExecute,
    ] {
        let (state, mut command, agent, approval, calls) = fixture();
        command.command_type = kind;
        command.confirmation_class = kind.required_confirmation();
        let project = state.lock().unwrap().agents[&agent].run.project_id;
        let body = json!({"project_id": project, "session_id": agent, "text":"continue", "initial_prompt":"start", "task_title":"Task", "limit":200,"query_id":Uuid::new_v4(),
            "attention_id":approval,"action_id":approval,"decision":"approve","answers":{"q":["yes"]}});
        let result = execute_command(
            state.clone(),
            &command,
            &serde_json::to_vec(&body).unwrap(),
            true,
        );
        assert_eq!(result.status, "completed", "{kind:?}: {result:?}");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "{kind:?}");
        assert_eq!(calls[0].0, "fixture-host");
        if !matches!(
            kind,
            RemoteCommandType::QueryApproval | RemoteCommandType::QuerySessionTranscript
        ) {
            assert_eq!(calls[0].1, command.command_id);
        }
        assert!(state.lock().unwrap().children.is_empty());
    }
}

#[test]
fn duplicate_mobile_command_recovers_without_another_prompt_and_rejects_changed_body() {
    let (state, command, agent, _, calls) = fixture();
    let body = serde_json::to_vec(&json!({"session_id":agent,"text":"continue"})).unwrap();
    assert_eq!(
        execute_command(state.clone(), &command, &body, true).status,
        "completed"
    );
    assert_eq!(
        execute_command(state.clone(), &command, &body, true).status,
        "completed"
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
    let changed = serde_json::to_vec(&json!({"session_id":agent,"text":"different"})).unwrap();
    assert_eq!(
        execute_command(state, &command, &changed, true).error_code,
        Some("operation_conflict")
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn ssh_runtime_cannot_start_mobile_connection_or_accept_mobile_commands() {
    let (state, command, _, _, calls) = fixture();
    state.lock().unwrap().remote_runtime = true;
    crate::edition::start(state.clone());
    assert!(!state.lock().unwrap().edition.remote.connection_started);
    assert!(matches!(
        crate::edition::handle_request(ClientRequest::CreateRemotePairing, state.clone()),
        ServerResponse::Error(_)
    ));
    assert_eq!(
        execute_command(state, &command, b"{}", true).error_code,
        Some("unauthorized")
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn mac_projection_includes_ssh_work_and_separates_target_availability() {
    let (state, _, agent, _, _) = fixture();
    let mut guard = state.lock().unwrap();
    guard
        .remote_connection_states
        .insert("fixture-host".into(), "reconnecting".into());
    let snapshot = ProjectionSnapshot::from_state(&guard);
    let project = guard.agents[&agent].run.project_id;
    assert_eq!(
        snapshot.records[&("project".into(), project.0.to_string())]["target_status"],
        "reconnecting"
    );
    assert!(
        snapshot
            .records
            .contains_key(&("session".into(), agent.0.to_string()))
    );
    assert_eq!(
        guard.agents[&agent].run.state,
        ditch_core::AgentState::Working
    );
}

#[test]
fn lost_ack_then_mac_restart_recovers_saved_remote_receipt_without_resending() {
    let (state, command, agent, _, calls) = fixture();
    let body = serde_json::to_vec(&json!({"session_id":agent,"text":"continue"})).unwrap();
    let capture = calls.clone();
    state.lock().unwrap().remote_connections.test_rpc =
        Some(Arc::new(move |alias, id, request| {
            capture.lock().unwrap().push((alias.into(), id, request));
            Ok(protocol_error(
                "operation_outcome_unknown",
                "lost acknowledgement",
            ))
        }));
    assert_eq!(
        execute_command(state.clone(), &command, &body, true).error_code,
        Some("operation_outcome_unknown")
    );
    let paths = state.lock().unwrap().paths.clone();
    drop(state);
    let mut restored = RuntimeState::new(paths, false).unwrap();
    let original_id = command.command_id;
    let capture = calls.clone();
    restored.remote_connections.test_rpc = Some(Arc::new(move |alias, id, request| {
        assert!(
            matches!(request, ClientRequest::GetOperationOutcome { request_id } if request_id == original_id)
        );
        capture.lock().unwrap().push((alias.into(), id, request));
        Ok(ServerResponse::OperationOutcome {
            request_id: original_id,
            state: "completed".into(),
            response: Some(serde_json::to_value(ServerResponse::Accepted).unwrap()),
        })
    }));
    assert_eq!(
        execute_command(Arc::new(Mutex::new(restored)), &command, &body, true).status,
        "completed"
    );
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[test]
fn stale_approval_never_routes_to_a_new_turn() {
    let (state, mut command, _, request_id, calls) = fixture();
    state
        .lock()
        .unwrap()
        .pending_permissions
        .remove(&request_id);
    command.command_type = RemoteCommandType::ApprovalRespond;
    command.confirmation_class = command.command_type.required_confirmation();
    let body = serde_json::to_vec(
        &json!({"attention_id":request_id,"action_id":request_id,"decision":"approve"}),
    )
    .unwrap();
    assert_eq!(
        execute_command(state, &command, &body, true).error_code,
        Some("approval_stale")
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn relay_reconnect_redelivers_saved_result_without_redispatching_work() {
    let (state, command, agent, _, calls) = fixture();
    let body = serde_json::to_vec(&json!({"session_id":agent,"text":"continue"})).unwrap();
    assert_eq!(
        execute_command(state.clone(), &command, &body, true).status,
        "completed"
    );
    let (sender, receiver) = mpsc::sync_channel(8);
    schedule_receipt_replay(
        &state,
        &MachineIdentity::generate(command.machine_id),
        sender,
    );
    let frame: Value = serde_json::from_str(
        &receiver
            .recv_timeout(StdDuration::from_secs(3))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(frame["type"], "command_result");
    assert_eq!(
        frame["payload"]["command_id"],
        command.command_id.to_string()
    );
    assert!(frame["payload"]["encrypted"]["ciphertext"].is_string());
    assert_eq!(calls.lock().unwrap().len(), 1);
}
