// Admission and idle checks share RuntimeState's lock with writer reservations.
// The maintenance exit deliberately does not use shutdown_runtime: that path
// cancels work and kills agent process groups.
impl RuntimeState {
    fn remote_maintenance_active(&self) -> bool {
        self.remote_maintenance.is_some_and(|(_, until)| {
            until > Instant::now() || remote_lifecycle::upgrade_lock_held(&self.paths)
        })
    }

    fn remote_upgrade_busy(&mut self) -> bool {
        self.reap_orphan_writers();
        self.runtime_status().active_session_count != 0
            || self.remote_mutations != 0
            || !self.children.is_empty()
            || !self.app_server_turns.is_empty()
            || !self.acceptance_controls.is_empty()
            || !self.writer_leases.is_empty()
            || !self.orphan_writer_groups.is_empty()
            || !self.remote_launching.is_empty()
            || !self.terminals.is_empty()
            || !self.setup_terminals.is_empty()
    }
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;

    #[test]
    fn maintenance_defers_busy_runtime_and_closes_mutation_admission_atomically() {
        let mut runtime = tests::test_runtime();
        runtime.remote_runtime = true;
        let instance = runtime.instance_id;
        let state = Arc::new(Mutex::new(runtime));
        let token = Uuid::new_v4();
        let request = ClientRequest::Shutdown;
        let in_flight = RemoteMutationGuard::acquire(&state, &request).unwrap();
        assert!(matches!(
            remote_maintenance_request(state.clone(), instance, token, "begin"),
            ServerResponse::Error(_)
        ));
        drop(in_flight);
        assert!(matches!(
            remote_maintenance_request(state.clone(), instance, token, "begin"),
            ServerResponse::Accepted
        ));
        assert!(RemoteMutationGuard::acquire(&state, &request).is_err());
        assert!(RemoteMutationGuard::acquire(&state, &ClientRequest::RuntimeStatus).is_ok());
        assert!(matches!(
            remote_maintenance_request(state.clone(), instance, Uuid::new_v4(), "cancel"),
            ServerResponse::Error(_)
        ));
        assert!(matches!(
            remote_maintenance_request(state.clone(), Uuid::new_v4(), token, "cancel"),
            ServerResponse::Error(_)
        ));
        assert!(matches!(
            remote_maintenance_request(state.clone(), instance, token, "cancel"),
            ServerResponse::Accepted
        ));
        assert!(RemoteMutationGuard::acquire(&state, &request).is_ok());
    }

    #[test]
    fn maintenance_blocks_writer_reservations_and_recovers_expired_lease() {
        let mut runtime = tests::test_runtime();
        runtime.remote_runtime = true;
        let project = Project::new("Maintenance", &runtime.paths.data_dir);
        let profile = AgentExecutionProfile::default();
        let token = Uuid::new_v4();
        runtime.remote_maintenance = Some((token, Instant::now() + Duration::from_secs(60)));
        let state = Arc::new(Mutex::new(runtime));
        assert!(WriterGuard::acquire(&state, AgentId::default(), &project, &profile).is_err());
        state.lock().unwrap().remote_maintenance =
            Some((token, Instant::now() - Duration::from_secs(1)));
        let writer = WriterGuard::acquire(&state, AgentId::default(), &project, &profile).unwrap();
        assert!(state.lock().unwrap().remote_upgrade_busy());
        drop(writer);
        assert!(!state.lock().unwrap().remote_upgrade_busy());
    }
}

struct RemoteMutationGuard(Option<Arc<Mutex<RuntimeState>>>);
impl RemoteMutationGuard {
    fn acquire(state: &Arc<Mutex<RuntimeState>>, request: &ClientRequest) -> Result<Self, String> {
        if matches!(
            request,
            ClientRequest::RemoteRuntimeMaintenance { .. }
                | ClientRequest::Health
                | ClientRequest::RuntimeStatus
                | ClientRequest::HostIdentityStatus
                | ClientRequest::Snapshot
                | ClientRequest::ListProjects
                | ClientRequest::ListAgentMessages { .. }
                | ClientRequest::SubscribeEvents { .. }
                | ClientRequest::SubscribeEventsSince { .. }
                | ClientRequest::SubscribeAttention
        ) {
            return Ok(Self(None));
        }
        Self::background_work(state)
    }

    fn background_work(state: &Arc<Mutex<RuntimeState>>) -> Result<Self, String> {
        let mut locked = state.lock().unwrap();
        if !locked.remote_runtime {
            return Ok(Self(None));
        }
        if locked.remote_maintenance_active() {
            return Err(
                "Remote runtime is being upgraded. No new work can start until repair finishes."
                    .into(),
            );
        }
        locked.remote_mutations += 1;
        Ok(Self(Some(Arc::clone(state))))
    }
}
impl Drop for RemoteMutationGuard {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            state.lock().unwrap().remote_mutations -= 1;
        }
    }
}

fn remote_maintenance_request(
    state: Arc<Mutex<RuntimeState>>,
    instance: Uuid,
    token: Uuid,
    action: &str,
) -> ServerResponse {
    let mut locked = state.lock().unwrap();
    if !locked.remote_runtime || instance != locked.instance_id {
        return protocol_error(
            "runtime_identity_changed",
            "The remote daemon changed; retry repair.",
        );
    }
    if action == "begin" {
        if locked.remote_maintenance_active()
            && locked.remote_maintenance.map(|v| v.0) != Some(token)
        {
            return protocol_error("upgrade_in_progress", "Another repair owns this runtime.");
        }
        if locked.remote_upgrade_busy() {
            return protocol_error(
                "runtime_busy",
                "Repair deferred: agents, validation, terminals, or pending operations are still active. Finish that work before retrying.",
            );
        }
        locked.remote_maintenance = Some((token, Instant::now() + Duration::from_secs(180)));
        return ServerResponse::Accepted;
    }
    if !locked.remote_maintenance_active() || locked.remote_maintenance.map(|v| v.0) != Some(token)
    {
        return protocol_error(
            "maintenance_expired",
            "Maintenance ownership expired; retry repair.",
        );
    }
    match action {
        "cancel" => {
            locked.remote_maintenance = None;
            ServerResponse::Accepted
        }
        "exit" => {
            if locked.remote_upgrade_busy() {
                return protocol_error(
                    "runtime_busy",
                    "Runtime is no longer idle; nothing was stopped.",
                );
            }
            // Keep admission closed through process exit. Kernel cleanup releases
            // the daemon lock; the replacement owns socket cleanup on startup.
            locked.remote_maintenance = Some((token, Instant::now() + Duration::from_secs(3600)));
            thread::spawn(|| {
                thread::sleep(Duration::from_millis(150));
                std::process::exit(0);
            });
            ServerResponse::Accepted
        }
        _ => protocol_error("invalid_maintenance_action", "Unknown maintenance action."),
    }
}
