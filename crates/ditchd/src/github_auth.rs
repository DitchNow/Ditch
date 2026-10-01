//! Native-only connection state. No authentication output enters general events.
use super::*;
use serde::{Deserialize, Serialize};

const SETTING: &str = "github_connection_v1";
const DEVICE_URL: &str = "https://github.com/login/device";
const STORAGE_ERROR: &str = "GitHub CLI is not using secure Keychain storage. Ditch remains disconnected. Shared credentials were not deleted or rewritten; unlock your Mac login Keychain and retry browser sign-in. Review any plaintext shared hosts.yml credential using GitHub CLI. Browser login may have saved a plaintext credential if Keychain failed.";
const INVALID_AUTH: &str = "GitHub sign-in expired or was revoked. Connect GitHub again.";
const CANCEL_DETAIL: &str = "Ditch is disconnected. Browser authorization may still have updated the shared GitHub CLI sign-in; it remains available to other tools.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Account {
    pub id: u64,
    pub login: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Connection {
    consent: bool,
    config: PathBuf,
    generation: String,
    phase: String,
    account: Option<Account>,
    detail: String,
}
impl Default for Connection {
    fn default() -> Self {
        Self {
            consent: false,
            config: PathBuf::new(),
            generation: "unconfigured".into(),
            phase: "disconnected".into(),
            account: None,
            detail: String::new(),
        }
    }
}
struct Session {
    generation: String,
    cancel: Arc<AtomicBool>,
    code: Option<String>,
    browser_ready: bool,
    running: bool,
    read_progress: Option<String>,
}
static SESSIONS: OnceLock<Mutex<HashMap<PathBuf, Session>>> = OnceLock::new();
fn sessions() -> &'static Mutex<HashMap<PathBuf, Session>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn read(s: &RuntimeState) -> Result<Connection, String> {
    s.store
        .setting(SETTING)
        .map_err(|_| "Cannot read GitHub connection")?
        .map(|v| serde_json::from_str(&v).map_err(|_| "Invalid saved GitHub connection".into()))
        .unwrap_or_else(|| Ok(Connection::default()))
}
fn save(s: &mut RuntimeState, c: &Connection) -> Result<(), String> {
    s.store
        .set_setting(
            SETTING,
            &serde_json::to_string(c).map_err(|_| "Cannot encode connection")?,
        )
        .map_err(|_| "Cannot save GitHub connection".into())
}
fn root(s: &RuntimeState) -> PathBuf {
    s.paths.data_dir.join("tools/github")
}

