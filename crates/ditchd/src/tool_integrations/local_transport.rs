use super::*;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::{OpenOptionsExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::time::Duration;

pub fn peer_pid(stream: &UnixStream) -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        let mut pid: i32 = 0;
        let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&mut pid as *mut i32).cast(),
                &mut len,
            )
        } == 0
            && pid > 0
        {
            return Some(pid);
        }
    }
    None
}

pub fn start(lease: Arc<Lease>) -> io::Result<()> {
    let listener = UnixListener::bind(&lease.socket)?;
    if let Err(error) =
        std::fs::set_permissions(&lease.socket, std::fs::Permissions::from_mode(0o600))
            .and_then(|_| listener.set_nonblocking(true))
    {
        let _ = std::fs::remove_file(&lease.socket);
        return Err(error);
    }
    let mut credential = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lease.credential_path)
    {
        Ok(file) => file,
        Err(error) => {
            let _ = std::fs::remove_file(&lease.socket);
            return Err(error);
        }
    };
    if let Err(error) = credential.write_all(lease.credential.as_bytes()) {
        let _ = std::fs::remove_file(&lease.socket);
        let _ = std::fs::remove_file(&lease.credential_path);
        return Err(error);
    }
    std::thread::spawn(move || {
        let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        while !lease.cancelled.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if connections.fetch_add(1, Ordering::AcqRel) >= 8 {
                        connections.fetch_sub(1, Ordering::AcqRel);
                        continue;
                    }
                    let connections = connections.clone();
                    let lease = lease.clone();
                    std::thread::spawn(move || {
                        struct Count(Arc<std::sync::atomic::AtomicUsize>);
                        impl Drop for Count {
                            fn drop(&mut self) {
                                self.0.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        let _count = Count(connections);
                        let _ = handle(stream, lease);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(&lease.socket);
        let _ = std::fs::remove_file(&lease.credential_path);
    });
    Ok(())
}

fn handle(mut stream: UnixStream, lease: Arc<Lease>) -> io::Result<()> {
    let pid = peer_pid(&stream).ok_or_else(|| io::Error::other("peer unavailable"))?;
    if !native::verify_peer(pid, 1, &lease.cli) {
        return Err(io::Error::other("unauthorized peer"));
    }
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut line = String::new();
    BufReader::new((&stream).take((MAX_REQUEST_BYTES + 1) as u64)).read_line(&mut line)?;
    if line.len() > MAX_REQUEST_BYTES || !line.ends_with('\n') {
        return Err(io::Error::other("invalid request"));
    }
    let request: AdapterRequest = serde_json::from_str(&line).map_err(io::Error::other)?;
    if let Err(reason) = authorize_adapter(
        &lease.credential,
        &lease.adapter_pid,
        &lease.cancelled,
        pid,
        &request,
    ) {
        // Only a signature-verified peer reaches this response. Send a bounded
        // reason, never credentials, request arguments or native observations.
        serde_json::to_writer(&mut stream, &json!({"ok":false,"error":reason}))
            .map_err(io::Error::other)?;
        return stream.write_all(b"\n");
    }
    if request.tool == ADAPTER_READY {
        // Read-only catalogue discovery may come from Codex's temporary helper.
        // It cannot replace the execution owner or invoke any native operation.
        serde_json::to_writer(&mut stream, &json!({"ok":true})).map_err(io::Error::other)?;
        return stream.write_all(b"\n");
    }
    let completed = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let monitor = &stream;
        let done = &completed;
        let lease = &lease;
        scope.spawn(move || {
            while !done.load(Ordering::Acquire) {
                let mut byte = 0u8;
                let count = unsafe {
                    libc::recv(
                        monitor.as_raw_fd(),
                        (&mut byte as *mut u8).cast(),
                        1,
                        libc::MSG_PEEK | libc::MSG_DONTWAIT,
                    )
                };
                if count == 0 {
                    lease.revoke();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let result = executor::execute(lease, request);
        completed.store(true, Ordering::Release);
        result
    });
    serde_json::to_writer(&mut stream, &result).map_err(io::Error::other)?;
    stream.write_all(b"\n")
}

/// Called only after verifying the peer's live signature and exact executable.
/// Startup verifies the named thread connection before broad catalogue discovery.
fn authorize_adapter(
    credential: &str,
    owner: &AtomicI32,
    cancelled: &AtomicBool,
    pid: i32,
    request: &AdapterRequest,
) -> Result<(), &'static str> {
    if cancelled.load(Ordering::Acquire) {
        return Err("revoked");
    }
    if pid <= 0 {
        return Err("peerUnavailable");
    }
    if request.credential.len() != credential.len()
        || request
            .credential
            .bytes()
            .zip(credential.bytes())
            .fold(0u8, |v, (a, b)| v | (a ^ b))
            != 0
    {
        return Err("invalidCredential");
    }
    if request.tool == ADAPTER_READY {
        // The first (thread-owned) connection claims execution. Subsequent
        // authenticated discovery helpers receive schemas only, never authority.
        let _ = owner.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire);
        return Ok(());
    }
    match owner.load(Ordering::Acquire) {
        0 => Err("adapterNotReady"),
        bound if bound != pid => Err("adapterChanged"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the actual authorization function, production CLI and installed
    /// Codex. The fixture deliberately bypasses native signature verification;
    /// it has no native provider and refuses every desktop operation.
    #[test]
    #[ignore = "set DITCH_TEST_CODEX_BINARY and DITCH_TEST_DESKTOP_CLI"]
    fn live_codex_discovery_preserves_production_execution_owner() {
        use crate::{app_server_client::Client, oss_tool_policy::OssToolPolicy};
        let binary = std::env::var("DITCH_TEST_CODEX_BINARY").unwrap();
        let cli = std::fs::canonicalize(std::env::var("DITCH_TEST_DESKTOP_CLI").unwrap()).unwrap();
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "adapter-{}",
                &Uuid::new_v4().simple().to_string()[..12]
            ));
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            r#"model_provider = "offline_fixture"
model = "fixture"
[model_providers.offline_fixture]
name = "Offline fixture"
base_url = "http://127.0.0.1:1/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0
"#,
        )
        .unwrap();
        let mut thread_id: Option<String> = None;
        for resumed in [false, true] {
            let socket = root.join("s");
            let credential = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
            let token = root.join("token");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&token)
                .unwrap();
            file.write_all(credential.as_bytes()).unwrap();
            drop(file);
            let listener = UnixListener::bind(&socket).unwrap();
            listener.set_nonblocking(true).unwrap();
            let owner = Arc::new(AtomicI32::new(0));
            let seen = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            // Always stop/join the fixture, including when an assertion fails.
            struct Fixture(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);
            impl Drop for Fixture {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Release);
                    if let Some(worker) = self.1.take() {
                        let _ = worker.join();
                    }
                }
            }
            let worker_owner = owner.clone();
            let worker_seen = seen.clone();
            let worker_stop = stop.clone();
            let worker_credential = credential.clone();
            let worker = std::thread::spawn(move || {
                let cancelled = AtomicBool::new(false);
                while !worker_stop.load(Ordering::Acquire) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(pair) => pair,
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(_) => break,
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let pid = peer_pid(&stream).unwrap();
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    let Ok(request) = serde_json::from_str::<AdapterRequest>(&line) else {
                        continue;
                    };
                    let result = authorize_adapter(
                        &worker_credential,
                        &worker_owner,
                        &cancelled,
                        pid,
                        &request,
                    );
                    worker_seen.lock().unwrap().push(pid);
                    let response = match result {
                        Ok(()) if request.tool == ADAPTER_READY => json!({"ok":true}),
                        Ok(()) => json!({"ok":false,"error":"fixtureRefusesDesktopActions"}),
                        Err(reason) => json!({"ok":false,"error":reason}),
                    };
                    let _ = serde_json::to_writer(&mut stream, &response);
                    let _ = stream.write_all(b"\n");
                }
            });
            let fixture = Fixture(stop, Some(worker));
            let mut client = Client::open(&binary, &root, Some(&home), None).unwrap();
            let policy = OssToolPolicy::discover(
                &json!({}),
                &json!({}),
                Some(json!({
                    "command":cli,"args":["tools","serve","--stdio"],
                    "env":{"DITCH_TOOL_SOCKET":socket,"DITCH_TOOL_CREDENTIAL_FILE":token},
                    "startup_timeout_sec":5
                })),
            );
            let mut params = json!({"cwd":root,"approvalPolicy":"never","sandbox":"danger-full-access","config":{"features":{}}});
            policy.apply_thread(&mut params);
            let method = if let Some(id) = &thread_id {
                params["threadId"] = json!(id);
                "thread/resume"
            } else {
                "thread/start"
            };
            let result = client.call(method, params).unwrap();
            let id = result["thread"]["id"].as_str().unwrap();
            if let Some(previous) = &thread_id {
                assert_eq!(id, previous);
            }
            thread_id = Some(id.to_owned());
            policy
                .verify_thread_adapter(&mut client, id, Duration::from_secs(10))
                .unwrap();
            let execution_pid = owner.load(Ordering::Acquire);
            assert!(execution_pid > 0);
            assert!(seen.lock().unwrap().iter().all(|pid| *pid == execution_pid));
            let inventory = client
                .call("mcpServerStatus/list", json!({"threadId":id}))
                .unwrap();
            policy
                .verify_inventory(inventory["data"].as_array().unwrap())
                .unwrap();
            assert_eq!(owner.load(Ordering::Acquire), execution_pid);
            let other = *seen
                .lock()
                .unwrap()
                .iter()
                .find(|pid| **pid != execution_pid)
                .expect("Codex must exercise secondary discovery for this regression");
            let tool_result = client.call("mcpServer/tool/call",
                json!({"threadId":id,"server":"ditch_tools","tool":"desktop_status","arguments":{}})).unwrap();
            assert!(
                tool_result
                    .to_string()
                    .contains("fixtureRefusesDesktopActions"),
                "the real thread tool call must pass owner authentication before the fixture refuses native dispatch: {tool_result}"
            );
            assert_eq!(
                seen.lock().unwrap().last(),
                Some(&execution_pid),
                "thread tool execution must reuse the authenticated owner after broad discovery"
            );
            let operation = AdapterRequest {
                credential: credential.clone(),
                operation_id: Uuid::new_v4(),
                tool: "desktop_status".into(),
                arguments: json!({}),
            };
            assert_eq!(
                authorize_adapter(
                    &credential,
                    &owner,
                    &AtomicBool::new(false),
                    other,
                    &operation
                ),
                Err("adapterChanged")
            );
            assert_eq!(
                authorize_adapter(
                    &credential,
                    &owner,
                    &AtomicBool::new(true),
                    execution_pid,
                    &operation
                ),
                Err("revoked")
            );
            // Persist an offline, immediately interrupted turn for the next child.
            let turn = client.call("turn/start", json!({"threadId":id,"input":[{"type":"text","text":"Offline adapter regression"}]})).unwrap();
            let _ = client.call(
                "turn/interrupt",
                json!({"threadId":id,"turnId":turn["turn"]["id"]}),
            );
            let mut persisted = false;
            for _ in 0..40 {
                if client
                    .call("thread/read", json!({"threadId":id,"includeTurns":false}))
                    .is_ok()
                {
                    persisted = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(persisted);
            client.graceful = true;
            drop(client);
            drop(fixture);
            std::fs::remove_file(&socket).unwrap();
            std::fs::remove_file(&token).unwrap();
            eprintln!(
                "resumed={resumed}: real CLI discovery passed; production owner unchanged; secondary execution rejected"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_helpers_never_acquire_execution_or_replace_owner() {
        let owner = AtomicI32::new(0);
        let cancelled = AtomicBool::new(false);
        let mut request = AdapterRequest {
            credential: "secret".into(),
            operation_id: Uuid::new_v4(),
            tool: "desktop_act".into(),
            arguments: json!({}),
        };
        let authorize =
            |pid, r: &AdapterRequest| authorize_adapter("secret", &owner, &cancelled, pid, r);
        assert_eq!(authorize(101, &request), Err("adapterNotReady"));
        request.tool = ADAPTER_READY.into();
        assert_eq!(authorize(101, &request), Ok(()));
        assert_eq!(authorize(202, &request), Ok(()));
        assert_eq!(owner.load(Ordering::Acquire), 101);
        for tool in [
            "desktop_status",
            "desktop_list_targets",
            "desktop_observe",
            "desktop_act",
        ] {
            request.tool = tool.into();
            assert_eq!(authorize(202, &request), Err("adapterChanged"));
            assert_eq!(authorize(101, &request), Ok(()));
        }
        request.tool = ADAPTER_READY.into();
        request.credential = "wrong!".into();
        assert_eq!(authorize(303, &request), Err("invalidCredential"));
        assert_eq!(owner.load(Ordering::Acquire), 101);
        request.credential = "secret".into();
        cancelled.store(true, Ordering::Release);
        assert_eq!(authorize(101, &request), Err("revoked"));
        assert_eq!(authorize(202, &request), Err("revoked"));
        request.tool = "desktop_act".into();
        assert_eq!(authorize(101, &request), Err("revoked"));
    }
}
