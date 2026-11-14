//! Per-thread OSS tool policy. Never writes the user's Codex configuration.
use serde_json::{Value, json};
use std::{collections::BTreeSet, io};

#[derive(Clone, Debug)]
pub struct OssToolPolicy {
    disabled_plugins: Vec<String>,
    config: Value,
    desktop: bool,
}

impl OssToolPolicy {
    pub fn discover(config: &Value, catalog: &Value, desktop: Option<Value>) -> Self {
        let mut plugins: BTreeSet<String> = config["plugins"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(id, _)| id.clone())
            .collect();
        for marketplace in catalog["marketplaces"].as_array().into_iter().flatten() {
            for plugin in marketplace["plugins"].as_array().into_iter().flatten() {
                if plugin["installed"] == true || plugin["enabled"] == true {
                    if let Some(id) = plugin["id"].as_str() {
                        plugins.insert(id.into());
                    }
                }
            }
        }
        let mut servers: serde_json::Map<String, Value> = config["mcp_servers"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, _)| (name.clone(), json!({"enabled":false})))
            .collect();
        let has_desktop = desktop.is_some();
        if let Some(mut desktop) = desktop {
            // Explicitly override inherited enabled=false for this reserved server.
            desktop["enabled"] = json!(true);
            servers.insert("ditch_tools".into(), desktop);
        }
        Self {
            config: json!({"plugins":plugins.iter().map(|id| (id.clone(),json!({"enabled":false}))).collect::<serde_json::Map<_,_>>(),
                "mcp_servers":servers}),
            disabled_plugins: plugins.into_iter().collect(),
            desktop: has_desktop,
        }
    }

    pub fn apply_thread(&self, params: &mut Value) {
        for key in ["plugins", "mcp_servers"] {
            params["config"][key] = self.config[key].clone();
        }
        params["config"]["features"]["apps"] = json!(false);
        params["developerInstructions"] = json!(if self.desktop {
            "Ditch computer use for this OSS turn is provided by Peekaboo through the ditch_tools MCP server: desktop_status, desktop_list_targets, desktop_observe, desktop_act. Use these tools for desktop interaction. Earlier conversation references to CUA or mcp__cua_repl are stale; those tools are unavailable in this turn. Do not substitute shell/AppleScript/browser plugins for desktop actions. Full Access authorizes supported actions; macOS permissions are checked by Ditch. List targets, observe an exact window, act on a fresh snapshot, and observe again to verify. Report tool errors and unverified actions honestly. A shell listing, supplied image, or successful dispatch is not proof of a desktop action. Ditch's own apps, protected system surfaces, and status/menu-bar items are unsupported; explain that limit instead of clicking a similarly named application menu. The visualizer is controlled by the user's turn option, not by you."
        } else {
            "Ditch desktop tools are unavailable for this OSS turn. CUA and inherited plugin tools are disabled. Do not claim to see or control the desktop, or use shell/AppleScript as a substitute. Ordinary coding and conversation remain available. Earlier desktop-tool references in this conversation do not establish current access."
        });
    }

    pub fn apply_turn(&self, params: &mut Value) {
        // Codex 0.166.0 describes this as saved metadata that does NOT yet
        // filter capabilities. Keep it accurate, but enforcement is the
        // config.plugins override plus the verified effective MCP inventory.
        params["disabledPluginIds"] = json!(self.disabled_plugins);
    }

    pub fn verify_thread_adapter(
        &self,
        client: &mut crate::app_server_client::Client,
        thread: &str,
        timeout: std::time::Duration,
    ) -> io::Result<()> {
        let inventory = client.call_timeout(
            "mcpServerStatus/list",
            json!({"threadId":thread,"serverName":"ditch_tools","detail":"toolsAndAuthOnly"}),
            timeout,
        )?;
        self.verify_inventory(inventory["data"].as_array().ok_or_else(|| {
            io::Error::other("Codex did not return the Ditch thread tool inventory")
        })?)
    }

    pub fn verify_inventory(&self, servers: &[Value]) -> io::Result<()> {
        let fail = |message: &str| {
            io::Error::other(format!(
                "OSS tool routing: {message}. No model turn was started."
            ))
        };
        let mut found = false;
        for server in servers {
            if server["name"] == "ditch_tools" && self.desktop {
                if found {
                    return Err(fail("duplicate Ditch adapter"));
                }
                found = true;
                let expected: BTreeSet<_> = ditch_core::tool_integrations::desktop_tool_schemas()
                    .into_iter()
                    .map(|t| t.0)
                    .collect();
                let actual: BTreeSet<_> = server["tools"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(name, _)| name.clone())
                    .collect();
                if server["runtimeStatus"] != "connected" {
                    let state = match server["runtimeStatus"].as_str() {
                        Some("starting") => "still starting",
                        Some("failed") => "failed to start",
                        Some("disabled") => "disabled",
                        Some("cancelled") => "cancelled",
                        Some("authenticationRequired") => "authentication required",
                        _ => "connection state unavailable",
                    };
                    return Err(fail(&format!("Ditch adapter {state}")));
                }
                if !server["toolsError"].is_null() {
                    // Classify known adapter failures without reflecting arbitrary
                    // subprocess stderr, paths, credentials or server payloads.
                    let error = server["toolsError"].as_str().unwrap_or("");
                    let reason = if error.contains("timed out") {
                        "Ditch adapter discovery timed out"
                    } else if error.contains("invalidCredential") {
                        "Ditch adapter credential was rejected"
                    } else if error.contains("revoked") {
                        "Ditch desktop turn was revoked during discovery"
                    } else if error.contains("adapterChanged") {
                        "Ditch execution adapter changed during discovery"
                    } else if error.contains("Ditch runtime connection unavailable") {
                        "Ditch adapter could not connect or authenticate to the runtime"
                    } else {
                        "Ditch adapter tool discovery failed"
                    };
                    return Err(fail(reason));
                }
                if actual != expected {
                    let missing: Vec<_> = expected.difference(&actual).cloned().collect();
                    let extras = actual.difference(&expected).count();
                    return Err(fail(&format!(
                        "Ditch tool catalogue mismatch (missing: {}; unexpected tools: {extras})",
                        if missing.is_empty() {
                            "none".into()
                        } else {
                            missing.join(", ")
                        }
                    )));
                }
            } else if server["runtimeStatus"] != "disabled"
                || server["tools"].as_object().is_none_or(|t| !t.is_empty())
            {
                return Err(fail("an inherited tool server is still available"));
            }
        }
        if self.desktop && !found {
            return Err(fail("Ditch desktop tools are missing"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launch_enforces_policy_before_new_or_resumed_oss_model_turns() {
        use crate::codex_app_server::{Event, Launch, spawn_turn};
        use std::{fs, os::unix::fs::PermissionsExt, sync::mpsc, time::Duration};
        for (resume, leaked) in [(false, false), (true, false), (false, true), (true, true)] {
            let root = std::env::temp_dir().join(format!("ditch-routing-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let capture = root.join("requests.jsonl");
            let binary = root.join("codex-fixture");
            let script=r#"#!/usr/bin/python3
import sys,json
for line in sys.stdin:
 v=json.loads(line)
 with open(CAPTURE,'a') as f:f.write(line)
 if 'id' not in v:continue
 m=v['method'];p=v.get('params',{})
 if m=='initialize':r={}
 elif m=='config/read':r={'config':{'model_context_window':8192,'model_auto_compact_token_limit':6144,'plugins':{'unified-computer-use@openai-bundled':{'enabled':True}},'mcp_servers':{'external':{}}}}
 elif m=='plugin/list':r={'marketplaces':[]}
 elif m=='skills/list':r={'data':[]}
 elif m in ['thread/start','thread/resume']:r={'thread':{'id':'existing-thread','modelProvider':p['modelProvider']},'model':p['model']}
 elif m=='mcpServerStatus/list':r={'data':LEAK,'nextCursor':None}
 elif m=='turn/start':
  print(json.dumps({'id':v['id'],'result':{'turn':{'id':'t'}}}),flush=True)
  print(json.dumps({'method':'turn/completed','params':{'threadId':'existing-thread','turn':{'id':'t','status':'completed','items':[]}}}),flush=True)
  break
 else:raise Exception(m)
 print(json.dumps({'id':v['id'],'result':r}),flush=True)
"#.replace("CAPTURE",&json!(capture).to_string()).replace("LEAK",if leaked {"[{'name':'cua_repl','runtimeStatus':'connected','tools':{'js':{}}}]"} else {"[]"});
            fs::write(&binary, script).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            let provider=crate::ollama::ResolvedOllamaProvider {
                endpoint: serde_json::from_value(json!({"id":uuid::Uuid::new_v4(),"display_name":"fixture","api_root_url":"http://127.0.0.1:1","auth_mode":"none","has_credentials":false,"allow_private_http":true,"enabled":true,"created_at":chrono::Utc::now(),"models":["qwen"],"reachable":null,"error":null})).unwrap(),
                token:None, model:"qwen".into(), context_window:8192,
            };
            let profile = ditch_core::AgentExecutionProfile {
                model: Some("qwen".into()),
                ..Default::default()
            };
            let (tx, rx) = mpsc::channel();
            let result = spawn_turn(
                Launch {
                    inference_provider: Some(&provider),
                    restricted: false,
                    protected_roots: vec![],
                    remote: false,
                    extra_roots: &[],
                    on_spawn: None,
                    binary: binary.to_str().unwrap(),
                    cwd: &root,
                    project_id: ditch_core::ProjectId::new(),
                    agent_id: ditch_core::AgentId::new(),
                    prompt: "hello",
                    resume_thread: resume.then_some("existing-thread"),
                    execution_profile: &profile,
                    codex_home: None,
                },
                tx,
            );
            if leaked {
                assert!(result.is_err());
            } else {
                let spawned = result.unwrap();
                loop {
                    if matches!(
                        rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                        Event::OutputClosed
                    ) {
                        break;
                    }
                }
                spawned.child.lock().unwrap().wait().unwrap();
            }
            let requests: Vec<Value> = fs::read_to_string(&capture)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            let thread = requests
                .iter()
                .find(|r| {
                    r["method"]
                        == if resume {
                            "thread/resume"
                        } else {
                            "thread/start"
                        }
                })
                .unwrap();
            assert_eq!(
                thread["params"]["config"]["plugins"]["unified-computer-use@openai-bundled"]["enabled"],
                false
            );
            let turn = requests.iter().find(|r| r["method"] == "turn/start");
            if leaked {
                assert!(turn.is_none());
            } else {
                assert_eq!(
                    turn.unwrap()["params"]["disabledPluginIds"],
                    json!(["unified-computer-use@openai-bundled"])
                );
                assert!(
                    requests
                        .iter()
                        .position(|r| r["method"] == "mcpServerStatus/list")
                        .unwrap()
                        < requests
                            .iter()
                            .position(|r| r["method"] == "turn/start")
                            .unwrap()
                );
            }
            fs::remove_dir_all(root).unwrap();
        }
    }
    /// Real Codex protocol + harmless fixture MCP plugin. No inference or GUI
    /// action, no user home, and no global plugin settings are touched.
    #[test]
    #[ignore = "set DITCH_TEST_CODEX_BINARY to an installed Codex executable"]
    fn live_codex_plugin_isolation_on_new_and_resumed_threads() {
        use std::fs;
        let binary = std::env::var("DITCH_TEST_CODEX_BINARY").unwrap();
        let root = std::env::temp_dir().join(format!("ditch-plugin-test-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let marketplace = root.join("marketplace");
        let plugin = marketplace.join("plugins/cua-fixture");
        fs::create_dir_all(plugin.join(".codex-plugin")).unwrap();
        fs::create_dir_all(marketplace.join(".agents/plugins")).unwrap();
        fs::create_dir_all(&home).unwrap();
        let server = root.join("server.py");
        fs::write(&server, r#"import sys,json
for line in sys.stdin:
 v=json.loads(line)
 if 'id' not in v: continue
 m=v.get('method')
 if m=='initialize': r={'protocolVersion':v['params']['protocolVersion'],'capabilities':{'tools':{}},'serverInfo':{'name':'harmless-cua-fixture','version':'1'}}
 elif m=='tools/list': r={'tools':[{'name':n,'description':'Fixture only; never controls the computer','inputSchema':{'type':'object','properties':{}}} for n in (['desktop_status','desktop_list_targets','desktop_observe','desktop_act'] if sys.argv[1:]==['desktop'] else ['js'])]}
 elif m in ['resources/list','resources/templates/list']: r={'resources':[],'resourceTemplates':[]}
 else: r={}
 print(json.dumps({'jsonrpc':'2.0','id':v['id'],'result':r}),flush=True)
"#).unwrap();
        fs::write(
            plugin.join(".codex-plugin/plugin.json"),
            json!({"name":"cua-fixture","version":"1.0.0","mcpServers":"./.mcp.json"}).to_string(),
        )
        .unwrap();
        fs::write(
            plugin.join(".mcp.json"),
            json!({"mcpServers":{"cua_repl":{"command":"/usr/bin/python3","args":[server]}}})
                .to_string(),
        )
        .unwrap();
        fs::write(marketplace.join(".agents/plugins/marketplace.json"), json!({"name":"ditch-test","plugins":[{"name":"cua-fixture","source":{"source":"local","path":"./plugins/cua-fixture"},"policy":{"installation":"AVAILABLE","authentication":"ON_USE"},"category":"Productivity"}]}).to_string()).unwrap();
        let config_path = home.join("config.toml");
        let config_text = format!(
            "model_provider = \"offline_fixture\"\nmodel = \"fixture\"\n[model_providers.offline_fixture]\nname = \"Offline fixture\"\nbase_url = \"http://127.0.0.1:1/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nrequest_max_retries = 0\nstream_max_retries = 0\n[marketplaces.ditch-test]\nsource_type = \"local\"\nsource = {}\n[plugins.\"cua-fixture@ditch-test\"]\nenabled = true\n",
            json!(marketplace)
        );
        fs::write(&config_path, &config_text).unwrap();
        let mut client =
            crate::app_server_client::Client::open(&binary, &root, Some(&home), None).unwrap();
        let config = client
            .call("config/read", json!({"includeLayers":false}))
            .unwrap();
        let plugins = client
            .call("plugin/list", json!({"cwds":[root],"forceRefetch":false}))
            .unwrap();
        let policy = OssToolPolicy::discover(&config["config"], &plugins, None);
        // Reproduce the former gap: apps=false does not disable a plugin MCP.
        let baseline=client.call("thread/start",json!({"cwd":root,"modelProvider":"offline_fixture","model":"fixture","config":{"features":{"apps":false},"model_providers":{"offline_fixture":{"name":"Offline fixture","base_url":"http://127.0.0.1:1/v1","wire_api":"responses","requires_openai_auth":false,"request_max_retries":0,"stream_max_retries":0}}},"approvalPolicy":"never","sandbox":"danger-full-access"})).unwrap();
        let original = baseline["thread"]["id"].as_str().unwrap().to_owned();
        let inventory = client
            .call("mcpServerStatus/list", json!({"threadId":original}))
            .unwrap();
        assert!(
            inventory["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["pluginId"] == "cua-fixture@ditch-test"
                    && !s["tools"].as_object().unwrap().is_empty()),
            "fixture plugin must actually be available before testing isolation: {inventory}"
        );
        assert!(
            policy
                .verify_inventory(inventory["data"].as_array().unwrap())
                .is_err()
        );
        // Codex only persists a rollout after turn/start. The fixture provider
        // deliberately has no listener; interrupt immediately, without inference.
        let turn=client.call("turn/start",json!({"threadId":original,"input":[{"type":"text","text":"Offline routing fixture"}]})).unwrap();
        let _ = client.call(
            "turn/interrupt",
            json!({"threadId":original,"turnId":turn["turn"]["id"]}),
        );
        let mut persisted = false;
        for _ in 0..40 {
            if client
                .call(
                    "thread/read",
                    json!({"threadId":original,"includeTurns":false}),
                )
                .is_ok()
            {
                persisted = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(persisted, "offline fixture rollout did not become readable");
        client.graceful = true;
        drop(client);
        // Codex may record project trust while creating its baseline thread.
        // The isolation policy itself must leave that home configuration intact.
        let before_policy = fs::read_to_string(&config_path).unwrap();
        for desktop in [false, true] {
            let policy = OssToolPolicy::discover(
                &config["config"],
                &plugins,
                desktop.then(|| json!({"command":"/usr/bin/python3","args":[server,"desktop"]})),
            );
            for resume in [None, Some(original.as_str())] {
                let mut client =
                    crate::app_server_client::Client::open(&binary, &root, Some(&home), None)
                        .unwrap();
                let mut params = json!({"cwd":root,"approvalPolicy":"never","sandbox":"danger-full-access","config":{"features":{}}});
                policy.apply_thread(&mut params);
                let method = if let Some(id) = resume {
                    params["threadId"] = json!(id);
                    "thread/resume"
                } else {
                    "thread/start"
                };
                let result = client.call(method, params).unwrap();
                let id = &result["thread"]["id"];
                if let Some(previous) = resume {
                    assert_eq!(id, previous);
                }
                if desktop {
                    policy
                        .verify_thread_adapter(
                            &mut client,
                            id.as_str().unwrap(),
                            std::time::Duration::from_secs(10),
                        )
                        .unwrap();
                }
                let inventory = client
                    .call("mcpServerStatus/list", json!({"threadId":id}))
                    .unwrap();
                policy
                    .verify_inventory(inventory["data"].as_array().unwrap())
                    .unwrap();
                eprintln!("{method}: inherited CUA excluded; Ditch catalogue enabled={desktop}");
            }
        }
        assert_eq!(fs::read_to_string(&config_path).unwrap(), before_policy);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn disables_configured_and_discovered_plugins_without_mutating_source() {
        let config = json!({"plugins":{"unified-computer-use@openai-bundled":{"enabled":true}},"mcp_servers":{"other":{"command":"unchanged"},"ditch_tools":{"enabled":false}}});
        let policy = OssToolPolicy::discover(
            &config,
            &json!({"marketplaces":[{"plugins":[{"id":"new@market","installed":true}]}]}),
            Some(json!({"command":"signed-cli"})),
        );
        for method in ["thread/start", "thread/resume"] {
            let mut params = json!({"config":{"features":{}},"method":method});
            policy.apply_thread(&mut params);
            policy.apply_turn(&mut params);
            assert_eq!(
                params["config"]["plugins"]["unified-computer-use@openai-bundled"]["enabled"],
                false
            );
            assert_eq!(params["config"]["mcp_servers"]["other"]["enabled"], false);
            assert_eq!(
                params["config"]["mcp_servers"]["ditch_tools"]["enabled"],
                true
            );
            assert_eq!(
                params["disabledPluginIds"],
                json!(["new@market", "unified-computer-use@openai-bundled"])
            );
        }
        assert_eq!(
            config["plugins"]["unified-computer-use@openai-bundled"]["enabled"],
            true
        );
    }
    #[test]
    fn requires_exact_connected_catalogue_and_rejects_leaked_servers() {
        let p = OssToolPolicy::discover(&json!({}), &json!({}), Some(json!({})));
        let tools = ditch_core::tool_integrations::desktop_tool_schemas()
            .into_iter()
            .map(|t| (t.0, json!({})))
            .collect::<serde_json::Map<_, _>>();
        let good = json!({"name":"ditch_tools","runtimeStatus":"connected","tools":tools});
        assert!(p.verify_inventory(&[good.clone()]).is_ok());
        assert!(p.verify_inventory(&[]).is_err());
        assert!(
            p.verify_inventory(&[
                good.clone(),
                json!({"name":"cua_repl","runtimeStatus":"connected","tools":{"js":{}}})
            ])
            .is_err()
        );
        for state in ["failed", "starting", "disabled"] {
            let mut broken = good.clone();
            broken["runtimeStatus"] = json!(state);
            assert!(p.verify_inventory(&[broken]).is_err());
        }
        let mut missing = good;
        missing["tools"]
            .as_object_mut()
            .unwrap()
            .remove("desktop_act");
        assert!(p.verify_inventory(&[missing]).is_err());
        let chat = OssToolPolicy::discover(&json!({}), &json!({}), None);
        assert!(chat.verify_inventory(&[]).is_ok());
        let mut params = json!({"config":{"features":{}}});
        chat.apply_thread(&mut params);
        assert!(
            params["developerInstructions"]
                .as_str()
                .unwrap()
                .contains("unavailable")
        );
    }

    #[test]
    fn connection_errors_are_specific_and_do_not_reflect_subprocess_payloads() {
        let policy = OssToolPolicy::discover(&json!({}), &json!({}), Some(json!({})));
        let server = json!({"name":"ditch_tools", "runtimeStatus":"connected", "tools":{},
            "toolsError":"MCP error invalidCredential; PRIVATE_TOKEN_AND_PATH"});
        let error = policy.verify_inventory(&[server]).unwrap_err().to_string();
        assert!(error.contains("credential was rejected"));
        assert!(!error.contains("PRIVATE_TOKEN_AND_PATH"));
        let error = policy
            .verify_inventory(&[
                json!({"name":"ditch_tools","runtimeStatus":"starting","tools":{}}),
            ])
            .unwrap_err()
            .to_string();
        assert!(error.contains("still starting"));
    }
}
