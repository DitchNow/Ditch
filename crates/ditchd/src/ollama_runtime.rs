fn ollama_request(
    state: Arc<Mutex<RuntimeState>>,
    request: ditch_core::ollama::OllamaRequest,
) -> ServerResponse {
    use ditch_core::ollama::*;
    let Some(project) = project_by_id(&state, request.project_id) else {
        return protocol_error("project_not_found", "Project not registered");
    };
    if project.is_remote() {
        match forward_project_command(&state, &project, ClientRequest::RuntimeStatus) {
            ServerResponse::RuntimeStatus(status)
                if status.capabilities.iter().any(|c| c == ollama::CAPABILITY) =>
            {
                ()
            }
            ServerResponse::Error(error) => return ServerResponse::Error(error),
            _ => {
                return protocol_error(
                    "ollama_upgrade_required",
                    "Upgrade the remote runtime before managing OSS endpoints",
                );
            }
        }
        return forward_project_command(&state, &project, ClientRequest::Ollama(request));
    }
    let result = (|| -> Result<OllamaResponse, String> {
        let (paths, remote) = {
            let s = state.lock().unwrap();
            (s.paths.clone(), s.remote_runtime)
        };
        match request.operation {
            OllamaOperation::List => Ok(OllamaResponse::Endpoints(
                state
                    .lock()
                    .unwrap()
                    .store
                    .ollama_endpoints()
                    .map_err(|e| e.to_string())?,
            )),
            OllamaOperation::Test {
                api_url,
                auth,
                allow_private_http,
                model,
            } => ollama::probe(
                &api_url,
                allow_private_http,
                auth.as_ref(),
                model.as_deref(),
            )
            .map(OllamaResponse::Probe)
            .map_err(|e| e.to_string()),
            OllamaOperation::Add {
                display_name,
                api_url,
                auth,
                allow_private_http,
                model,
            } => {
                let display_name = display_name.trim();
                if display_name.is_empty()
                    || display_name.len() > 120
                    || display_name.chars().any(char::is_control)
                {
                    return Err("Enter a server name (1–120 characters)".into());
                }
                if !ollama::valid_model(&model) {
                    return Err("Enter an exact model ID".into());
                }
                ollama::validate_token(auth.as_ref()).map_err(|e| e.to_string())?;
                let url = ollama::normalize_url(&api_url, allow_private_http)
                    .map_err(|e| e.to_string())?;
                // Validate even when discovery is hidden. The manual model option is intentional.
                ollama::validate_destination(&url).map_err(|e| e.to_string())?;
                let probe = ollama::probe(&api_url, allow_private_http, auth.as_ref(), None)
                    .map_err(|e| e.to_string())?;
                let mut models = probe.models;
                if !models.contains(&model) {
                    models.push(model);
                }
                let endpoint = OllamaEndpoint {
                    id: Uuid::new_v4(),
                    display_name: display_name.into(),
                    api_root_url: url.as_str().trim_end_matches('/').into(),
                    auth_mode: if auth.is_some() { "bearer" } else { "none" }.into(),
                    has_credentials: auth.is_some(),
                    allow_private_http,
                    enabled: true,
                    created_at: Utc::now(),
                    models,
                    reachable: Some(probe.reachable),
                    error: probe.error,
                };
                if let Some(token) = &auth {
                    ollama::save_token(&paths, remote, endpoint.id, token)
                        .map_err(|_| "Could not store endpoint credential securely".to_owned())?;
                }
                if let Err(e) = state
                    .lock()
                    .unwrap()
                    .store
                    .insert_ollama_endpoint(&endpoint)
                {
                    ollama::remove_token(&paths, remote, endpoint.id);
                    return Err(e.to_string());
                }
                Ok(OllamaResponse::Endpoint(endpoint))
            }
            OllamaOperation::Refresh { endpoint_id } => {
                let endpoint = state
                    .lock()
                    .unwrap()
                    .store
                    .ollama_endpoint(endpoint_id)
                    .map_err(|e| e.to_string())?
                    .ok_or("Endpoint not found")?;
                if !endpoint.enabled {
                    return Err("Endpoint is disabled".into());
                }
                let result = ollama::load_token(&paths, remote, &endpoint).and_then(|token| {
                    ollama::probe(
                        &endpoint.api_root_url,
                        endpoint.allow_private_http,
                        token.as_ref(),
                        None,
                    )
                });
                let (models, reachable, error) = match result {
                    Ok(p) => (
                        if p.models.is_empty() {
                            None
                        } else {
                            Some(p.models)
                        },
                        Some(p.reachable),
                        p.error,
                    ),
                    Err(e) => (None, Some(false), Some(e.to_string())),
                };
                let endpoint = state
                    .lock()
                    .unwrap()
                    .store
                    .update_ollama_status(endpoint_id, models, reachable, error, false)
                    .map_err(|e| e.to_string())?;
                Ok(OllamaResponse::Endpoint(endpoint))
            }
            OllamaOperation::Disable { endpoint_id } => {
                let endpoint = state
                    .lock()
                    .unwrap()
                    .store
                    .update_ollama_status(endpoint_id, None, None, None, true)
                    .map_err(|e| e.to_string())?;
                Ok(OllamaResponse::Endpoint(endpoint))
            }
        }
    })();
    match result {
        Ok(value) => ServerResponse::Ollama(value),
        Err(message) => protocol_error("ollama_endpoint_error", message),
    }
}

