//! Native-only connection state. No authentication output enters general events.
use super::*;
use serde::{Deserialize, Serialize};

const SETTING: &str = "github_connection_v1";
const DEVICE_URL: &str = "https://github.com/login/device";
const STORAGE_ERROR: &str = "GitHub CLI is not using secure Keychain storage. Ditch remains disconnected. Shared credentials were not deleted or rewritten; unlock your Mac login Keychain and retry browser sign-in. Review any plaintext shared hosts.yml credential using GitHub CLI. Browser login may have saved a plaintext credential if Keychain failed.";
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

// Overrides must be explicitly selected in the consent panel, never inherited.
fn config_path(s: &RuntimeState, selected: Option<String>) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("Mac home directory unavailable")?;
    let path = selected
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config/gh"));
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
        "installing" | "checking" | "authenticating"
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
        "browser_ready":job.is_some_and(|j|j.browser_ready),"busy":job.is_some_and(|j|j.running)}),
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
        GitHubRequest::AcceptConsent {
            config_path: selected,
        } => {
            let mut s = state.lock().unwrap();
            let mut c = read(&s)?;
            if sessions()
                .lock()
                .unwrap()
                .get(&root(&s))
                .is_some_and(|j| j.running)
            {
                return Err("Cancel the current attempt first".into());
            }
            c.config = config_path(&s, selected)?;
            c.consent = true;
            c.account = None;
            c.phase = "disconnected".into();
            c.generation = Uuid::new_v4().to_string();
            save(&mut s, &c)?;
            drop(s);
            return start(state, Job::Connect);
        }
        GitHubRequest::Connect => return start(state, Job::Connect),
        GitHubRequest::BrowserLogin => return start(state, Job::Login),
        GitHubRequest::Install => return start(state, Job::Install),
        GitHubRequest::UseAccount { generation } => return start(state, Job::Use(generation)),
        GitHubRequest::OpenBrowser | GitHubRequest::OpenRevocationHelp => {
            let help = matches!(request, GitHubRequest::OpenRevocationHelp);
            let s = state.lock().unwrap();
            let c = read(&s)?;
            let jobs = sessions().lock().unwrap();
            if !help
                && !jobs.get(&root(&s)).is_some_and(|j| {
                    j.generation == c.generation
                        && j.browser_ready
                        && !j.cancel.load(Ordering::SeqCst)
                })
            {
                return Err("No active GitHub device authorization".into());
            }
            // Fixed URL. No executable or URL emitted by a subprocess is trusted.
            let mut child = Command::new("/usr/bin/open")
                .arg(if help {
                    "https://cli.github.com/manual/gh_auth_logout"
                } else {
                    DEVICE_URL
                })
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| "Could not open GitHub in the browser")?;
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
        _ => return Err("Unsupported connection operation".into()),
    }
    status(&state)
}
enum Job {
    Connect,
    Login,
    Install,
    Use(String),
}
fn start(state: Arc<Mutex<RuntimeState>>, job: Job) -> Result<Value, String> {
    let (root, c, cancel) = {
        let mut s = state.lock().unwrap();
        let root = root(&s);
        let mut c = read(&s)?;
        let mut jobs = sessions().lock().unwrap();
        if jobs.get(&root).is_some_and(|j| j.running) {
            return Err("A GitHub operation is running; cancel or wait for it".into());
        }
        if !matches!(job, Job::Install) && !c.consent {
            return Err("Accept the shared GitHub CLI disclosure first".into());
        }
        if let Job::Use(ref generation) = job {
            if c.phase != "awaiting_account" || &c.generation != generation || c.account.is_none() {
                return Err("Account confirmation expired; reconnect".into());
            }
        }
        if let Some(previous) = jobs.get(&root) {
            previous.cancel.store(true, Ordering::SeqCst);
        }
        if matches!(job, Job::Install) && c.phase != "connected" {
            c.account = None;
        }
        c.generation = Uuid::new_v4().to_string();
        if !matches!(job, Job::Install | Job::Use(_)) {
            c.account = None;
        }
        c.phase = if matches!(job, Job::Install) || !root.join("active/gh").is_file() {
            "installing"
        } else {
            "checking"
        }
        .into();
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
            },
        );
        (root, c, cancel)
    };
    let background = state.clone();
    thread::spawn(move || {
        if let Err(error) = run_job(&background, &root, &c, &cancel, &job) {
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
    job: &Job,
) -> Result<(), String> {
    if matches!(job, Job::Install) || !root.join("active/gh").is_file() {
        let _install = INSTALL.lock().unwrap();
        check_cancel(cancel)?;
        install(root, cancel, &|message| {
            let _ = update(state, &c.generation, "installing", None, message);
        })?;
    }
    check_cancel(cancel)?;
    if matches!(job, Job::Install) {
        return update(
            state,
            &c.generation,
            if c.account.is_some() {
                "connected"
            } else {
                "disconnected"
            },
            c.account.clone(),
            "Managed CLI ready. Credentials were preserved.",
        );
    }
    let binary = root.join("active/gh");
    verify_version(&binary, root, &c.config, cancel)?;
    if let Job::Use(_) = job {
        let account = probe(&binary, root, &c.config, cancel)?
            .ok_or("No authenticated account; sign in again")?;
        if c.account.as_ref() != Some(&account) {
            return update(
                state,
                &c.generation,
                "account_changed",
                None,
                "The active account changed. Reconnect to confirm it.",
            );
        }
        return update(
            state,
            &c.generation,
            "connected",
            Some(account),
            "Connected using your shared GitHub CLI sign-in.",
        );
    }
    if !matches!(job, Job::Login) {
        if let Some(account) = probe(&binary, root, &c.config, cancel)? {
            return update(
                state,
                &c.generation,
                "awaiting_account",
                Some(account),
                "Confirm the active account, or sign in through your browser.",
            );
        }
    }
    update(
        state,
        &c.generation,
        "authenticating",
        None,
        "Complete GitHub authorization in your browser.",
    )?;
    login(root, c, cancel)?;
    check_cancel(cancel)?;
    let account = probe(&binary, root, &c.config, cancel)?
        .ok_or("Login ended without a verified GitHub account")?;
    update(
        state,
        &c.generation,
        "awaiting_account",
        Some(account),
        "Sign-in verified. Confirm this account to connect Ditch.",
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
        "[.hosts[\"github.com\"][]? | {state,active,host,login,tokenSource,gitProtocol}]",
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
        return Err(
            "GitHub sign-in is invalid or unavailable. Retry or choose browser sign-in.".into(),
        );
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
        });
        if session.generation != c.generation || session.cancel.load(Ordering::SeqCst) {
            session.generation = c.generation.clone();
            session.cancel = Arc::new(AtomicBool::new(false));
        }
        Ok(Self {
            state: state.clone(),
            generation: c.generation,
            account: c.account.ok_or("No bound identity")?,
            config: c.config,
            cancel: session.cancel.clone(),
            root,
        })
    }
    pub fn check_commit(&self, s: &RuntimeState) -> Result<(), String> {
        check_cancel(&self.cancel)?;
        let c = read(s)?;
        if c.phase != "connected"
            || c.generation != self.generation
            || c.account.as_ref() != Some(&self.account)
        {
            return Err("Connection changed; discarded this result".into());
        }
        Ok(())
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
                let _ = update(&self.state, &self.generation, "failed", None, &e);
                Err(e)
            }
        }
    }
}

