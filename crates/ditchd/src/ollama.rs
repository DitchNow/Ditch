//! Execution-host inference configuration. No global Codex configuration is written.
use ditch_core::{AppPaths, ollama::*};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    path::PathBuf,
    process::Command,
    time::Duration,
};
use uuid::Uuid;

pub const CAPABILITY: &str = "ollama_inference_v1";
pub struct ResolvedOllamaProvider {
    pub endpoint: OllamaEndpoint,
    pub token: Option<EndpointToken>,
    pub model: String,
}
fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

pub fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && !model.chars().any(|c| c.is_control() || c.is_whitespace())
}
pub fn normalize_url(raw: &str, allow_private_http: bool) -> io::Result<url::Url> {
    if raw.len() > 2048 || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid("Enter a URL without whitespace"));
    }
    let mut url = url::Url::parse(raw).map_err(|_| invalid("Invalid Ollama URL"))?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(invalid(
            "Use an HTTP(S) root URL without credentials, query or fragment",
        ));
    }
    let path = url.path().trim_end_matches('/');
    if !matches!(path, "" | "/v1") {
        return Err(invalid(
            "Enter the Ollama server root URL, optionally ending in /v1",
        ));
    }
    url.set_path("");
    if url.scheme() == "http" && !allow_private_http {
        return Err(invalid(
            "HTTPS is required unless private HTTP is explicitly enabled",
        ));
    }
    Ok(url)
}
fn allowed_ip(ip: IpAddr, http: bool) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            // Includes common cloud metadata endpoints, unspecified and reserved networks.
            if ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || o == [168, 63, 129, 16]
                || ip.is_documentation()
            {
                return false;
            }
            !http || ip.is_private() || ip.is_loopback()
        }
        IpAddr::V6(ip) => {
            if ip == "fd00:ec2::254".parse::<std::net::Ipv6Addr>().unwrap() {
                return false;
            }
            if let Some(v4) = ip.to_ipv4_mapped() {
                return allowed_ip(IpAddr::V4(v4), http);
            }
            let s = ip.segments();
            if ip.is_unspecified()
                || ip.is_multicast()
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] == 0x2001 && s[1] == 0xdb8)
            {
                return false;
            }
            let private = ip.is_loopback() || (s[0] & 0xfe00) == 0xfc00;
            private || (!http && (s[0] & 0xe000) == 0x2000)
        }
    }
}
pub fn validate_destination(url: &url::Url) -> io::Result<Vec<SocketAddr>> {
    let host = url.host_str().unwrap().trim_matches(['[', ']']).to_owned();
    if host == "metadata.google.internal" || host.ends_with(".metadata.google.internal") {
        return Err(invalid("Metadata endpoints are not allowed"));
    }
    let port = url.port_or_known_default().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = tx.send(
            (host.as_str(), port)
                .to_socket_addrs()
                .map(|v| v.collect::<Vec<_>>()),
        );
    });
    let addresses = rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| invalid("Endpoint DNS lookup timed out"))?
        .map_err(|_| invalid("Endpoint hostname could not be resolved"))?;
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|a| !allowed_ip(a.ip(), url.scheme() == "http"))
    {
        return Err(invalid(
            "Endpoint resolves to a blocked destination; public endpoints require HTTPS",
        ));
    }
    Ok(addresses)
}
pub fn validate_token(token: Option<&EndpointToken>) -> io::Result<()> {
    if token.is_some_and(|t| {
        t.0.is_empty()
            || t.0.len() > 8192
            || t.0.chars().any(|c| c.is_control() || c.is_whitespace())
    }) {
        return Err(invalid("Invalid API token"));
    }
    Ok(())
}
pub fn configure(command: &mut Command, provider: &ResolvedOllamaProvider) {
    let key = provider.endpoint.provider_key();
    // JSON strings are valid TOML basic strings for these validated inputs.
    let quote = |v: &str| serde_json::to_string(v).unwrap();
    let prefix = format!("model_providers.{key}");
    for config in [
        format!("model_provider={}", quote(&key)),
        format!("model={}", quote(&provider.model)),
        format!("{prefix}.name={}", quote(&provider.endpoint.display_name)),
        format!(
            "{prefix}.base_url={}",
            quote(&format!("{}/v1", provider.endpoint.api_root_url))
        ),
        format!("{prefix}.wire_api=\"responses\""),
        format!("{prefix}.requires_openai_auth=false"),
        format!("{prefix}.supports_websockets=false"),
    ] {
        command.arg("-c").arg(config);
    }
    if let Some(token) = &provider.token {
        let env = format!("DITCH_OLLAMA_TOKEN_{}", provider.endpoint.id.simple());
        command
            .arg("-c")
            .arg(format!("{prefix}.env_key={}", quote(&env)));
        command.env(env, &token.0);
    }
}