// Browser sign-in uses the standard Mac gh configuration, never inherited overrides.
fn config_path(s: &RuntimeState) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("Mac home directory unavailable")?;
    let path = home.join(".config/gh");
    if !path.is_absolute()
        || path
            .components()
            .any(|v| matches!(v, std::path::Component::ParentDir))
    {
        return Err("Choose an absolute configuration directory without '..'".into());
    }
    fn resolve(path: &Path) -> Result<PathBuf, String> {
        if path.exists() {
            return fs::canonicalize(path)
                .map_err(|_| "Cannot resolve configuration directory".into());
        }
        Ok(resolve(path.parent().ok_or("Invalid configuration path")?)?
            .join(path.file_name().ok_or("Invalid directory")?))
    }
    let path = resolve(&path)?;
    if s.projects
        .values()
        .any(|p| !p.is_remote() && resolve(&p.root).is_ok_and(|project| path.starts_with(project)))
    {
        return Err("GitHub configuration must be outside project directories".into());
    }
    if path.exists() {
        use std::os::unix::fs::MetadataExt;
        let meta = fs::metadata(&path).map_err(|_| "Cannot inspect configuration directory")?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
            return Err("Choose a user-owned directory without group/other write access".into());
        }
    }
    Ok(path)
}
pub(super) fn status(state: &Arc<Mutex<RuntimeState>>) -> Result<Value, String> {
    let mut s = state.lock().unwrap();
    let root = root(&s);
    let mut c = read(&s)?;
    let jobs = sessions().lock().unwrap();
    let job = jobs.get(&root).filter(|j| j.generation == c.generation);
    if matches!(
        c.phase.as_str(),
        "installing" | "checking" | "authenticating" | "verifying" | "awaiting_account"
    ) && !job.is_some_and(|j| j.running)
    {
        c.phase = "disconnected".into();
        c.account = None;
        c.detail = "The previous attempt ended. Reconnect explicitly to continue.".into();
        save(&mut s, &c)?;
    }
    Ok(
        json!({"version":VERSION,"installed":root.join("active/gh").is_file(),"connected":c.phase=="connected",
        "connection_state":c.phase,"consent":c.consent,"generation":c.generation,"account":c.account,
        "detail":c.detail,"config_path":c.config,"device_code":job.and_then(|j|j.code.clone()),
        "browser_ready":job.is_some_and(|j|j.browser_ready),"read_progress":job.and_then(|j|j.read_progress.clone()),"busy":job.is_some_and(|j|j.running)}),
    )
}
pub(super) fn control(
    state: Arc<Mutex<RuntimeState>>,
    request: GitHubRequest,
) -> Result<Value, String> {
    match request {
        GitHubRequest::Status => return status(&state),
        GitHubRequest::CancelRead => {
            let s = state.lock().unwrap();
            if read(&s)?.phase == "connected" {
                if let Some(job) = sessions().lock().unwrap().get_mut(&root(&s)) {
                    job.cancel.store(true, Ordering::SeqCst);
                }
            }
        }
        GitHubRequest::Disconnect | GitHubRequest::Cancel => {
            let mut s = state.lock().unwrap();
            let mut c = read(&s)?;
            c.generation = Uuid::new_v4().to_string();
            c.phase = "disconnected".into();
            c.account = None;
            c.detail = CANCEL_DETAIL.into();
            save(&mut s, &c)?;
            if let Some(job) = sessions().lock().unwrap().get_mut(&root(&s)) {
                job.cancel.store(true, Ordering::SeqCst);
                job.code = None;
                job.browser_ready = false;
            }
        }
        GitHubRequest::Connect => return start(state),
        GitHubRequest::OpenBrowser => {
            {
                let s = state.lock().unwrap();
                let c = read(&s)?;
                let jobs = sessions().lock().unwrap();
                if !jobs.get(&root(&s)).is_some_and(|j| {
                    j.generation == c.generation
                        && j.browser_ready
                        && !j.cancel.load(Ordering::SeqCst)
                }) {
                    return Err("No active GitHub device authorization".into());
                }
            }
            // Fixed URL only. Check the launcher result without holding runtime locks.
            let mut command = Command::new("/usr/bin/open");
            command.arg(DEVICE_URL).stdin(Stdio::null());
            bounded_output_cancel(command, 5, 4096, &AtomicBool::new(false))
                .map_err(|_| "Could not open your browser. Try Open browser again.")?;
        }
        _ => return Err("Unsupported connection operation".into()),
    }
    status(&state)
}
fn start(state: Arc<Mutex<RuntimeState>>) -> Result<Value, String> {
    let (root, c, cancel) = {
        let mut s = state.lock().unwrap();
        let root = root(&s);
        let mut c = read(&s)?;
        let mut jobs = sessions().lock().unwrap();
        if jobs.get(&root).is_some_and(|j| j.running) {
            return Err("A GitHub operation is running; cancel or wait for it".into());
        }
        // Connect is the sole, explicit authorization action in the native UI.
        c.config = config_path(&s)?;
        c.consent = true;
        if let Some(previous) = jobs.get(&root) {
            previous.cancel.store(true, Ordering::SeqCst);
        }
        c.generation = Uuid::new_v4().to_string();
        c.account = None;
        c.phase = "checking".into();
        c.detail = "Preparing GitHub connection…".into();
        save(&mut s, &c)?;
        let cancel = Arc::new(AtomicBool::new(false));
        jobs.insert(
            root.clone(),
            Session {
                generation: c.generation.clone(),
                cancel: cancel.clone(),
                code: None,
                browser_ready: false,
                running: true,
                read_progress: None,
            },
        );
        (root, c, cancel)
    };
    let background = state.clone();
    thread::spawn(move || {
        if let Err(error) = run_job(&background, &root, &c, &cancel) {
            let _ = update(&background, &c.generation, "failed", None, &error);
        }
        let mut jobs = sessions().lock().unwrap();
        if let Some(j) = jobs.get_mut(&root).filter(|j| j.generation == c.generation) {
            j.running = false;
            j.code = None;
            j.browser_ready = false;
        }
    });
    status(&state)
}
fn update(
    state: &Arc<Mutex<RuntimeState>>,
    generation: &str,
    phase: &str,
    account: Option<Account>,
    detail: &str,
) -> Result<(), String> {
    let mut s = state.lock().unwrap();
    let mut c = read(&s)?;
    if c.generation != generation {
        return Err("GitHub operation cancelled".into());
    }
    c.phase = phase.into();
    c.account = account;
    c.detail = detail.into();
    save(&mut s, &c)
}
fn run_job(
    state: &Arc<Mutex<RuntimeState>>,
    root: &Path,
    c: &Connection,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let binary = root.join("active/gh");
    if verify_version(&binary, root, &c.config, cancel).is_err() {
        check_cancel(cancel)?;
        let _install = INSTALL.lock().unwrap();
        check_cancel(cancel)?;
        install(root, cancel, &|message| {
            let _ = update(state, &c.generation, "installing", None, message);
        })?;
        verify_version(&binary, root, &c.config, cancel)?;
    }
    update(
        state,
        &c.generation,
        "authenticating",
        None,
        "Preparing browser sign-in…",
    )?;
    // Never silently adopt an existing account. Every Connect runs the browser flow.
    login(root, c, cancel)?;
    check_cancel(cancel)?;
    if let Some(job) = sessions()
        .lock()
        .unwrap()
        .get_mut(root)
        .filter(|j| j.generation == c.generation)
    {
        job.code = None;
        job.browser_ready = false;
    }
    update(
        state,
        &c.generation,
        "verifying",
        None,
        "Verifying your GitHub account…",
    )?;
    let account = probe(&binary, root, &c.config, cancel)?
        .ok_or("Login ended without a verified GitHub account. Try again.")?;
    update(
        state,
        &c.generation,
        "connected",
        Some(account),
        "GitHub is connected.",
    )
}
pub(super) fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err("GitHub operation cancelled".into())
    } else {
        Ok(())
    }
}
fn verify_version(
    binary: &Path,
    root: &Path,
    config: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut cmd = managed_command(binary, root, config);
    cmd.arg("--version");
    let bytes = bounded_output_cancel(cmd, 10, 4096, cancel)?;
    if !String::from_utf8_lossy(&bytes).starts_with(&format!("gh version {VERSION} ")) {
        return Err("Managed CLI version changed. Repair it before connecting.".into());
    }
    Ok(())
}
fn safe_status(
    binary: &Path,
    root: &Path,
    config: &Path,
    cancel: &AtomicBool,
) -> Result<Value, String> {
    let mut cmd = managed_command(binary, root, config);
    // Pinned status.go removes Token before JSON serialization without --show-token.
    // jq selects only non-secret metadata inside gh, before it reaches this process.
    cmd.args([
        "auth",
        "status",
        "--hostname",
        "github.com",
        "--active",
        "--json",
        "hosts",
        "--jq",
        "[.hosts[\"github.com\"][]? | {state,active,host,login,tokenSource,gitProtocol, invalidCredential: ((.error // \"\") | test(\"HTTP 401|Bad credentials\"; \"i\"))}]",
    ]);
    serde_json::from_slice(&bounded_output_cancel(cmd, 30, 32768, cancel)?)
        .map_err(|_| "Invalid GitHub authentication status".into())
}
fn active_entry(value: &Value) -> Result<Option<&Value>, String> {
    let rows = value.as_array().ok_or("Invalid authentication status")?;
    let entries: Vec<_> = rows
        .iter()
        .filter(|v| v["active"] == true && v["host"] == "github.com")
        .collect();
    if entries.is_empty() {
        return Ok(None);
    }
    if entries.len() != 1 {
        return Err("Ambiguous active GitHub account".into());
    }
    let v = entries[0];
    if v["state"] != "success" {
        return Err(if v["invalidCredential"] == true {
            INVALID_AUTH.into()
        } else {
            "Could not verify GitHub right now. Check your connection and try again.".into()
        });
    }
    if v["tokenSource"] != "keyring" {
        return Err(STORAGE_ERROR.into());
    }
    Ok(Some(v))
}
fn probe(
    binary: &Path,
    root: &Path,
    config: &Path,
    cancel: &AtomicBool,
) -> Result<Option<Account>, String> {
    let value = safe_status(binary, root, config, cancel)?;
    let Some(entry) = active_entry(&value)? else {
        return Ok(None);
    };
    let mut cmd = managed_command(binary, root, config);
    cmd.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        "GET",
        "user",
        "--jq",
        "{id,login}",
    ]);
    let account: Account = serde_json::from_slice(&bounded_output_cancel(cmd, 30, 32768, cancel)?)
        .map_err(|_| "Could not verify GitHub identity")?;
    if account.id == 0 || entry["login"].as_str() != Some(account.login.as_str()) {
        return Err("Account changed during verification; reconnect".into());
    }
    let after = safe_status(binary, root, config, cancel)?;
    if active_entry(&after)?.and_then(|v| v["login"].as_str()) != Some(account.login.as_str()) {
        return Err("Account changed during verification; reconnect".into());
    }
    Ok(Some(account))
}

