//! Short-lived JSON-RPC client for capability/discovery and turn initialization.
//! No model turn is started by opening this connection.
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

// Compatibility allowance for servers that still return hydrated resume history.
// Keep the queue small so this limit cannot multiply by 128 buffered messages.
pub const MAX_MESSAGE: usize = 64 * 1024 * 1024;
pub struct Client {
    pub child: Option<Arc<Mutex<Child>>>,
    pub writer: Option<Arc<Mutex<ChildStdin>>>,
    pub graceful: bool,
    pub incoming: Option<mpsc::Receiver<io::Result<String>>>,
    next_id: u64,
}
impl Client {
    pub fn open(
        binary: &str,
        cwd: &Path,
        home: Option<&Path>,
        on_spawn: Option<&dyn Fn(i32) -> io::Result<()>>,
    ) -> io::Result<Self> {
        let mut command = Command::new(binary);
        command
            .arg("app-server")
            .current_dir(cwd)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(path) = crate::effective_path_for_binary(Path::new(binary)) {
            command.env("PATH", path);
        }
        if let Some(home) = home {
            command.env("CODEX_HOME", home);
        }
        let mut child = command.spawn()?;
        let writer = Arc::new(Mutex::new(
            child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("missing stdin"))?,
        ));
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing stdout"))?;
        let child = Arc::new(Mutex::new(child));
        let (tx, rx) = mpsc::sync_channel(1);
        let mut client = Self {
            child: Some(child),
            writer: Some(writer),
            graceful: false,
            incoming: Some(rx),
            next_id: 0,
        };
        if let Some(callback) = on_spawn {
            callback(client.child.as_ref().unwrap().lock().unwrap().id() as i32)?;
        }
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = match read_message(&mut reader, MAX_MESSAGE) {
                    Ok(Some(line)) => Ok(line),
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                let failed = message.is_err();
                if tx.send(message).is_err() || failed {
                    break;
                }
            }
        });
        client.call("initialize", json!({"clientInfo":{"name":"the_ditch","title":"Ditch","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
        client.send(&json!({"method":"initialized","params":{}}))?;
        Ok(client)
    }
    pub fn send(&self, value: &Value) -> io::Result<()> {
        // Ditch renders its own persisted transcript. This only omits history
        // from the RPC response; Codex still loads the full model context.
        // Centralize this for user agents, workers, and coordinator resumes.
        let mut compact_resume;
        let value = if value.get("method").and_then(Value::as_str) == Some("thread/resume") {
            compact_resume = value.clone();
            compact_resume["params"]["excludeTurns"] = json!(true);
            &compact_resume
        } else {
            value
        };
        let mut writer = self
            .writer
            .as_ref()
            .unwrap()
            .lock()
            .map_err(|_| io::Error::other("App Server writer poisoned"))?;
        serde_json::to_writer(&mut *writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()
    }
    pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.call_timeout(method, params, Duration::from_secs(20))
    }
    pub fn call_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> io::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("App Server {method} timed out"),
                ));
            }
            let timeout = deadline.saturating_duration_since(Instant::now());
            let line = self
                .incoming
                .as_ref()
                .unwrap()
                .recv_timeout(timeout)
                .map_err(|e| io::Error::other(format!("App Server {method}: {e}")))??;
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if value.get("id").and_then(Value::as_u64) == Some(id) && value.get("method").is_none()
            {
                if let Some(error) = value.get("error") {
                    return Err(io::Error::other(format!("App Server {method}: {error}")));
                }
                return value
                    .get("result")
                    .cloned()
                    .ok_or_else(|| io::Error::other("Missing RPC result"));
            }
            if value.get("method").is_some() && value.get("id").is_some() {
                self.send(&json!({"id":value["id"],"error":{"code":-32601,"message":"No interactive request is allowed during discovery"}}))?;
            }
        }
    }
    pub fn set_roots(&mut self, roots: &[std::path::PathBuf]) -> io::Result<()> {
        if !roots.is_empty() {
            self.call("skills/extraRoots/set", json!({"extraRoots":roots}))?;
        }
        Ok(())
    }
}

