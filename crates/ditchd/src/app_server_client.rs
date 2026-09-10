//! Short-lived JSON-RPC client for capability/discovery and turn initialization.
//! No model turn is started by opening this connection.
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;
pub struct Client {
    pub child: Option<Arc<Mutex<Child>>>,
    pub writer: Option<Arc<Mutex<ChildStdin>>>,
    pub graceful: bool,
    pub incoming: Option<mpsc::Receiver<String>>,
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
        let (tx, rx) = mpsc::sync_channel(128);
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
                let mut bytes = Vec::new();
                let result = reader
                    .by_ref()
                    .take((MAX_MESSAGE + 1) as u64)
                    .read_until(b'\n', &mut bytes);
                if !matches!(result, Ok(n) if n > 0 && n <= MAX_MESSAGE) {
                    break;
                }
                let Ok(line) = String::from_utf8(bytes) else {
                    break;
                };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        client.call("initialize", json!({"clientInfo":{"name":"the_ditch","title":"Ditch","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
        client.send(&json!({"method":"initialized","params":{}}))?;
        Ok(client)
    }
    pub fn send(&self, value: &Value) -> io::Result<()> {
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
                .map_err(|e| io::Error::other(format!("App Server {method}: {e}")))?;
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
    fn expired_rpc_deadline_rejects_even_an_already_queued_reply() {
        let mut child = Command::new("/bin/cat")
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let writer = child.stdin.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(2);
        tx.send(json!({"method":"irrelevant/notification"}).to_string())
            .unwrap();
        tx.send(json!({"id":1,"result":{}}).to_string()).unwrap();
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