/// Before/after checks cannot provide transactional isolation from external gh.
pub(super) struct Binding {
    state: Arc<Mutex<RuntimeState>>,
    generation: String,
    account: Account,
    pub config: PathBuf,
    pub cancel: Arc<AtomicBool>,
    root: PathBuf,
    deadline: Instant,
}
impl Binding {
    pub fn capture(state: Arc<Mutex<RuntimeState>>) -> Result<Self, String> {
        let s = state.lock().unwrap();
        let c = read(&s)?;
        if !c.consent || c.phase != "connected" {
            return Err("Connect GitHub before reading private repositories".into());
        }
        let root = root(&s);
        let mut jobs = sessions().lock().unwrap();
        let session = jobs.entry(root.clone()).or_insert_with(|| Session {
            generation: c.generation.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            code: None,
            browser_ready: false,
            running: false,
            read_progress: None,
        });
        if session.generation != c.generation || session.cancel.load(Ordering::SeqCst) {
            session.generation = c.generation.clone();
            session.cancel = Arc::new(AtomicBool::new(false));
        }
        session.read_progress = None;
        Ok(Self {
            state: state.clone(),
            generation: c.generation,
            account: c.account.ok_or("No bound identity")?,
            config: c.config,
            cancel: session.cancel.clone(),
            root,
            deadline: Instant::now() + Duration::from_secs(600),
        })
    }
    pub fn check_commit(&self, s: &RuntimeState) -> Result<(), String> {
        check_cancel(&self.cancel)?;
        if Instant::now() >= self.deadline {
            return Err("GitHub request took too long. Try again.".into());
        }
        let c = read(s)?;
        if c.phase != "connected"
            || c.generation != self.generation
            || c.account.as_ref() != Some(&self.account)
        {
            return Err("Connection changed; discarded this result".into());
        }
        Ok(())
    }
    pub fn progress(&self, detail: &str) {
        if let Some(job) = sessions()
            .lock()
            .unwrap()
            .get_mut(&self.root)
            .filter(|j| j.generation == self.generation && !j.cancel.load(Ordering::SeqCst))
        {
            job.read_progress = Some(detail.into());
        }
    }
    pub fn verify(&self) -> Result<(), String> {
        self.check_commit(&self.state.lock().unwrap())?;
        match probe(
            &self.root.join("active/gh"),
            &self.root,
            &self.config,
            &self.cancel,
        ) {
            Ok(Some(account)) if account == self.account => {
                self.check_commit(&self.state.lock().unwrap())
            }
            Ok(_) => {
                self.cancel.store(true, Ordering::SeqCst);
                let _ = update(
                    &self.state,
                    &self.generation,
                    "account_changed",
                    None,
                    "The shared account changed. Reconnect to confirm it; previous browsing results were cleared.",
                );
                Err("GitHub account changed; reconnect".into())
            }
            Err(e) => {
                // Cancellation and transient network failures must not revoke a connection.
                if e == INVALID_AUTH || e == STORAGE_ERROR {
                    let _ = update(&self.state, &self.generation, "failed", None, &e);
                }
                Err(e)
            }
        }
    }
}