fn read_message(reader: &mut impl BufRead, limit: usize) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    let count = reader
        .take((limit + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Codex App Server output read failed: {error}"),
            )
        })?;
    if count == 0 {
        return Ok(None);
    }
    if count > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Codex App Server output exceeded the {limit}-byte message limit. If this happened while resuming a conversation, update Codex on the execution host to a version supporting thread/resume excludeTurns."
            ),
        ));
    }
    if bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Codex App Server output ended in the middle of a message",
        ));
    }
    String::from_utf8(bytes).map(Some).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Codex App Server output is not valid UTF-8: {error}"),
        )
    })
}
impl Drop for Client {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            self.writer.take();
            if self.graceful {
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    if child.lock().unwrap().try_wait().ok().flatten().is_some() {
                        return;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                // A non-responsive server retains its durable lease. Do not kill the
                // connection owner before it has terminated its separate command group.
                std::thread::spawn(move || {
                    let _ = child.lock().unwrap().wait();
                });
                return;
            }
            let mut child = child.lock().unwrap();
            // SAFETY: this child was spawned in its own process group above.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;
    #[test]
    fn reader_accepts_large_legacy_resume_and_preserves_next_message() {
        let response = json!({"id":"ditch:thread","result":{"thread":{
            "id":"old-thread","turns":[{"text":"x".repeat(12 * 1024 * 1024)}]
        }}})
        .to_string();
        let mut input = io::Cursor::new(format!("{response}\n{{}}\n"));
        assert_eq!(
            read_message(&mut input, MAX_MESSAGE)
                .unwrap()
                .unwrap()
                .trim_end(),
            response
        );
        assert_eq!(
            read_message(&mut input, MAX_MESSAGE).unwrap().unwrap(),
            "{}\n"
        );
        assert!(read_message(&mut input, MAX_MESSAGE).unwrap().is_none());
    }

    #[test]
    fn reader_reports_size_truncation_utf8_and_io_errors() {
        let mut input = io::Cursor::new(b"12345678\n");
        assert_eq!(read_message(&mut input, 9).unwrap().unwrap(), "12345678\n");
        input.set_position(0);
        let error = read_message(&mut input, 8).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("8-byte message limit"));
        assert_eq!(
            read_message(&mut io::Cursor::new(b"{}"), 8)
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            read_message(&mut io::Cursor::new(b"\xff\n"), 8)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture failure"))
            }
        }
        let error = read_message(&mut BufReader::new(Broken), 8).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(
            error
                .to_string()
                .contains("output read failed: fixture failure")
        );
    }

    /// Resume only: no model request and no modification of the source session.
    #[test]
    #[ignore = "requires a disposable copy of a Codex home and thread"]
    fn live_large_history_resume() {
        let binary = std::env::var("DITCH_TEST_CODEX_BINARY").unwrap();
        let root = std::env::var("DITCH_TEST_PROJECT_ROOT").unwrap();
        let home = std::env::var("DITCH_TEST_CODEX_HOME").unwrap();
        let thread = std::env::var("DITCH_TEST_RESUME_THREAD").unwrap();
        for policy in ["on-request", "never"] {
            let mut client =
                Client::open(&binary, Path::new(&root), Some(Path::new(&home)), None).unwrap();
            let result = client
                .call(
                    "thread/resume",
                    json!({
                        "threadId":thread,"cwd":root,"approvalPolicy":policy,
                        "sandbox":"workspace-write","model":"gpt-6-astra"
                    }),
                )
                .unwrap();
            assert_eq!(result["thread"]["id"], thread);
            assert_eq!(result["model"], "gpt-6-astra");
            assert_eq!(result["approvalPolicy"], policy);
            assert!(result["thread"]["turns"].as_array().unwrap().is_empty());
            assert!(
                client
                    .child
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .try_wait()
                    .unwrap()
                    .is_none()
            );
            eprintln!(
                "{policy}: resumed existing thread; response {} bytes",
                result.to_string().len()
            );
        }
    }

    #[test]
    fn expired_rpc_deadline_rejects_even_an_already_queued_reply() {
        let mut child = Command::new("/bin/cat")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let writer = child.stdin.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(2);
        tx.send(Ok(json!({"method":"irrelevant/notification"}).to_string()))
            .unwrap();
        tx.send(Ok(json!({"id":1,"result":{}}).to_string()))
            .unwrap();
        let mut client = Client {
            child: Some(Arc::new(Mutex::new(child))),
            writer: Some(Arc::new(Mutex::new(writer))),
            graceful: false,
            incoming: Some(rx),
            next_id: 0,
        };
        let error = client
            .call_timeout("config/read", json!({}), Duration::ZERO)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