fn secret_path(paths: &AppPaths, id: Uuid) -> PathBuf {
    paths
        .data_dir
        .join("inference-secrets")
        .join(id.to_string())
}
#[cfg(target_os = "macos")]
fn service(paths: &AppPaths) -> String {
    format!("Ditch.Ollama.{}", paths.data_dir.display())
}
pub fn save_token(
    paths: &AppPaths,
    remote: bool,
    id: Uuid,
    token: &EndpointToken,
) -> io::Result<()> {
    validate_token(Some(token))?;
    #[cfg(target_os = "macos")]
    if !remote {
        return security_framework::passwords::set_generic_password(
            &service(paths),
            &id.to_string(),
            token.0.as_bytes(),
        )
        .map_err(|_| invalid("Could not save endpoint credential in Keychain"));
    }
    let _ = remote;
    let path = secret_path(paths, id);
    let dir = path.parent().unwrap();
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err(invalid("Credential directory must be private (0700)"));
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?
        .write_all(token.0.as_bytes())
}
pub fn load_token(
    paths: &AppPaths,
    remote: bool,
    endpoint: &OllamaEndpoint,
) -> io::Result<Option<EndpointToken>> {
    if !endpoint.has_credentials {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    if !remote {
        let bytes = security_framework::passwords::get_generic_password(
            &service(paths),
            &endpoint.id.to_string(),
        )
        .map_err(|_| invalid("Endpoint credential is unavailable in Keychain"))?;
        return String::from_utf8(bytes)
            .map(|s| Some(EndpointToken(s)))
            .map_err(|_| invalid("Invalid stored credential"));
    }
    let _ = remote;
    let path = secret_path(paths, endpoint.id);
    let dir = std::fs::symlink_metadata(path.parent().unwrap())?;
    if !dir.is_dir() || dir.permissions().mode() & 0o077 != 0 {
        return Err(invalid("Credential directory must be private (0700)"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(invalid("Credential file must be private (0600)"));
    }
    let mut value = String::new();
    file.take(8193).read_to_string(&mut value)?;
    let token = EndpointToken(value);
    validate_token(Some(&token))?;
    Ok(Some(token))
}
pub fn remove_token(paths: &AppPaths, remote: bool, id: Uuid) {
    #[cfg(target_os = "macos")]
    if !remote {
        let _ = security_framework::passwords::delete_generic_password(
            &service(paths),
            &id.to_string(),
        );
        return;
    }
    let _ = remote;
    let _ = std::fs::remove_file(secret_path(paths, id));
}
fn body(response: ureq::Response) -> io::Result<String> {
    let mut body = String::new();
    response
        .into_reader()
        .take(1_048_577)
        .read_to_string(&mut body)?;
    if body.len() > 1_048_576 {
        return Err(invalid("Endpoint response exceeded size limit"));
    }
    Ok(body)
}
pub fn probe(
    raw: &str,
    private_http: bool,
    token: Option<&EndpointToken>,
    model: Option<&str>,
) -> io::Result<OllamaProbe> {
    validate_token(token)?;
    let url = normalize_url(raw, private_http)?;
    let addresses = validate_destination(&url)?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(8))
        .resolver(move |_: &str| Ok(addresses.clone()))
        .build();
    let root = url.as_str().trim_end_matches('/');
    let request = |method: &str, path: &str| {
        let request = agent.request(method, &format!("{root}{path}"));
        if let Some(token) = token {
            request.set("Authorization", &format!("Bearer {}", token.0))
        } else {
            request
        }
    };
    let mut result = OllamaProbe {
        models: vec![],
        reachable: false,
        error: None,
        responses_tool_check: false,
    };
    for path in ["/api/tags", "/v1/models"] {
        if let Ok(response) = request("GET", path).call() {
            result.reachable = true;
            if let Ok(value) = body(response).and_then(|s| {
                serde_json::from_str::<serde_json::Value>(&s).map_err(io::Error::other)
            }) {
                let entries = if path == "/api/tags" {
                    value["models"].as_array()
                } else {
                    value["data"].as_array()
                };
                if let Some(entries) = entries {
                    result.models = entries
                        .iter()
                        .filter_map(|v| v["name"].as_str().or(v["id"].as_str()))
                        .filter(|m| valid_model(m))
                        .take(512)
                        .map(str::to_owned)
                        .collect();
                    result.models.sort();
                    result.models.dedup();
                    break;
                }
            }
        }
    }
    if let Some(model) = model {
        if !valid_model(model) {
            return Err(invalid("Enter an exact model ID"));
        }
        let payload = serde_json::json!({"model":model,"input":"Call ditch_probe with ok=true.","stream":true,"store":false,"max_output_tokens":128,
            "tools":[{"type":"function","name":"ditch_probe","description":"Connection test; no action is executed.","parameters":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}}],"tool_choice":{"type":"function","name":"ditch_probe"}});
        if let Ok(response) = request("POST", "/v1/responses").send_json(payload) {
            result.reachable = true;
            if let Ok(data) = body(response) {
                result.responses_tool_check = data
                    .lines()
                    .filter_map(|l| l.strip_prefix("data:"))
                    .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
                    .any(|v| {
                        v["type"] == "response.completed"
                            && v["response"]["status"] == "completed"
                            && v["response"]["output"].as_array().is_some_and(|items| {
                                items.iter().any(|i| {
                                    i["type"] == "function_call"
                                        && i["name"] == "ditch_probe"
                                        && i["arguments"]
                                            .as_str()
                                            .and_then(|a| {
                                                serde_json::from_str::<serde_json::Value>(a).ok()
                                            })
                                            .is_some_and(|a| a["ok"] == true)
                                })
                            })
                    });
            }
        }
        if !result.responses_tool_check {
            result.error = Some(
                "Responses streaming/tool check did not pass. Coding compatibility is unverified."
                    .into(),
            );
        }
    }
    if !result.reachable {
        result.error = Some("Endpoint unavailable, unauthorized, or discovery is not exposed. Check URL/token or enter a model ID manually.".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint() -> OllamaEndpoint {
        OllamaEndpoint {
            id: Uuid::new_v4(),
            display_name: "GPU A".into(),
            api_root_url: "http://127.0.0.1:1234".into(),
            auth_mode: "bearer".into(),
            has_credentials: true,
            allow_private_http: true,
            enabled: true,
            created_at: chrono::Utc::now(),
            models: vec!["qwen:9b".into()],
            reachable: None,
            error: None,
        }
    }
    #[test]
    fn probe_uses_native_fallback_auth_and_bounded_streaming_tool_check() {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for path in ["/api/tags", "/v1/models", "/v1/responses"] {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    headers.push_str(&line);
                }
                assert!(headers.lines().next().unwrap().contains(path));
                assert!(
                    headers
                        .to_lowercase()
                        .contains("authorization: bearer private-probe-token")
                );
                if path == "/v1/responses" {
                    let size: usize = headers
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .and_then(|v| v.parse().ok())
                        })
                        .unwrap();
                    let mut data = vec![0; size];
                    reader.read_exact(&mut data).unwrap();
                    let value: serde_json::Value = serde_json::from_slice(&data).unwrap();
                    assert_eq!(value["model"], "qwen:9b");
                    assert_eq!(value["stream"], true);
                }
                let (status, data) = if path == "/api/tags" {
                    ("404 Not Found", "hidden".to_owned())
                } else if path == "/v1/models" {
                    ("200 OK", r#"{"data":[{"id":"qwen:9b"}]}"#.to_owned())
                } else {
                    (
                        "200 OK",
                        format!(
                            "data: {}\n\n",
                            serde_json::json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"function_call","name":"ditch_probe","arguments":"{\"ok\":true}"}]}})
                        ),
                    )
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",
                    data.len()
                )
                .unwrap();
            }
        });
        let result = probe(
            &format!("http://{address}"),
            true,
            Some(&EndpointToken("private-probe-token".into())),
            Some("qwen:9b"),
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(result.models, ["qwen:9b"]);
        assert!(result.responses_tool_check);
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("private-probe-token")
        );
    }
    #[test]
    fn headless_credentials_are_private_and_never_follow_symlinks() {
        let root = std::env::temp_dir().join(format!("ditch-ollama-secret-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let mut paths = AppPaths::for_remote_user();
        paths.data_dir = root.clone();
        let endpoint = endpoint();
        let token = EndpointToken("test-secret".into());
        save_token(&paths, true, endpoint.id, &token).unwrap();
        assert_eq!(load_token(&paths, true, &endpoint).unwrap(), Some(token));
        let path = secret_path(&paths, endpoint.id);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        remove_token(&paths, true, endpoint.id);
        let elsewhere = root.join("other");
        std::fs::write(&elsewhere, "other-secret").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert!(load_token(&paths, true, &endpoint).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn app_server_start_and_resume_keep_provider_and_mismatch_stops_before_turn() {
        use crate::codex_app_server::{self, Event, Launch};
        let root = std::env::temp_dir().join(format!("ditch-ollama-launch-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("fake-codex");
        std::fs::write(&binary, r#"#!/usr/bin/python3
import json, sys, pathlib
root=pathlib.Path(__file__).parent
assert 'private-launch-token' not in ' '.join(sys.argv)
for line in sys.stdin:
 r=json.loads(line); method=r['method']
 if method=='initialized': continue
 if method=='initialize': result={}
 elif method in ('thread/start','thread/resume'):
  p=r['params']; assert p['model']=='qwen:9b'; assert p['modelProvider'].startswith('ditch_ollama_')
  assert p['config']['web_search']=='disabled'
  (root/'method').write_text(method)
  result={'thread':{'id':'saved-thread','modelProvider':'openai' if (root/'mismatch').exists() else p['modelProvider']}}
 elif method=='turn/start':
  (root/'turn').write_text('started'); result={'turn':{'id':'turn'}}
 else: raise Exception(method)
 print(json.dumps({'id':r['id'],'result':result}),flush=True)
 if method=='turn/start': print(json.dumps({'method':'turn/completed','params':{'threadId':'saved-thread','turn':{'id':'turn','status':'completed'}}}),flush=True)
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let endpoint = endpoint();
        let profile = ditch_core::AgentExecutionProfile {
            model: Some("qwen:9b".into()),
            inference_endpoint_id: Some(endpoint.id),
            ..Default::default()
        };
        let provider = ResolvedOllamaProvider {
            endpoint,
            token: Some(EndpointToken("private-launch-token".into())),
            model: "qwen:9b".into(),
        };
        for (resume, mismatch) in [
            (None, false),
            (Some("saved-thread"), false),
            (Some("saved-thread"), true),
        ] {
            let _ = std::fs::remove_file(root.join("turn"));
            if mismatch {
                std::fs::write(root.join("mismatch"), "yes").unwrap();
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = codex_app_server::spawn_turn(
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
                    prompt: "Test",
                    resume_thread: resume,
                    execution_profile: &profile,
                    codex_home: Some(root.join("home").into_os_string()),
                },
                tx,
            )
            .unwrap();
            let mut failed = false;
            let mut completed = false;
            while let Ok(event) = rx.recv_timeout(Duration::from_secs(5)) {
                match event {
                    Event::Failed(_) => {
                        failed = true;
                        break;
                    }
                    Event::TurnCompleted { .. } => {
                        completed = true;
                        break;
                    }
                    Event::OutputClosed => break,
                    _ => (),
                }
            }
            let mut child = spawned.child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
            assert_eq!(failed, mismatch);
            assert_eq!(completed, !mismatch);
            assert_eq!(root.join("turn").exists(), !mismatch);
            assert_eq!(
                std::fs::read_to_string(root.join("method")).unwrap(),
                if resume.is_some() {
                    "thread/resume"
                } else {
                    "thread/start"
                }
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn urls_and_destinations_are_restricted() {
        assert_eq!(
            normalize_url("https://gpu.example/v1/", false)
                .unwrap()
                .as_str(),
            "https://gpu.example/"
        );
        for url in [
            "https://token@gpu.example",
            "https://gpu.example/?token=x",
            "file:///etc/passwd",
            " https://gpu.example",
            "https://gpu.example/a",
            "http://192.168.1.2",
        ] {
            assert!(normalize_url(url, false).is_err());
        }
        for ip in [
            "169.254.169.254",
            "100.100.100.200",
            "168.63.129.16",
            "::ffff:169.254.169.254",
            "fe80::1",
            "0.0.0.0",
        ] {
            assert!(!allowed_ip(ip.parse().unwrap(), false));
        }
        assert!(!allowed_ip("8.8.8.8".parse().unwrap(), true));
        assert!(allowed_ip("192.168.1.20".parse().unwrap(), true));
    }
    #[test]
    fn launch_uses_arguments_and_child_environment_only() {
        let endpoint = OllamaEndpoint {
            id: Uuid::new_v4(),
            display_name: "GPU \"quoted\" $(no-shell)".into(),
            api_root_url: "https://gpu.example".into(),
            auth_mode: "bearer".into(),
            has_credentials: true,
            allow_private_http: false,
            enabled: true,
            created_at: chrono::Utc::now(),
            models: vec![],
            reachable: None,
            error: None,
        };
        let mut command = Command::new("codex");
        configure(
            &mut command,
            &ResolvedOllamaProvider {
                endpoint,
                token: Some(EndpointToken("private-token".into())),
                model: "qwen:9b".into(),
            },
        );
        let args = format!("{:?}", command.get_args().collect::<Vec<_>>());
        assert!(!args.contains("private-token"));
        assert!(args.contains("requires_openai_auth=false"));
        assert!(
            command
                .get_envs()
                .any(|(_, v)| v == Some(std::ffi::OsStr::new("private-token")))
        );
    }
}