impl Drop for Binding {
    fn drop(&mut self) {
        if let Some(job) = sessions()
            .lock()
            .unwrap()
            .get_mut(&self.root)
            .filter(|j| j.generation == self.generation && Arc::ptr_eq(&j.cancel, &self.cancel))
        {
            job.read_progress = None;
        }
    }
}

#[derive(Default)]
struct LoginParser {
    text: String,
    escape: u8,
}
impl LoginParser {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), String> {
        for &b in bytes {
            match self.escape {
                1 => {
                    self.escape = if b == b'[' {
                        2
                    } else if b == b']' {
                        3
                    } else {
                        0
                    }
                }
                2 => {
                    if (0x40..=0x7e).contains(&b) {
                        self.escape = 0;
                    }
                }
                3 => {
                    if b == 7 {
                        self.escape = 0;
                    } else if b == 27 {
                        self.escape = 4;
                    }
                }
                4 => self.escape = if b == b'\\' { 0 } else { 3 },
                _ => {
                    if b == 27 {
                        self.escape = 1;
                    } else if b == b'\n' || b == b'\r' || b == b'\t' || (32..127).contains(&b) {
                        self.text.push(b as char);
                    }
                }
            }
        }
        if self.text.len() > 65536 {
            return Err("Unexpected authentication output; retry connection".into());
        }
        if self
            .text
            .contains("Authentication credentials saved in plain text")
        {
            return Err(STORAGE_ERROR.into());
        }
        Ok(())
    }
    fn code(&self) -> Option<String> {
        let code = self
            .text
            .split("First copy your one-time code: ")
            .nth(1)?
            .split_whitespace()
            .next()?;
        (code.len() == 9
            && code.as_bytes()[4] == b'-'
            && code
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 4 || b.is_ascii_uppercase() || b.is_ascii_digit()))
        .then(|| code.to_string())
    }
}
fn login_command(binary: &Path, root: &Path, config: &Path) -> Command {
    let mut command = managed_command(binary, root, config);
    // Pipes + GH_PROMPT_DISABLED avoid survey's terminal queries entirely.
    // Omitting --git-protocol preserves existing preferences; noninteractive
    // login skips credential-helper and SSH-key setup.
    command.args([
        "auth",
        "login",
        "--hostname",
        "github.com",
        "--web",
        "--skip-ssh-key",
        "--clipboard=false",
    ]);
    command
}
fn login(root: &Path, c: &Connection, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    let mut command = login_command(&root.join("active/gh"), root, &c.config);
    command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start GitHub sign-in")?;
    let (sender, receiver) = mpsc::sync_channel(16);
    let readers = [
        child
            .stdout
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|mut reader| {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut bytes = [0; 2048];
            while let Ok(n) = reader.read(&mut bytes) {
                if n == 0 || sender.send(bytes[..n].to_vec()).is_err() {
                    break;
                }
            }
        })
    })
    .collect::<Vec<_>>();
    drop(sender);
    let mut parser = LoginParser::default();
    let deadline = Instant::now() + Duration::from_secs(600);
    let result = (|| {
        loop {
            check_cancel(cancel)?;
            if Instant::now() >= deadline {
                return Err("GitHub sign-in expired. Try again.".into());
            }
            if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(40)) {
                parser.feed(&bytes)?;
                if let Some(job) =
                    sessions().lock().unwrap().get_mut(root).filter(|j| {
                        j.generation == c.generation && !j.cancel.load(Ordering::SeqCst)
                    })
                {
                    job.code = parser.code();
                    job.browser_ready = job.code.is_some();
                }
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|_| "Cannot monitor GitHub sign-in")?
            {
                // Drain until EOF so the final Keychain warning cannot race child exit.
                // A stuck descendant remains bounded and cancellable.
                loop {
                    check_cancel(cancel)?;
                    if Instant::now() >= deadline {
                        return Err("GitHub sign-in expired. Try again.".into());
                    }
                    match receiver.recv_timeout(Duration::from_millis(40)) {
                        Ok(bytes) => parser.feed(&bytes)?,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    }
                }
                return if status.success() {
                    Ok(())
                } else {
                    Err("GitHub browser sign-in could not finish. Check your connection and try again.".into())
                };
            }
        }
    })();
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    drop(receiver);
    for reader in readers {
        let _ = reader.join();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connect_always_uses_browser_and_binds_verified_account_without_confirmation() {
        let (state, root, c) = fixture();
        run_job(&state, &root, &c, &Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(status(&state).unwrap()["connected"], true);
        let calls = fs::read_to_string(c.config.join("calls")).unwrap();
        let login = calls.find("auth login").unwrap();
        assert!(login < calls.find("auth status").unwrap());
        assert!(!calls.contains("auth switch"));
        assert!(!calls.contains("config get"));
    }
    #[test]
    fn cancelling_a_read_does_not_disconnect_or_allow_its_commit() {
        let (state, _, _) = fixture();
        let old = Binding::capture(state.clone()).unwrap();
        control(state.clone(), GitHubRequest::CancelRead).unwrap();
        assert!(old.check_commit(&state.lock().unwrap()).is_err());
        assert_eq!(status(&state).unwrap()["connected"], true);
        let next = Binding::capture(state.clone()).unwrap();
        next.check_commit(&state.lock().unwrap()).unwrap();
    }
    #[test]
    fn browser_login_uses_pipes_and_never_answers_interactive_prompts() {
        let (_, root, c) = fixture();
        fs::write(
            root.join("active/gh"),
            r#"#!/bin/sh
[ ! -t 0 ] && [ ! -t 1 ] && [ ! -t 2 ] || exit 70
[ "$GH_PROMPT_DISABLED" = 1 ] || exit 71
printf '! First copy your one-time code: ABCD-1234\n' >&2
printf 'Open this URL to continue in your web browser: https://github.com/login/device\n' >&2
"#,
        )
        .unwrap();
        login(&root, &c, &Arc::new(AtomicBool::new(false))).unwrap();
    }
    #[test]
    fn final_plaintext_warning_is_not_lost_when_login_exits() {
        let (_, root, c) = fixture();
        fs::write(
            root.join("active/gh"),
            "#!/bin/sh\nprintf 'Authentication credentials saved in plain text' >&2\n",
        )
        .unwrap();
        assert_eq!(
            login(&root, &c, &Arc::new(AtomicBool::new(false))).unwrap_err(),
            STORAGE_ERROR
        );
    }
    #[test]
    #[ignore = "Set DITCH_GITHUB_TEST_BINARY to the real managed CLI; no account or external network is used"]
    fn real_cli_noninteractive_login_reaches_device_request_without_terminal_queries() {
        let (_, root, c) = fixture();
        let binary = PathBuf::from(std::env::var("DITCH_GITHUB_TEST_BINARY").unwrap());
        let mut command = login_command(&binary, &root, &c.config);
        // Isolated config, no auth files, and all HTTPS directed at a closed local port.
        command.env("HTTPS_PROXY", "http://127.0.0.1:1");
        command
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Real gh hung before the device authorization request");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("/login/device/code"), "{error}");
        assert!(!error.contains("Authenticate Git"));
        assert!(!result.stderr.windows(4).any(|v| v == b"\x1b[6n"));
    }
    fn fixture() -> (Arc<Mutex<RuntimeState>>, PathBuf, Connection) {
        let state = Arc::new(Mutex::new(super::super::super::tests::test_runtime()));
        let root = root(&state.lock().unwrap());
        fs::create_dir_all(root.join("active")).unwrap();
        let config = root.join("fixture-config");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("status.json"),r#"[{"state":"success","active":true,"host":"github.com","login":"alice","tokenSource":"keyring","gitProtocol":"https"}]"#).unwrap();
        fs::write(config.join("user.json"), r#"{"id":1,"login":"alice"}"#).unwrap();
        let script = r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_CONFIG_DIR/calls"
case "$1 $2" in
  "--version ") printf 'gh version 2.101.0 (fixture)\n' ;;
  "auth status") /bin/cat "$GH_CONFIG_DIR/status.json" ;;
  "api --hostname") /bin/cat "$GH_CONFIG_DIR/user.json" ;;
  "auth login") printf '! First copy your one-time code: ABCD-1234\n' >&2 ;;
  *) exit 74 ;;