fn resolve_ollama_profile(
    state: &Arc<Mutex<RuntimeState>>,
    profile: &AgentExecutionProfile,
) -> Result<Option<ollama::ResolvedOllamaProvider>, String> {
    let Some(id) = profile.inference_endpoint_id else {
        return Ok(None);
    };
    let model = profile
        .model
        .as_deref()
        .filter(|m| ollama::valid_model(m))
        .ok_or("An exact OSS model ID is required")?;
    let (endpoint, paths, remote) = {
        let s = state.lock().unwrap();
        (
            s.store
                .ollama_endpoint(id)
                .map_err(|e| e.to_string())?
                .ok_or("Inference endpoint not found on this execution host")?,
            s.paths.clone(),
            s.remote_runtime,
        )
    };
    if !endpoint.enabled {
        return Err(
            "Inference endpoint is disabled. Start a new agent to use another server.".into(),
        );
    }
    let url = ollama::normalize_url(&endpoint.api_root_url, endpoint.allow_private_http)
        .map_err(|e| e.to_string())?;
    ollama::validate_destination(&url).map_err(|e| e.to_string())?;
    let token = ollama::load_token(&paths, remote, &endpoint)
        .map_err(|_| "Inference endpoint credential is unavailable".to_owned())?;
    Ok(Some(ollama::ResolvedOllamaProvider {
        endpoint,
        token,
        model: model.into(),
    }))
}

fn validate_inference_binding(saved: Option<Uuid>, incoming: Option<Uuid>) -> Result<(), String> {
    if saved == incoming {
        Ok(())
    } else {
        Err("Start a new agent to change inference server. Existing threads must use their original endpoint.".into())
    }
}

#[cfg(test)]
mod ollama_runtime_tests {
    use super::*;
    use ditch_core::ollama::*;
    #[test]
    fn endpoint_management_routes_to_owner_and_rejects_old_hosts_before_dispatch() {
        let mut remote = tests::test_runtime();
        remote.remote_runtime = true;
        let host_project = Project::new("Remote", &remote.paths.data_dir);
        remote
            .projects
            .insert(host_project.root_key(), host_project.clone());
        let endpoint = OllamaEndpoint {
            id: Uuid::new_v4(),
            display_name: "GPU".into(),
            api_root_url: "https://gpu.example".into(),
            auth_mode: "none".into(),
            has_credentials: false,
            allow_private_http: false,
            enabled: true,
            created_at: Utc::now(),
            models: vec!["qwen:9b".into()],
            reachable: None,
            error: None,
        };
        remote.store.insert_ollama_endpoint(&endpoint).unwrap();
        let remote = Arc::new(Mutex::new(remote));
        let mut local = tests::test_runtime();
        let project = Project::new_remote(
            host_project.id,
            "Remote",
            host_project.root.to_str().unwrap(),
            Uuid::new_v4(),
            "fixture-host",
        );
        local.projects.insert(project.root_key(), project.clone());
        let old = Arc::new(AtomicBool::new(false));
        let old_flag = old.clone();
        let host = remote.clone();
        local.remote_connections.test_rpc = Some(Arc::new(move |alias, _, request| {
            assert_eq!(alias, "fixture-host");
            if matches!(request, ClientRequest::RuntimeStatus) {
                let mut status = host.lock().unwrap().runtime_status();
                if old_flag.load(Ordering::SeqCst) {
                    status.capabilities.retain(|c| c != ollama::CAPABILITY);
                }
                return Ok(ServerResponse::RuntimeStatus(status));
            }
            assert!(
                !old_flag.load(Ordering::SeqCst),
                "unknown endpoint operation reached old host"
            );
            let wire = serde_json::to_vec(&request).unwrap();
            Ok(handle_request(
                serde_json::from_slice(&wire).unwrap(),
                host.clone(),
            ))
        }));
        let local = Arc::new(Mutex::new(local));
        let list = OllamaRequest {
            project_id: project.id,
            operation: OllamaOperation::List,
        };
        assert!(
            matches!(ollama_request(local.clone(), list.clone()), ServerResponse::Ollama(OllamaResponse::Endpoints(v)) if v == [endpoint.clone()])
        );
        assert!(
            local
                .lock()
                .unwrap()
                .store
                .ollama_endpoints()
                .unwrap()
                .is_empty()
        );
        let disable = OllamaRequest {
            project_id: project.id,
            operation: OllamaOperation::Disable {
                endpoint_id: endpoint.id,
            },
        };
        assert!(
            matches!(ollama_request(local.clone(), disable), ServerResponse::Ollama(OllamaResponse::Endpoint(e)) if !e.enabled)
        );
        assert!(
            !remote
                .lock()
                .unwrap()
                .store
                .ollama_endpoint(endpoint.id)
                .unwrap()
                .unwrap()
                .enabled
        );
        old.store(true, Ordering::SeqCst);
        assert!(
            matches!(ollama_request(local, list), ServerResponse::Error(e) if e.code == "ollama_upgrade_required")
        );
    }
    #[test]
    fn provider_binding_is_immutable() {
        let id = Uuid::new_v4();
        assert!(validate_inference_binding(None, None).is_ok());
        assert!(validate_inference_binding(Some(id), Some(id)).is_ok());
        assert!(validate_inference_binding(None, Some(id)).is_err());
        assert!(validate_inference_binding(Some(id), None).is_err());
        assert!(validate_inference_binding(Some(id), Some(Uuid::new_v4())).is_err());
    }
}
