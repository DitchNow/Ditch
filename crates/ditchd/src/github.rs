//! Managed GitHub boundary. Credentials never enter IPC or the task database.
use super::*;
use ditch_core::github::*;
use ditch_core::{TaskActor, TaskDraft, TaskPriority};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use sha2::{Digest, Sha256};
#[path = "github_auth.rs"]
mod auth;

const NS: &str = "github";
const VERSION: &str = "2.101.0";
static OPERATION: Mutex<()> = Mutex::new(());
static INSTALL: Mutex<()> = Mutex::new(());
static RATE_LIMIT: Mutex<Option<Instant>> = Mutex::new(None);

/// Replaceable for deterministic tests; application IPC never exposes this method.
trait GitHubService {
    fn get(&self, path: &str) -> Result<Value, String>;
    fn progress(&self, _detail: &str) {}
    fn check_commit(&self, _state: &RuntimeState) -> Result<(), String> {
        Ok(())
    }
}
struct GhCli {
    root: PathBuf,
    binding: auth::Binding,
}
impl GitHubService for GhCli {
    fn progress(&self, detail: &str) {
        self.binding.progress(detail);
    }
    fn get(&self, path: &str) -> Result<Value, String> {
        if let Some(until) = *RATE_LIMIT.lock().unwrap() {
            if until > Instant::now() {
                return Err(format!(
                    "GitHub rate limit: retry in {} seconds",
                    until
                        .saturating_duration_since(Instant::now())
                        .as_secs()
                        .max(1)
                ));
            }
        }
        self.binding.verify()?;
        let binary = self.root.join("active/gh");
        let mut command = managed_command(&binary, &self.root, &self.binding.config);
        command.args([
            "api",
            "--hostname",
            "github.com",
            "--method",
            "GET",
            "--include",
            path,
        ]);
        let bytes = bounded_output_cancel(command, 30, 4 * 1024 * 1024, &self.binding.cancel)?;
        self.binding.verify()?;
        let (_, body) = http_parts(&bytes);
        serde_json::from_slice(body).map_err(|_| "GitHub returned invalid JSON".into())
    }
    fn check_commit(&self, state: &RuntimeState) -> Result<(), String> {
        self.binding.check_commit(state)
    }
}

fn managed_command(binary: &Path, root: &Path, config: &Path) -> Command {
    let mut command = Command::new(binary);
    command.env_clear();
    for key in [
        "HOME",
        "USER",
        "LOGNAME",
        "TMPDIR",
        "__CF_USER_TEXT_ENCODING",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("PATH", "/usr/bin:/bin")
        .env("GH_CONFIG_DIR", config)
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PAGER", "/bin/cat")
        .env("GH_PAGER", "/bin/cat")
        .env("GH_BROWSER", "/usr/bin/true")
        .env("GH_EDITOR", "/usr/bin/true")
        .env("LANG", "en_US.UTF-8")
        .current_dir(root)
        .stdin(Stdio::null());
    command
}

/// Drain both pipes with hard caps, bound time, and kill the subprocess group.
fn bounded_output_cancel(
    mut command: Command,
    seconds: u64,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    auth::check_cancel(cancel)?;
    command
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start the managed GitHub tool")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(65537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            break Err("GitHub operation cancelled");
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => break Err("GitHub tool timed out or could not be monitored"),
        }
    };
    // Also close inherited pipes if a child tried to leave descendants behind.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    let bytes = out
        .join()
        .map_err(|_| "GitHub output reader failed")?
        .map_err(|_| "GitHub output could not be read")?;
    let errors = err
        .join()
        .map_err(|_| "GitHub error reader failed")?
        .map_err(|_| "GitHub error could not be read")?;
    if bytes.len() > limit || errors.len() > 65536 {
        return Err("GitHub response exceeded its limit".into());
    }
    if !status?.success() {
        let (headers, _) = http_parts(&bytes);
        let message = String::from_utf8_lossy(&errors);
        let headers = headers.to_ascii_lowercase();
        if headers.contains(" 429 ")
            || headers.contains("retry-after:")
            || headers.contains("x-ratelimit-remaining: 0")
            || message.to_ascii_lowercase().contains("rate limit")
        {
            let delay = retry_delay(&headers);
            *RATE_LIMIT.lock().unwrap() = Some(Instant::now() + Duration::from_secs(delay));
            return Err(format!("GitHub rate limit: retry in {delay} seconds"));
        }
        if headers.contains(" 401 ") || message.contains("HTTP 401") {
            return Err("GitHub sign-in expired or was revoked; reconnect".into());
        }
        if headers.contains(" 403 ") || message.contains("HTTP 403") {
            return Err("GitHub denied access. Check organization/SSO authorization.".into());
        }
        if headers.contains(" 404 ") || message.contains("HTTP 404") {
            return Err(
                "Repository or issue was not found or is inaccessible to this account.".into(),
            );
        }
        return Err("GitHub request failed. Check your network and account access, then retry. Private diagnostic output was withheld.".into());
    }
    Ok(bytes)
}

fn http_parts(bytes: &[u8]) -> (String, &[u8]) {
    if !bytes.starts_with(b"HTTP/") {
        return (String::new(), bytes);
    }
    for separator in [b"\r\n\r\n".as_slice(), b"\n\n".as_slice()] {
        if let Some(end) = bytes.windows(separator.len()).position(|v| v == separator) {
            return (
                String::from_utf8_lossy(&bytes[..end]).into_owned(),
                &bytes[end + separator.len()..],
            );
        }
    }
    (String::new(), bytes)
}
fn retry_delay(headers: &str) -> u64 {
    let field = |name: &str| {
        headers
            .lines()
            .find_map(|line| line.strip_prefix(name)?.trim().parse::<u64>().ok())
    };
    field("retry-after:")
        .or_else(|| {
            field("x-ratelimit-reset:")
                .map(|epoch| epoch.saturating_sub(Utc::now().timestamp().max(0) as u64))
        })
        .unwrap_or(60)
        .clamp(1, 3600)
}