esac
"#;
        fs::write(root.join("active/gh"), script).unwrap();
        fs::set_permissions(root.join("active/gh"), fs::Permissions::from_mode(0o700)).unwrap();
        let c = Connection {
            consent: true,
            config,
            generation: Uuid::new_v4().to_string(),
            phase: "connected".into(),
            account: Some(Account {
                id: 1,
                login: "alice".into(),
            }),
            detail: String::new(),
        };
        save(&mut state.lock().unwrap(), &c).unwrap();
        (state, root, c)
    }
    #[test]
    fn parser_handles_every_chunk_boundary_without_terminal_interaction() {
        let output = b"! First copy your one-time code: ABCD-1234\nOpen this URL to continue in your web browser: https://github.com/login/device\n";
        for split in 0..=output.len() {
            let mut parser = LoginParser::default();
            parser.feed(&output[..split]).unwrap();
            parser.feed(&output[split..]).unwrap();
            assert_eq!(parser.code().as_deref(), Some("ABCD-1234"));
        }
        let mut parser = LoginParser::default();
        parser.feed(b"\x1b[6nUnknown terminal prompt").unwrap();
        assert!(parser.code().is_none());
        assert!(
            parser
                .feed(b"Authentication credentials saved in plain text")
                .is_err()
        );
    }
    #[test]
    fn transient_verification_failure_preserves_connection_but_revocation_does_not() {
        let (state, _, c) = fixture();
        let binding = Binding::capture(state.clone()).unwrap();
        fs::write(
            c.config.join("status.json"),
            r#"[{"active":true,"host":"github.com","state":"timeout"}]"#,
        )
        .unwrap();
        assert!(binding.verify().is_err());
        assert_eq!(status(&state).unwrap()["connected"], true);
        fs::write(
            c.config.join("status.json"),
            r#"[{"active":true,"host":"github.com","state":"error","invalidCredential":true}]"#,
        )
        .unwrap();
        assert_eq!(binding.verify().unwrap_err(), INVALID_AUTH);
        assert_eq!(status(&state).unwrap()["connected"], false);
    }
    #[test]
    fn cancellation_during_probe_preserves_connection() {
        let (state, root, c) = fixture();
        let binding = Binding::capture(state.clone()).unwrap();
        fs::write(root.join("active/gh"), "#!/bin/sh\nexec /bin/sleep 20\n").unwrap();
        let cancel = binding.cancel.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel.store(true, Ordering::SeqCst);
        });
        assert!(binding.verify().is_err());
        worker.join().unwrap();
        assert_eq!(status(&state).unwrap()["connected"], true);
        assert!(fs::read(c.config.join("status.json")).is_ok());
    }
    #[test]
    fn status_schema_checks_state_and_storage_even_with_successful_exit() {
        for value in [
            json!([{"active":true,"host":"github.com","state":"error","tokenSource":"keyring"}]),
            json!([{"active":true,"host":"github.com","state":"success","tokenSource":"/private/hosts.yml"}]),
            json!([{"active":true,"host":"github.com","state":"success","tokenSource":"GH_TOKEN"}]),
        ] {
            assert!(active_entry(&value).is_err());
        }
        assert!(active_entry(&json!([])).unwrap().is_none());
    }
    #[test]
    fn real_process_boundary_projects_only_safe_auth_fields() {
        let (_, root, c) = fixture();
        assert_eq!(
            probe(
                &root.join("active/gh"),
                &root,
                &c.config,
                &AtomicBool::new(false)
            )
            .unwrap(),
            c.account
        );
        let calls = fs::read_to_string(c.config.join("calls")).unwrap();
        assert!(calls.contains("--json hosts --jq"));
        assert!(
            calls.contains("{state,active,host,login,tokenSource,gitProtocol, invalidCredential:")
        );
        for forbidden in [
            "--show-token",
            "auth token",
            "auth logout",
            "auth switch",
            "setup-git",
            "--insecure-storage",
        ] {
            assert!(!calls.contains(forbidden));
        }
    }
    #[test]
    fn external_account_switch_invalidates_connection_before_results() {
        let (state, root, c) = fixture();
        let binding = Binding::capture(state.clone()).unwrap();
        binding.verify().unwrap();
        fs::write(c.config.join("status.json"),r#"[{"state":"success","active":true,"host":"github.com","login":"bob","tokenSource":"keyring"}]"#).unwrap();
        fs::write(c.config.join("user.json"), r#"{"id":2,"login":"bob"}"#).unwrap();
        assert!(binding.verify().is_err());
        assert_eq!(
            status(&state).unwrap()["connection_state"],
            "account_changed"
        );
        assert!(binding.check_commit(&state.lock().unwrap()).is_err());
        sessions().lock().unwrap().remove(&root);
    }
    #[test]
    fn disconnect_is_local_durable_and_rejects_late_success() {
        let (state, root, c) = fixture();
        let binding = Binding::capture(state.clone()).unwrap();
        let before = fs::read(c.config.join("status.json")).unwrap();
        control(state.clone(), GitHubRequest::Disconnect).unwrap();
        assert!(binding.cancel.load(Ordering::SeqCst));
        assert!(
            update(
                &state,
                &c.generation,
                "connected",
                c.account.clone(),
                "late success"
            )
            .is_err()
        );
        assert_eq!(fs::read(c.config.join("status.json")).unwrap(), before);
        assert!(!c.config.join("calls").exists()); // no subprocess, logout, or credential access
        sessions().lock().unwrap().remove(&root); // simulate runtime restart
        assert_eq!(status(&state).unwrap()["connection_state"], "disconnected");
        assert!(Binding::capture(state).is_err());
    }
    #[test]
    fn status_never_probes_credentials_and_restart_does_not_resume_login() {
        let (state, root, mut c) = fixture();
        c.consent = false;
        c.phase = "disconnected".into();
        c.account = None;
        save(&mut state.lock().unwrap(), &c).unwrap();
        status(&state).unwrap();
        assert!(!c.config.join("calls").exists());
        c.phase = "authenticating".into();
        save(&mut state.lock().unwrap(), &c).unwrap();
        sessions().lock().unwrap().remove(&root);
        assert_eq!(status(&state).unwrap()["connection_state"], "disconnected");
        assert!(!c.config.join("calls").exists());
    }
    #[test]
    fn duplicate_connect_clicks_cannot_launch() {
        let (state, root, c) = fixture();
        sessions().lock().unwrap().insert(
            root.clone(),
            Session {
                generation: c.generation.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
                code: None,
                browser_ready: false,
                running: true,
                read_progress: None,
            },
        );
        assert!(control(state.clone(), GitHubRequest::Connect).is_err());
        sessions().lock().unwrap().remove(&root);
    }

    #[test]
    fn default_config_must_remain_outside_registered_projects() {
        let (state, _, _) = fixture();
        let mut s = state.lock().unwrap();
        let path = config_path(&s).unwrap();
        assert!(path.ends_with(".config/gh"));
        let project = Project::new("fixture", path);
        s.projects.insert(project.root_key(), project);
        assert!(config_path(&s).is_err());
    }
    #[test]
    fn subprocess_cancellation_and_output_limits_are_enforced() {
        let (_, root, c) = fixture();
        let binary = root.join("sleep-tool");
        fs::write(&binary, "#!/bin/sh\nexec /bin/sleep 20\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            worker.store(true, Ordering::SeqCst);
        });
        let start = Instant::now();
        assert!(
            bounded_output_cancel(
                managed_command(&binary, &root, &c.config),
                30,
                1024,
                &cancel
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
        let mut cmd = Command::new("/usr/bin/printf");
        cmd.arg("too much output");
        assert!(bounded_output_cancel(cmd, 2, 4, &AtomicBool::new(false)).is_err());
    }
}
