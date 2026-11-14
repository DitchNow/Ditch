use super::*;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

fn desktop_lock() -> std::io::Result<File> {
    let root =
        PathBuf::from(std::env::var_os("HOME").ok_or_else(|| std::io::Error::other("no home"))?)
            .join("Library/Application Support/Ditch Desktop Coordination");
    desktop_lock_at(&root)
}

fn desktop_lock_at(root: &std::path::Path) -> std::io::Result<File> {
    match std::fs::create_dir(&root) {
        Ok(()) => std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let metadata = std::fs::symlink_metadata(&root)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(std::io::Error::other(
            "invalid desktop coordination directory",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join("desktop.lock"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Fake(Arc<AtomicUsize>);
    impl ToolIntegration for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn tools(&self) -> Vec<String> {
            vec![
                "desktop_status".into(),
                "desktop_act".into(),
                "desktop_observe".into(),
            ]
        }
        fn invoke(&self, request: &Value, _: &Arc<AtomicBool>, _: Option<File>) -> Value {
            assert_eq!(request["visualizer"], false, "model arguments cannot enable feedback");
            self.0.fetch_add(1, Ordering::SeqCst);
            json!({"ok":true,"result":{"test":"private-observation"}})
        }
    }

    #[test]
    fn scoped_requests_are_bound_deduplicated_and_revoked_without_payload_events() {
        let state = Arc::new(Mutex::new(crate::tests::test_runtime()));
        let calls = Arc::new(AtomicUsize::new(0));
        let lease = Lease {
            agent_id: AgentId::default(),
            run_id: Uuid::new_v4(),
            cancelled: Arc::new(AtomicBool::new(false)),
            state: Arc::downgrade(&state),
            provider: Arc::new(Fake(calls.clone())),
            socket: PathBuf::new(),
            credential: String::new(),
            credential_path: PathBuf::new(),
            cli: PathBuf::new(),
            adapter_pid: AtomicI32::new(0),
            tools_registered: AtomicBool::new(false),
            binding: Mutex::new((None, None)),
            completed: Mutex::new(HashMap::new()),
            vision: false,
            visualizer: false,
        };
        let request = |id, tool: &str| AdapterRequest {
            credential: String::new(),
            operation_id: id,
            tool: tool.into(),
            arguments: json!({"visualizer":true}),
        };
        assert!(lease.confirm_registered(std::process::id() as i32).is_err(), "catalogue alone is not an authenticated connection");
        lease.adapter_pid.store(std::process::id() as i32, Ordering::Release);
        assert!(lease.confirm_registered((std::process::id() as i32) + 1).is_err());
        assert!(!lease.tools_registered.load(Ordering::Acquire));
        lease.confirm_registered(std::process::id() as i32).unwrap();
        assert!(lease.tools_registered.load(Ordering::Acquire));
        assert_eq!(
            execute(&lease, request(Uuid::new_v4(), "desktop_act"))["error"],
            "turnNotReady"
        );
        lease.bind_thread("thread");
        lease.bind_turn("turn");
        let mut image = request(Uuid::new_v4(), "desktop_observe");
        image.arguments = json!({"include_image":true});
        assert_eq!(execute(&lease, image)["error"], "visionUnsupported");
        let events = subscribe();
        let id = Uuid::new_v4();
        assert_eq!(execute(&lease, request(id, "desktop_status"))["ok"], true);
        assert_eq!(execute(&lease, request(id, "desktop_status"))["ok"], true);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Losing the response cache cannot cause a durable operation to replay.
        lease.completed.lock().unwrap().clear();
        assert_eq!(
            execute(&lease, request(id, "desktop_status"))["error"],
            "outcome_unknown"
        );
        for event in events.try_iter() {
            let serialized = serde_json::to_string(&event).unwrap();
            assert!(!serialized.contains("private-observation"));
            assert!(!serialized.contains("arguments"));
        }
        let mut exited = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let exited_pid = exited.id() as i32;
        exited.wait().unwrap();
        lease.adapter_pid.store(exited_pid, Ordering::Release);
        assert!(lease.confirm_registered(exited_pid).is_err(), "an exited execution owner requires a fresh turn");
        lease.revoke();
        assert!(lease.confirm_registered(std::process::id() as i32).is_err());
        assert_eq!(
            execute(&lease, request(Uuid::new_v4(), "desktop_status"))["error"],
            "revoked"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn desktop_ownership_is_nonblocking_and_released_by_file_lifetime() {
        let root = std::env::temp_dir().join(format!("ditch-desktop-lock-{}", Uuid::new_v4()));
        let first = desktop_lock_at(&root).unwrap();
        assert!(desktop_lock_at(&root).is_err());
        drop(first);
        assert!(desktop_lock_at(&root).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub fn execute(lease: &Lease, request: AdapterRequest) -> Value {
    if lease.cancelled.load(Ordering::Acquire) {
        return json!({"ok":false,"error":"revoked"});
    }
    if !lease.provider.tools().contains(&request.tool) {
        return json!({"ok":false,"error":"unknownTool"});
    }
    if request.tool != "desktop_status" && !lease.bound() {
        return json!({"ok":false,"error":"turnNotReady"});
    }
    if request.tool == "desktop_observe"
        && request.arguments["include_image"] == true
        && !lease.vision
    {
        return json!({"ok":false,"error":"visionUnsupported","message":"Use accessibility observations without include_image for this model."});
    }
    if let Some(result) = lease
        .completed
        .lock()
        .unwrap()
        .get(&request.operation_id)
        .cloned()
    {
        return result;
    }
    let Some(state) = lease.state.upgrade() else {
        return json!({"ok":false,"error":"revoked"});
    };
    let receipt = ToolOperationReceipt {
        operation_id: request.operation_id,
        run_id: lease.run_id,
        agent_id: lease.agent_id,
        tool: request.tool.clone(),
        state: "dispatching".into(),
    };
    match state.lock().unwrap().store.reserve_tool_operation(&receipt) {
        Ok(true) => {}
        Ok(false) => {
            return json!({"ok":false,"error":"outcome_unknown","message":"Operation was already accepted; observe before continuing."});
        }
        Err(_) => return json!({"ok":false,"error":"receiptUnavailable"}),
    }
    let lock = if request.tool == "desktop_status" {
        None
    } else {
        match desktop_lock() {
            Ok(lock) => Some(lock),
            Err(_) => {
                let _ = state
                    .lock()
                    .unwrap()
                    .store
                    .finish_tool_operation(request.operation_id, "failed");
                return json!({"ok":false,"error":"desktopBusy"});
            }
        }
    };
    lease.event(request.operation_id, &request.tool, "running");
    let result = lease.provider.invoke(
        &json!({"sessionId":lease.run_id,"operationId":request.operation_id,
        "tool":request.tool,"arguments":request.arguments,"visualizer":lease.visualizer}),
        &lease.cancelled,
        lock,
    );
    let outcome = if result["error"] == "outcome_unknown" {
        "outcome_unknown"
    } else if result["ok"] == true {
        "completed"
    } else if lease.cancelled.load(Ordering::Acquire) {
        "cancelled"
    } else {
        "failed"
    };
    if state
        .lock()
        .unwrap()
        .store
        .finish_tool_operation(request.operation_id, outcome)
        .is_err()
    {
        lease.revoke();
        return json!({"ok":false,"error":"outcome_unknown"});
    }
    lease.event(request.operation_id, &request.tool, outcome);
    let mut completed = lease.completed.lock().unwrap();
    if completed.len() >= 8 {
        completed.clear();
    }
    completed.insert(request.operation_id, result.clone());
    result
}