fn verify_archive(bytes: &[u8], expected: &str) -> Result<(), String> {
    if bytes.len() > 80 * 1024 * 1024 || format!("{:x}", Sha256::digest(bytes)) != expected {
        return Err("GitHub CLI checksum verification failed".into());
    }
    Ok(())
}
fn verify_macho(bytes: &[u8], architecture: &str) -> Result<(), String> {
    let cpu = match architecture {
        "aarch64" => 0x0100000cu32,
        "x86_64" => 0x01000007u32,
        _ => return Err("Unsupported Mac architecture".into()),
    };
    if bytes.len() < 8
        || bytes[..4] != [0xcf, 0xfa, 0xed, 0xfe]
        || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != cpu
    {
        return Err("GitHub executable architecture verification failed".into());
    }
    Ok(())
}

fn install(root: &Path, cancel: &AtomicBool, progress: &dyn Fn(&str)) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Managed GitHub CLI currently requires macOS".into());
    }
    let manifest: Value = serde_json::from_str(include_str!("github_manifest.json")).unwrap();
    let architecture = std::env::consts::ARCH;
    let artifact = manifest["artifacts"]
        .get(architecture)
        .ok_or("Unsupported Mac architecture")?;
    fs::create_dir_all(root).map_err(|_| "Cannot create managed tool directory")?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot protect managed tool directory")?;
    // A crashed download may leave a private staging directory. Only installer
    // UUID directories are disposable; active and previous versions are retained.
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name
                .to_str()
                .and_then(|v| v.strip_prefix("install-"))
                .is_some_and(|v| Uuid::parse_str(v).is_ok())
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
    let stage = root.join(format!("install-{}", Uuid::new_v4()));
    fs::create_dir(&stage).map_err(|_| "Cannot stage GitHub CLI")?;
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot protect staging directory")?;
    let result = (|| {
        auth::check_cancel(cancel)?;
        progress("Downloading the verified GitHub CLI release…");
        let response = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(90))
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(5))
            .build()
            .get(artifact["url"].as_str().unwrap())
            .call()
            .map_err(|_| "GitHub CLI download failed")?;
        let mut bytes = Vec::new();
        let mut reader = response.into_reader();
        let mut chunk = [0; 65536];
        loop {
            auth::check_cancel(cancel)?;
            let n = reader
                .read(&mut chunk)
                .map_err(|_| "GitHub CLI download interrupted")?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
            if bytes.len() > 80 * 1024 * 1024 {
                return Err("GitHub CLI download exceeded its limit".into());
            }
        }
        progress("Verifying download checksum and executable…");
        verify_archive(&bytes, artifact["sha256"].as_str().unwrap())?;
        let archive = stage.join("download.zip");
        fs::write(&archive, bytes).map_err(|_| "Could not save verified archive")?;
        // Extract only known regular file bytes to paths we create ourselves.
        // Never let an archive choose a destination or create a symlink.
        for (entry, destination, cap) in [
            ("bin/gh", "gh", 100 * 1024 * 1024),
            ("LICENSE", "LICENSE", 65536),
        ] {
            let mut command = managed_command(
                Path::new("/usr/bin/unzip"),
                root,
                &root.join("unused-config"),
            );
            command.args(["-p"]).arg(&archive).arg(format!(
                "{}/{}",
                artifact["directory"].as_str().unwrap(),
                entry
            ));
            let bytes = bounded_output_cancel(command, 30, cap, cancel)?;
            if destination == "gh" {
                verify_macho(&bytes, architecture)?;
            }
            fs::write(stage.join(destination), bytes)
                .map_err(|_| "Cannot write managed GitHub tool")?;
        }
        fs::set_permissions(stage.join("gh"), fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot set GitHub executable permissions")?;
        let mut command = managed_command(&stage.join("gh"), root, &root.join("unused-config"));
        command.arg("--version");
        let output = bounded_output_cancel(command, 10, 4096, cancel)?;
        if !String::from_utf8_lossy(&output).starts_with(&format!("gh version {VERSION} ")) {
            return Err("GitHub executable version verification failed".into());
        }
        let version = root.join(format!("gh-{VERSION}-{architecture}-{}", Uuid::new_v4()));
        auth::check_cancel(cancel)?;
        progress("Activating the verified GitHub CLI…");
        fs::remove_file(&archive).map_err(|_| "Cannot finish staging")?;
        fs::rename(&stage, &version).map_err(|_| "Cannot activate GitHub CLI")?;
        let next = root.join("active.next");
        let _ = fs::remove_file(&next);
        std::os::unix::fs::symlink(&version, &next)
            .map_err(|_| "Cannot stage active GitHub link")?;
        auth::check_cancel(cancel)?;
        fs::rename(&next, root.join("active"))
            .map_err(|_| "Cannot atomically activate GitHub CLI")?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

fn initialize(store: &DitchStore) -> Result<(), String> {
    store.with_extension_connection(NS, |c| {
        c.execute_batch("CREATE TABLE IF NOT EXISTS github_links(project_id TEXT NOT NULL REFERENCES projects(id),repository_id TEXT NOT NULL,json TEXT NOT NULL,PRIMARY KEY(project_id,repository_id));
        CREATE TABLE IF NOT EXISTS github_sources(project_id TEXT NOT NULL REFERENCES projects(id),host TEXT NOT NULL,issue_id TEXT NOT NULL,repository_id TEXT NOT NULL,number TEXT NOT NULL,task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),snapshot TEXT NOT NULL,PRIMARY KEY(project_id,host,issue_id));")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS github_import_receipts(request_id TEXT PRIMARY KEY, fingerprint TEXT NOT NULL, result TEXT NOT NULL);")?;
        Ok(())
    }).map_err(|e| e.to_string())
}
fn repository(s: &RuntimeState, project: ProjectId, repo: u64) -> Result<GitHubRepository, String> {
    let raw: Option<String> = s
        .store
        .with_extension_connection(NS, |c| {
            Ok(c.query_row(
                "SELECT json FROM github_links WHERE project_id=?1 AND repository_id=?2",
                params![project.0.to_string(), repo.to_string()],
                |r| r.get(0),
            )
            .optional()?)
        })
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&raw.ok_or("Link this repository to the selected project first")?)
        .map_err(|_| "Invalid saved repository".into())
}
fn verified_repository(
    state: &Arc<Mutex<RuntimeState>>,
    service: &dyn GitHubService,
    project: ProjectId,
    id: u64,
) -> Result<GitHubRepository, String> {
    let previous = repository(&state.lock().unwrap(), project, id)?;
    // Stable repository identity follows renames without guessing from a remote URL.
    let repo: GitHubRepository =
        serde_json::from_value(service.get(&format!("repositories/{id}"))?)
            .map_err(|_| "Invalid repository response")?;
    if repo.id != id || !repo.has_issues {
        return Err("Repository is unavailable or issues are disabled".into());
    }
    repository_name(&repo.full_name)?;
    let s = state.lock().unwrap();
    service.check_commit(&s)?;
    if repo != previous {
        s.store
            .with_extension_connection(NS, |c| {
                c.execute(
                    "UPDATE github_links SET json=?3 WHERE project_id=?1 AND repository_id=?2",
                    params![
                        project.0.to_string(),
                        id.to_string(),
                        serde_json::to_string(&repo).unwrap()
                    ],
                )?;
                Ok(())
            })
            .map_err(|e| e.to_string())?;
    }
    Ok(repo)
}
fn import_issues(
    s: &mut RuntimeState,
    project: ProjectId,
    repo: &GitHubRepository,
    issues: Vec<GitHubIssue>,
    request_id: Option<String>,
) -> Result<Value, String> {
    if !s
        .projects
        .values()
        .any(|p| p.id == project && p.archived_at.is_none())
    {
        return Err("Project is unavailable".into());
    }
    let mut identity: Vec<_> = issues.iter().map(|i| (i.id, i.number)).collect();
    identity.sort_unstable();
    identity.dedup();
    let fingerprint = serde_json::to_string(&(project, repo.id, &identity)).unwrap();
    let receipt = match request_id {
        Some(id) => Uuid::parse_str(&id)
            .map_err(|_| "Invalid import request ID")?
            .to_string(),
        None => format!("legacy-{:x}", Sha256::digest(fingerprint.as_bytes())),
    };
    let prior: Option<(String, String)> = s
        .store
        .with_extension_connection(NS, |c| {
            Ok(c.query_row(
                "SELECT fingerprint,result FROM github_import_receipts WHERE request_id=?1",
                [&receipt],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
        .map_err(|e| e.to_string())?;
    if let Some((saved, result)) = prior {
        if saved != fingerprint {
            return Err("Import request ID was reused for different issues".into());
        }
        return serde_json::from_str(&result).map_err(|_| "Invalid import receipt".into());
    }
    let mut changes = Vec::new();
    let mut sources = Vec::new();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for issue in issues {
        if !seen.insert(issue.id) {
            continue;
        }
        let prior: Option<String> = s.store.with_extension_connection(NS, |c| {
            Ok(c.query_row("SELECT task_id FROM github_sources WHERE project_id=?1 AND host='github.com' AND issue_id=?2",
                params![project.0.to_string(),issue.id.to_string()], |r| r.get(0)).optional()?)
        }).map_err(|e| e.to_string())?;
        if let Some(id) = prior {
            let task_id =
                TaskId(Uuid::parse_str(&id).map_err(|_| "Invalid imported task identity")?);
            let mut task = s
                .tasks
                .get(&task_id)
                .cloned()
                .ok_or("Imported task is unavailable")?;
            if task.github_source.as_ref().is_none_or(|source| {
                source.repository_id != repo.id || source.number != issue.number
            }) {
                task.github_source = Some(issue_source(repo, &issue));
                let audit = task.audit(
                    TaskActor::User,
                    "github_relink",
                    Some(task.column()),
                    Some(
                        "Explicit import reconciled the transferred issue by stable identity"
                            .into(),
                    ),
                );
                sources.push((task.id, issue));
                changes.push((task, audit));
            }
            ids.push(id);
            continue;
        }
        let mut task = new_backlog_task(
            project,
            TaskDraft {
                title: issue.title.clone(),
                description: issue.body.clone().unwrap_or_default(),
                skills: vec![],
                acceptance_criteria: vec![],
                priority: TaskPriority::Normal,
            },
            s.tasks
                .values()
                .map(|t| t.order_key)
                .max()
                .unwrap_or(0)
                .saturating_add((changes.len() as i64 + 1) * 1024),
            s.projects
                .values()
                .any(|p| p.id == project && p.is_remote()),
        )
        .map_err(|e| e.to_string())?;
        // Import is explicit scope admission, not execution or Ditch acceptance.
        // A remotely closed issue remains Backlog with its remote state badge.
        task.github_source = Some(issue_source(repo, &issue));
        ids.push(task.id.0.to_string());
        sources.push((task.id, issue));
        let audit = task.audit(
            TaskActor::User,
            "github_import",
            None,
            Some(
                "Selected issue admitted to Backlog; acceptance criteria still required before Run"
                    .into(),
            ),
        );
        changes.push((task, audit));
    }
    let result = json!({"task_ids":ids});
    s.store
        .save_task_group(NS, &changes, |tx| {
            for (id, issue) in &sources {
                tx.execute(
                    "INSERT INTO github_sources VALUES(?1,'github.com',?2,?3,?4,?5,?6) ON CONFLICT(project_id,host,issue_id) DO UPDATE SET repository_id=excluded.repository_id,number=excluded.number,snapshot=excluded.snapshot",
                    params![
                        project.0.to_string(),
                        issue.id.to_string(),
                        repo.id.to_string(),
                        issue.number.to_string(),
                        id.0.to_string(),
                        serde_json::to_string(issue).unwrap()
                    ],
                )?;
            }
            tx.execute(
                "INSERT INTO github_import_receipts VALUES(?1,?2,?3)",
                params![
                    receipt,
                    fingerprint,
                    serde_json::to_string(&result).unwrap()
                ],
            )?;
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    s.publish_tasks(changes);
    Ok(result)
}

fn issue_source(repo: &GitHubRepository, issue: &GitHubIssue) -> GitHubTaskSource {
    GitHubTaskSource {
        repository_id: repo.id,
        issue_id: issue.id,
        repository: repo.full_name.clone(),
        number: issue.number,
        url: issue.html_url.clone(),
        state: issue.state.clone(),
        last_synced_at: Utc::now().to_rfc3339(),
    }
}
fn imported_ids(s: &RuntimeState, project: ProjectId, repository_id: u64) -> Result<Value, String> {
    s.store.with_extension_connection(NS, |c| {
        let mut q = c.prepare("SELECT issue_id,task_id FROM github_sources WHERE project_id=?1 AND host='github.com' AND repository_id=?2")?;
        let pairs = q.query_map(params![project.0.to_string(),repository_id.to_string()], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
        Ok(Value::Object(pairs.into_iter().map(|(id,task)|(id,Value::String(task))).collect()))
    }).map_err(|e|e.to_string())
}

pub fn handle(state: Arc<Mutex<RuntimeState>>, request: GitHubRequest) -> ServerResponse {
    match handle_inner(state, request) {
        Ok(value) => ServerResponse::GitHub(value),
        Err(e) => protocol_error("github_unavailable", e),
    }
}
fn handle_inner(state: Arc<Mutex<RuntimeState>>, request: GitHubRequest) -> Result<Value, String> {
    let root = {
        let s = state.lock().unwrap();
        if s.remote_runtime {
            return Err("GitHub account management is available on the Mac only".into());
        }
        initialize(&s.store)?;
        s.paths.data_dir.join("tools/github")
    };
    if matches!(
        request,
        GitHubRequest::Status
            | GitHubRequest::Connect
            | GitHubRequest::OpenBrowser
            | GitHubRequest::Cancel
            | GitHubRequest::CancelRead
            | GitHubRequest::Disconnect
    ) {
        return auth::control(state, request);
    }
    let _operation = OPERATION
        .try_lock()
        .map_err(|_| "Another GitHub read is in progress")?;
    let binding = auth::Binding::capture(state.clone())?;
    let service = GhCli {
        root: root.clone(),
        binding,
    };
    handle_service(state, request, &service, &root)
}
fn handle_service(
    state: Arc<Mutex<RuntimeState>>,
    request: GitHubRequest,
    service: &dyn GitHubService,
    _root: &Path,
) -> Result<Value, String> {
    match request {
        GitHubRequest::Status
        | GitHubRequest::Connect
        | GitHubRequest::OpenBrowser
        | GitHubRequest::Cancel
        | GitHubRequest::CancelRead
        | GitHubRequest::Disconnect => auth::control(state, request),
        GitHubRequest::Links { project_id } => {
            let s = state.lock().unwrap();
            service.check_commit(&s)?;
            let values = s.store.with_extension_connection(NS, |c| {
                let mut q = c.prepare("SELECT project_id,json FROM github_links WHERE (?1 IS NULL OR project_id=?1)")?;
                Ok(q.query_map([project_id.map(|id| id.0.to_string())], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?)
            }).map_err(|e| e.to_string())?;
            Ok(
                json!({"repositories":values.into_iter().map(|(id,raw)| json!({"project_id":id,"repository":serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null)})).collect::<Vec<_>>()}),
            )
        }
        GitHubRequest::Repositories { page } => {
            valid_page(page)?;
            let repositories = service.get(&format!("user/repos?affiliation=owner,collaborator,organization_member&per_page=50&page={page}"))?;
            let rows = repositories
                .as_array()
                .ok_or("Invalid repositories response")?;
            service.check_commit(&state.lock().unwrap())?;
            Ok(json!({"repositories":repositories,"page":page,"has_more":rows.len()==50}))
        }
        GitHubRequest::Link {
            project_id,
            repository: input,
        } => {
            check_registered(&state, project_id)?;
            let name = repository_name(&input)?;
            let repo: GitHubRepository =
                serde_json::from_value(service.get(&format!("repos/{name}"))?)
                    .map_err(|_| "Invalid repository response")?;
            if !repo.has_issues {
                return Err("Issues are disabled for this repository".into());
            }
            let s = state.lock().unwrap();
            service.check_commit(&s)?;
            s.store.with_extension_connection(NS, |c| {
                c.execute("INSERT INTO github_links VALUES(?1,?2,?3) ON CONFLICT(project_id,repository_id) DO UPDATE SET json=excluded.json",
                    params![project_id.0.to_string(),repo.id.to_string(),serde_json::to_string(&repo).unwrap()])?; Ok(())
            }).map_err(|e| e.to_string())?;
            Ok(json!(repo))
        }
        GitHubRequest::Issues {
            project_id,
            repository_id,
            state: filter,
            page,
        } => {
            check_registered(&state, project_id)?;
            valid_page(page)?;
            let repo = verified_repository(&state, service, project_id, repository_id)?;
            let name = repository_name(&repo.full_name)?;
            let filter = match filter {
                IssueFilter::Open => "open",
                IssueFilter::Closed => "closed",
                IssueFilter::All => "all",
            };
            let raw = service.get(&format!(
                "repos/{name}/issues?state={filter}&per_page=50&page={page}"
            ))?;
            let rows = raw.as_array().ok_or("Invalid issue response")?;
            let issues = rows
                .iter()
                .filter(|v| v.get("pull_request").is_none())
                .map(|v| serde_json::from_value::<GitHubIssue>(v.clone()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "Invalid issue fields")?;
            let s = state.lock().unwrap();
            service.check_commit(&s)?;
            let imported = imported_ids(&s, project_id, repository_id)?;
            Ok(
                json!({"issues":issues,"imported":imported,"page":page,"has_more":rows.len()==50,"last_synced_at":Utc::now()}),
            )
        }
        GitHubRequest::Import {
            project_id,
            repository_id,
            numbers,
            request_id,
        } => {
            check_registered(&state, project_id)?;
            if numbers.is_empty() || numbers.len() > 50 || numbers.contains(&0) {
                return Err("Select 1–50 issues".into());
            }
            let repo = verified_repository(&state, service, project_id, repository_id)?;
            let name = repository_name(&repo.full_name)?;
            let mut issues = Vec::new();
            let total = numbers.len();
            for (index, number) in numbers.into_iter().enumerate() {
                service.progress(&format!("Importing issue {} of {total}…", index + 1));
                let raw = service.get(&format!("repos/{name}/issues/{number}"))?;
                if raw.get("pull_request").is_some() {
                    return Err("Pull requests cannot be imported as issues".into());
                }
                let issue: GitHubIssue =
                    serde_json::from_value(raw).map_err(|_| "Invalid issue response")?;
                let expected = format!("https://github.com/{name}/issues/{number}");
                if issue.number != number || !issue.html_url.eq_ignore_ascii_case(&expected) {
                    return Err("An issue moved. Link its destination repository and select it there before importing.".into());
                }
                issues.push(issue);
            }
            service.progress("Saving tasks to Backlog…");
            let mut s = state.lock().unwrap();
            service.check_commit(&s)?;
            import_issues(&mut s, project_id, &repo, issues, request_id)
        }
        GitHubRequest::Comments {
            project_id,
            repository_id,
            number,
            page,
        } => {
            check_registered(&state, project_id)?;
            valid_page(page)?;
            if number == 0 {
                return Err("Invalid issue number".into());
            }
            let repo = verified_repository(&state, service, project_id, repository_id)?;
            let name = repository_name(&repo.full_name)?;
            let comments = service.get(&format!(
                "repos/{name}/issues/{number}/comments?per_page=50&page={page}"
            ))?;
            let rows = comments.as_array().ok_or("Invalid comments response")?;
            let expected =
                format!("https://github.com/{name}/issues/{number}#").to_ascii_lowercase();
            if rows.iter().any(|row| {
                !row["html_url"]
                    .as_str()
                    .is_some_and(|url| url.to_ascii_lowercase().starts_with(&expected))
            }) {
                return Err("The issue moved or its comments could not be verified in this linked repository.".into());
            }
            service.check_commit(&state.lock().unwrap())?;
            Ok(json!({"comments":comments,"has_more":rows.len()==50,"page":page}))
        }
        GitHubRequest::Refresh { task_id } => {
            let (project, source) = {
                let s = state.lock().unwrap();
                let task = s.tasks.get(&task_id).ok_or("Task not found")?;
                (
                    task.project_id,
                    task.github_source
                        .clone()
                        .ok_or("Task has no GitHub source")?,
                )
            };
            check_registered(&state, project)?;
            let repo = verified_repository(&state, service, project, source.repository_id)?;
            let name = repository_name(&repo.full_name)?;
            let raw = service.get(&format!("repos/{name}/issues/{}", source.number))?;
            let issue: GitHubIssue =
                serde_json::from_value(raw).map_err(|_| "Invalid issue response")?;
            if issue.id != source.issue_id {
                return Err("Issue identity changed; relink explicitly".into());
            }
            let expected_url = format!(
                "https://github.com/{}/issues/{}",
                repo.full_name, source.number
            );
            if issue.number != source.number || !issue.html_url.eq_ignore_ascii_case(&expected_url)
            {
                return Err("This issue moved. Link its destination repository and import it again to update the source without duplicating local work.".into());
            }
            let mut s = state.lock().unwrap();
            service.check_commit(&s)?;
            let mut task = s.tasks.get(&task_id).cloned().ok_or("Task was removed")?;
            let source = task.github_source.as_mut().ok_or("Source was removed")?;
            source.state = issue.state.clone();
            source.url = issue.html_url.clone();
            source.repository = repo.full_name.clone();
            source.last_synced_at = Utc::now().to_rfc3339();
            // Remote metadata does not change the execution scope or local revision.
            let audit = task.audit(TaskActor::User, "github_refresh", Some(task.column()), None);
            let changes = vec![(task, audit)];
            s.store
                .save_task_group(NS, &changes, |tx| {
                    tx.execute(
                        "UPDATE github_sources SET snapshot=?2 WHERE task_id=?1",
                        params![
                            task_id.0.to_string(),
                            serde_json::to_string(&issue).unwrap()
                        ],
                    )?;
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
            s.publish_tasks(changes);
            Ok(json!({"refreshed":true}))
        }
    }
}
fn valid_page(page: u32) -> Result<(), String> {
    if (1..=10000).contains(&page) {
        Ok(())
    } else {
        Err("Invalid page".into())
    }
}
fn check_registered(state: &Arc<Mutex<RuntimeState>>, id: ProjectId) -> Result<(), String> {
    if state
        .lock()
        .unwrap()
        .projects
        .values()
        .any(|p| p.id == id && p.archived_at.is_none())
    {
        Ok(())
    } else {
        Err("Project is unavailable".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_transfer_relink_preserves_existing_task_identity_and_revision() {
        let (state, project, mut fake) = fixture();
        let original = call(
            &state,
            &fake,
            GitHubRequest::Import {
                project_id: project.id,
                repository_id: 42,
                numbers: vec![7],
                request_id: None,
            },
        )
        .unwrap();
        let before = {
            let mut s = state.lock().unwrap();
            let task = s.tasks.values_mut().next().unwrap();
            task.title = "Local edit".into();
            task.state = ditch_core::TaskState::Ready;
            task.touch();
            task.clone()
        };
        let moved_repo = json!({"id":43,"full_name":"owner/moved","html_url":"https://github.com/owner/moved","has_issues":true});
        fake.0
            .insert("repos/owner/moved".into(), moved_repo.clone());
        fake.0.insert("repositories/43".into(), moved_repo);
        let mut issue = fake.0["repos/owner/repo/issues/7"].clone();
        issue["number"] = json!(19);
        issue["html_url"] = json!("https://github.com/owner/moved/issues/19");
        fake.0
            .insert("repos/owner/repo/issues/7".into(), issue.clone());
        assert!(call(&state, &fake, GitHubRequest::Refresh { task_id: before.id }).is_err());
        fake.0.insert("repos/owner/moved/issues/19".into(), issue);
        call(
            &state,
            &fake,
            GitHubRequest::Link {
                project_id: project.id,
                repository: "owner/moved".into(),
            },
        )
        .unwrap();
        let result = call(
            &state,
            &fake,
            GitHubRequest::Import {
                project_id: project.id,
                repository_id: 43,
                numbers: vec![19],
                request_id: None,
            },
        )
        .unwrap();
        assert_eq!(result, original);
        let s = state.lock().unwrap();
        let task = &s.tasks[&before.id];
        assert_eq!(s.tasks.len(), 1);
        assert_eq!(task.title, before.title);
        assert_eq!(task.revision, before.revision);
        assert_eq!(task.state, before.state);
        assert_eq!(task.github_source.as_ref().unwrap().repository_id, 43);
        assert_eq!(task.github_source.as_ref().unwrap().number, 19);
    }
    #[test]
    fn invalid_archives_and_wrong_architectures_fail_verification() {
        assert!(verify_archive(b"malicious payload", &"0".repeat(64)).is_err());
        for (arch, cpu) in [("aarch64", 0x0100000cu32), ("x86_64", 0x01000007u32)] {
            let mut header = vec![0xcf, 0xfa, 0xed, 0xfe];
            header.extend_from_slice(&cpu.to_le_bytes());
            verify_macho(&header, arch).unwrap();
            assert!(
                verify_macho(
                    &header,
                    if arch == "aarch64" {
                        "x86_64"
                    } else {
                        "aarch64"
                    }
                )
                .is_err()
            );
        }
        assert!(verify_macho(b"#!/bin/sh", "aarch64").is_err());
        assert!(verify_macho(&[], "unsupported").is_err());
    }

    #[test]
    fn import_receipts_reject_reuse_and_project_identity_is_independent() {
        let (state, project, mut fake) = fixture();
        let receipt = Uuid::new_v4().to_string();
        let first = GitHubRequest::Import {
            project_id: project.id,
            repository_id: 42,
            numbers: vec![7],
            request_id: Some(receipt.clone()),
        };
        let result = call(&state, &fake, first.clone()).unwrap();
        assert_eq!(result, call(&state, &fake, first).unwrap());
        let mut other = fake.0["repos/owner/repo/issues/7"].clone();
        other["id"] = json!(80);
        other["number"] = json!(8);
        other["html_url"] = json!("https://github.com/owner/repo/issues/8");
        fake.0.insert("repos/owner/repo/issues/8".into(), other);
        assert!(
            call(
                &state,
                &fake,
                GitHubRequest::Import {
                    project_id: project.id,
                    repository_id: 42,
                    numbers: vec![8],
                    request_id: Some(receipt)
                }
            )
            .is_err()
        );
        let second = {
            let mut s = state.lock().unwrap();
            let p = Project::new("Second", s.paths.data_dir.join("second"));
            s.store.upsert_project(&p).unwrap();
            s.projects.insert(p.root_key(), p.clone());
            p
        };
        call(
            &state,
            &fake,
            GitHubRequest::Link {
                project_id: second.id,
                repository: "owner/repo".into(),
            },
        )
        .unwrap();
        let result2 = call(
            &state,
            &fake,
            GitHubRequest::Import {
                project_id: second.id,
                repository_id: 42,
                numbers: vec![7],
                request_id: None,
            },
        )
        .unwrap();
        assert_ne!(result, result2);
        assert_eq!(state.lock().unwrap().tasks.len(), 2);
    }
    #[test]
    fn pagination_beyond_one_hundred_and_repository_rename_preserve_identity() {
        let (state, project, mut fake) = fixture();
        let mut renamed = fake.0["repositories/42"].clone();
        renamed["full_name"] = json!("org/renamed");
        fake.0.insert("repositories/42".into(), renamed);
        for page in 1..=3 {
            fake.0.insert(
                format!("repos/org/renamed/issues?state=open&per_page=50&page={page}"),
                json!(
                    (0..50)
                        .map(|n| {
                            let mut i = fake.0["repos/owner/repo/issues/7"].clone();
                            i["id"] = json!(page * 50 + n);
                            i["number"] = json!(page * 50 + n);
                            i
                        })
                        .collect::<Vec<_>>()
                ),
            );
        }
        let mut total = 0;
        for page in 1..=3 {
            let result = call(
                &state,
                &fake,
                GitHubRequest::Issues {
                    project_id: project.id,
                    repository_id: 42,
                    state: IssueFilter::Open,
                    page,
                },
            )
            .unwrap();
            total += result["issues"].as_array().unwrap().len();
        }
        assert_eq!(total, 150);
        assert_eq!(
            repository(&state.lock().unwrap(), project.id, 42)
                .unwrap()
                .full_name,
            "org/renamed"
        );
    }
    #[test]
    fn http_headers_and_backoff_are_bounded() {
        let (_, body) = http_parts(b"HTTP/2.0 200 OK\r\nX-Test: true\r\n\r\n[{\"id\":1}]");
        assert_eq!(
            serde_json::from_slice::<Value>(body).unwrap(),
            json!([{"id":1}])
        );
        assert_eq!(retry_delay("retry-after: 120"), 120);
        assert_eq!(retry_delay("retry-after: 9999999"), 3600);
        assert_eq!(retry_delay("missing"), 60);
    }
    #[test]
    #[ignore = "Explicit network smoke test: official release, isolated app directory, no authentication"]
    fn official_managed_install_smoke() {
        let (state, _, _) = fixture();
        let root = state
            .lock()
            .unwrap()
            .paths
            .data_dir
            .join("official-installer");
        let _lock = INSTALL.lock().unwrap();
        install(&root, &AtomicBool::new(false), &|_| {}).unwrap();
        let previous = fs::read_link(root.join("active")).unwrap();
        assert!(previous.join("LICENSE").is_file());
        assert!(install(&root, &AtomicBool::new(true), &|_| {}).is_err());
        assert_eq!(fs::read_link(root.join("active")).unwrap(), previous);
        install(&root, &AtomicBool::new(false), &|_| {}).unwrap();
        assert_ne!(fs::read_link(root.join("active")).unwrap(), previous);
        assert!(previous.join("gh").is_file());
    }

    #[derive(Clone)]
    struct Fake(HashMap<String, Value>);
    impl GitHubService for Fake {
        fn get(&self, path: &str) -> Result<Value, String> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| format!("Unexpected GET: {path}"))
        }
    }
    fn fixture() -> (Arc<Mutex<RuntimeState>>, Project, Fake) {
        let mut s = super::super::tests::test_runtime();
        initialize(&s.store).unwrap();
        let project = Project::new("GitHub fixture", s.paths.data_dir.join("repo"));
        s.store.upsert_project(&project).unwrap();
        s.projects.insert(project.root_key(), project.clone());
        let mut fake = Fake(HashMap::from([
            (
                "repos/owner/repo".into(),
                json!({"id":42,"full_name":"owner/repo","html_url":"https://github.com/owner/repo","has_issues":true}),
            ),
            (
                "repos/owner/repo/issues/7".into(),
                json!({"id":70,"number":7,"title":"Original title","body":"Original body","state":"closed","state_reason":"completed","html_url":"https://github.com/owner/repo/issues/7","updated_at":"2026-09-28T00:00:00Z"}),
            ),
        ]));
        fake.0
            .insert("repositories/42".into(), fake.0["repos/owner/repo"].clone());
        let state = Arc::new(Mutex::new(s));
        call(
            &state,
            &fake,
            GitHubRequest::Link {
                project_id: project.id,
                repository: "owner/repo".into(),
            },
        )
        .unwrap();
        (state, project, fake)
    }
    fn call(
        state: &Arc<Mutex<RuntimeState>>,
        fake: &Fake,
        request: GitHubRequest,
    ) -> Result<Value, String> {
        handle_service(state.clone(), request, fake, Path::new("unused-test-tool"))
    }
    #[test]
    fn selected_import_is_idempotent_and_refresh_preserves_local_work() {
        let (state, project, mut fake) = fixture();
        let request = GitHubRequest::Import {
            project_id: project.id,
            repository_id: 42,
            numbers: vec![7, 7],
            request_id: None,
        };
        let first = call(&state, &fake, request.clone()).unwrap();
        assert_eq!(first, call(&state, &fake, request).unwrap());
        let id = {
            let mut s = state.lock().unwrap();
            assert_eq!(s.tasks.len(), 1);
            assert!(s.agents.is_empty());
            let t = s.tasks.values_mut().next().unwrap();
            assert_eq!(t.column(), TaskColumn::Backlog);
            assert_eq!(t.github_source.as_ref().unwrap().state, "closed");
            t.title = "User's local title".into();
            t.acceptance_criteria = vec!["User criterion".into()];
            t.touch();
            t.id
        };
        let original = state.lock().unwrap().tasks[&id].clone();
        let issue = fake.0.get_mut("repos/owner/repo/issues/7").unwrap();
        issue["state"] = json!("open");
        issue["title"] = json!("Changed remotely");
        call(&state, &fake, GitHubRequest::Refresh { task_id: id }).unwrap();
        let s = state.lock().unwrap();
        let refreshed = &s.tasks[&id];
        assert_eq!(refreshed.title, original.title);
        assert_eq!(refreshed.acceptance_criteria, original.acceptance_criteria);
        assert_eq!(refreshed.revision, original.revision);
        assert_eq!(refreshed.column(), TaskColumn::Backlog);
        assert_eq!(refreshed.github_source.as_ref().unwrap().state, "open");
    }
    #[test]
    fn concurrent_imports_share_one_task_and_disconnect_retains_local_copy() {
        let (state, project, fake) = fixture();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let jobs: Vec<_> = (0..2)
            .map(|_| {
                let state = state.clone();
                let fake = fake.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    call(
                        &state,
                        &fake,
                        GitHubRequest::Import {
                            project_id: project.id,
                            repository_id: 42,
                            numbers: vec![7],
                            request_id: Some(Uuid::new_v4().to_string()),
                        },
                    )
                    .unwrap()
                })
            })
            .collect();
        let results: Vec<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
        assert_eq!(results[0], results[1]);
        auth::control(state.clone(), GitHubRequest::Disconnect).unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.tasks.len(), 1);
        assert!(s.tasks.values().next().unwrap().github_source.is_some());
        assert_eq!(s.store.load_tasks().unwrap().len(), 1);
    }
    #[test]
    fn imported_tasks_require_criteria_before_native_start() {
        let (state, project, fake) = fixture();
        call(
            &state,
            &fake,
            GitHubRequest::Import {
                project_id: project.id,
                repository_id: 42,
                numbers: vec![7],
                request_id: None,
            },
        )
        .unwrap();
        let task = state.lock().unwrap().tasks.values().next().unwrap().clone();
        let response = handle_task_request(
            state.clone(),
            TaskRequest {
                project_id: Some(project.id),
                request_id: Uuid::new_v4(),
                operation: TaskOperation::Start {
                    task_id: task.id,
                    expected_revision: task.revision,
                    execution_profile: AgentExecutionProfile::default(),
                },
            },
        );
        let ServerResponse::TaskResponse(TaskResponse::Error(error)) = response else {
            panic!("Expected missing criteria rejection")
        };
        assert!(error.message.contains("acceptance criteria"));
        assert!(state.lock().unwrap().agents.is_empty());
    }
    #[test]
    fn pull_requests_and_partial_fetch_failures_cannot_create_tasks() {
        let (state, project, mut fake) = fixture();
        assert!(
            call(
                &state,
                &fake,
                GitHubRequest::Import {
                    project_id: project.id,
                    repository_id: 42,
                    numbers: vec![7, 8],
                    request_id: None,
                }
            )
            .is_err()
        );
        assert!(state.lock().unwrap().tasks.is_empty());
        fake.0.get_mut("repos/owner/repo/issues/7").unwrap()["pull_request"] =
            json!({"url":"https://github.com/owner/repo/pull/7"});
        assert!(
            call(
                &state,
                &fake,
                GitHubRequest::Import {
                    project_id: project.id,
                    repository_id: 42,
                    numbers: vec![7],
                    request_id: None,
                }
            )
            .is_err()
        );
        assert!(state.lock().unwrap().tasks.is_empty());
    }
    #[test]
    fn pagination_counts_pull_requests_but_does_not_show_them_as_issues() {
        let (state, project, mut fake) = fixture();
        let mut rows = vec![json!({"pull_request":{}}); 49];
        rows.push(fake.0["repos/owner/repo/issues/7"].clone());
        fake.0.insert(
            "repos/owner/repo/issues?state=all&per_page=50&page=1".into(),
            json!(rows),
        );
        let result = call(
            &state,
            &fake,
            GitHubRequest::Issues {
                project_id: project.id,
                repository_id: 42,
                state: IssueFilter::All,
                page: 1,
            },
        )
        .unwrap();
        assert_eq!(result["has_more"], true);
        assert_eq!(result["issues"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn inherited_credentials_and_execution_overrides_are_absent() {
        let command = managed_command(
            Path::new("/owned/gh"),
            Path::new("/owned"),
            Path::new("/shared/gh"),
        );
        let env: HashMap<_, _> = command
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v))
            .collect();
        for key in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "GH_DEBUG",
        ] {
            assert!(!env.contains_key(key));
        }
        assert_eq!(env["GH_CONFIG_DIR"].unwrap(), "/shared/gh");
        assert_eq!(env["GH_PAGER"].unwrap(), "/bin/cat");
    }
}

#[cfg(target_os = "macos")]
pub(super) fn native_peer(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of_val(&pid) as libc::socklen_t;
    // SAFETY: kernel writes at most the supplied pid_t-sized buffer.
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    } != 0
        || pid <= 0
    {
        return false;
    }
    if pid == std::process::id() as i32 {
        return true;
    }
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    let Some(bundle) = current
        .ancestors()
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
        .last()
    else {
        return false;
    };
    let mut buffer = vec![0u8; 4096];
    // SAFETY: libproc receives a live buffer and its actual capacity.
    let count = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if count <= 0 {
        return false;
    }
    let Some(end) = buffer.iter().position(|b| *b == 0) else {
        return false;
    };
    let Ok(path) = std::str::from_utf8(&buffer[..end]) else {
        return false;
    };
    Path::new(path).parent() == Some(bundle.join("Contents/MacOS").as_path())
}
#[cfg(not(target_os = "macos"))]
pub(super) fn native_peer(_stream: &std::os::unix::net::UnixStream) -> bool {
    false
}