#[derive(Default)]
struct LoginParser {
    text: String,
    escape: u8,
    git_answered: bool,
    browser_answered: bool,
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
    fn response(&mut self) -> Option<&'static [u8]> {
        if !self.git_answered
            && self
                .text
                .contains("Authenticate Git with your GitHub credentials?")
        {
            self.git_answered = true;
            return Some(b"n\r");
        }
        if !self.browser_answered
            && self
                .text
                .contains("Press Enter to open https://github.com/login/device in your browser...")
        {
            self.browser_answered = true;
            return Some(b"\r");
        }
        None
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
fn login(root: &Path, c: &Connection, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    let binary = root.join("active/gh");
    let mut cmd = managed_command(&binary, root, &c.config);
    cmd.args(["config", "get", "git_protocol", "--host", "github.com"]);
    let bytes = bounded_output_cancel(cmd, 10, 128, cancel)?;
    let protocol = match std::str::from_utf8(&bytes).unwrap_or("").trim() {
        "https" => "https",
        "ssh" => "ssh",
        _ => return Err("Unsupported Git protocol preference".into()),
    };
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|_| "Cannot open GitHub sign-in session")?;
    let mut command = CommandBuilder::new(&binary);
    command.env_clear();
    let environment = managed_command(&binary, root, &c.config);
    for (key, value) in environment.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    command.env_remove("GH_PROMPT_DISABLED");
    command.env("TERM", "xterm-256color");
    command.env("GH_BROWSER", "/usr/bin/true");
    command.cwd(root);
    command.args([
        "auth",
        "login",
        "--hostname",
        "github.com",
        "--web",
        "--git-protocol",
        protocol,
        "--skip-ssh-key",
        "--clipboard=false",
    ]);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| "Cannot read GitHub sign-in")?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|_| "Cannot control GitHub sign-in")?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|_| "Could not start GitHub sign-in")?;
    drop(pair.slave);
    let (sender, receiver) = mpsc::sync_channel(16);
    let reading = thread::spawn(move || {
        let mut bytes = [0; 2048];
        while let Ok(n) = reader.read(&mut bytes) {
            if n == 0 || sender.send(bytes[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut parser = LoginParser::default();
    let deadline = Instant::now() + Duration::from_secs(600);
    let result =
        (|| {
            loop {
                check_cancel(cancel)?;
                if Instant::now() >= deadline {
                    return Err("GitHub sign-in expired; reconnect".into());
                }
                if let Ok(bytes) = receiver.recv_timeout(Duration::from_millis(40)) {
                    parser.feed(&bytes)?;
                    while let Some(answer) = parser.response() {
                        writer
                            .write_all(answer)
                            .map_err(|_| "Cannot answer GitHub prompt")?;
                        writer.flush().map_err(|_| "Cannot answer GitHub prompt")?;
                    }
                    if let Some(job) = sessions().lock().unwrap().get_mut(root).filter(|j| {
                        j.generation == c.generation && !j.cancel.load(Ordering::SeqCst)
                    }) {
                        job.code = parser.code();
                        job.browser_ready = parser.browser_answered && job.code.is_some();
                    }
                }
                if let Some(status) = child
                    .try_wait()
                    .map_err(|_| "Cannot monitor GitHub sign-in")?
                {
                    while let Ok(bytes) = receiver.try_recv() {
                        parser.feed(&bytes)?;
                    }
                    return if status.success() {
                        Ok(())
                    } else {
                        Err("GitHub sign-in was denied or failed. Retry in your browser.".into())
                    };
                }
            }
        })();
    if let Some(pid) = child.process_id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    drop(receiver);
    drop(writer);
    drop(pair.master);
    let _ = reading.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirming_existing_account_connects_without_login_or_switch() {
        let (state, root, mut c) = fixture();
        c.phase = "awaiting_account".into();
        save(&mut state.lock().unwrap(), &c).unwrap();
        run_job(
            &state,
            &root,
            &c,
            &Arc::new(AtomicBool::new(false)),
            &Job::Use(c.generation.clone()),
        )
        .unwrap();
        assert_eq!(status(&state).unwrap()["connected"], true);
        let calls = fs::read_to_string(c.config.join("calls")).unwrap();
        assert!(!calls.contains("auth login"));
        assert!(!calls.contains("auth switch"));
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
    fn dedicated_pty_answers_git_setup_no_and_expected_browser_prompt() {
        let (state, root, c) = fixture();
        let binary = root.join("active/gh");
        fs::write(
            &binary,
            r#"#!/bin/sh
case "$1 $2" in
  "config get") printf 'https\n' ;;
  "auth login")
    printf 'Authenticate Git with your GitHub credentials? (Y/n) '
    read -r answer
    [ "$answer" = n ] || exit 70
    printf '! First copy your one-time code: ABCD-1234\n'
    printf 'Press Enter to open https://github.com/login/device in your browser... '
    read -r answer
    [ -z "$answer" ] || exit 71
    ;;
  *) exit 72 ;;
esac
"#,
        )
        .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        sessions().lock().unwrap().insert(
            root.clone(),
            Session {
                generation: c.generation.clone(),
                cancel: cancel.clone(),
                code: None,
                browser_ready: false,
                running: true,
            },
        );
        login(&root, &c, &cancel).unwrap();
        assert_eq!(read(&state.lock().unwrap()).unwrap().phase, "connected");
        sessions().lock().unwrap().remove(&root);
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
  "config get") printf 'https\n' ;;
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
    fn parser_handles_every_chunk_boundary_and_only_known_prompts() {
        let output=b"\x1b[32m? Authenticate Git with your GitHub credentials?\x1b[0m (Y/n)\n! First copy your one-time code: ABCD-1234\nPress Enter to open https://github.com/login/device in your browser... ";
        for split in 0..=output.len() {
            let mut parser = LoginParser::default();
            let mut responses = Vec::new();
            for chunk in [&output[..split], &output[split..]] {
                parser.feed(chunk).unwrap();
                while let Some(value) = parser.response() {
                    responses.push(value.to_vec());
                }
            }
            assert_eq!(responses, vec![b"n\r".to_vec(), b"\r".to_vec()]);
            assert_eq!(parser.code().as_deref(), Some("ABCD-1234"));
        }
        let mut parser = LoginParser::default();
        parser.feed(b"Upload a key? Press Enter to open https://evil.test/login/device in your browser...").unwrap();
        assert!(parser.response().is_none());
        assert!(
            parser
                .feed(b"Authentication credentials saved in plain text")
                .is_err()
        );
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
        assert!(calls.contains("{state,active,host,login,tokenSource,gitProtocol}"));
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
    fn no_credential_probe_before_consent_and_restart_does_not_resume_login() {
        let (state, root, mut c) = fixture();
        c.consent = false;
        c.phase = "disconnected".into();
        c.account = None;
        save(&mut state.lock().unwrap(), &c).unwrap();
        assert!(control(state.clone(), GitHubRequest::Connect).is_err());
        status(&state).unwrap();
        assert!(!c.config.join("calls").exists());
        c.phase = "authenticating".into();
        save(&mut state.lock().unwrap(), &c).unwrap();
        sessions().lock().unwrap().remove(&root);
        assert_eq!(status(&state).unwrap()["connection_state"], "disconnected");
        assert!(!c.config.join("calls").exists());
    }
    #[test]
    fn duplicate_clicks_and_stale_account_confirmation_cannot_launch() {
        let (state, root, c) = fixture();
        sessions().lock().unwrap().insert(
            root.clone(),
            Session {
                generation: c.generation.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
                code: None,
                browser_ready: false,
                running: true,
            },
        );
        assert!(control(state.clone(), GitHubRequest::BrowserLogin).is_err());
        sessions().lock().unwrap().remove(&root);
        assert!(
            control(
                state,
                GitHubRequest::UseAccount {
                    generation: "stale".into()
                }
            )
            .is_err()
        );
        assert!(!c.config.join("calls").exists());
    }
    #[test]
    fn config_resolver_rejects_project_paths_and_relative_overrides() {
        let (state, _, _) = fixture();
        let mut s = state.lock().unwrap();
        let project = Project::new("fixture", s.paths.data_dir.join("project"));
        fs::create_dir_all(&project.root).unwrap();
        s.projects.insert(project.root_key(), project.clone());
        assert!(config_path(&s, Some(project.root.join("gh").display().to_string())).is_err());
        assert!(config_path(&s, Some("../gh".into())).is_err());
        assert!(config_path(&s, None).unwrap().ends_with(".config/gh"));
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
