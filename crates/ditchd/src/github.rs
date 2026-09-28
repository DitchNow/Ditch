//! Managed GitHub boundary. Credentials never enter IPC or the task database.
use super::*;
use ditch_core::github::*;
use ditch_core::{Task, TaskActor, TaskDraft, TaskPriority, TaskState};
use rusqlite::{OptionalExtension, params};
use serde_json::json;
use sha2::{Digest, Sha256};

const NS: &str = "github";
const VERSION: &str = "2.101.0";
const COEXISTENCE: &str = "GitHub connection is unavailable: this GitHub CLI version shares macOS Keychain entries with other gh installations. An isolated credential arrangement must be verified before Ditch can connect or log out.";
static OPERATION: Mutex<()> = Mutex::new(());

/// Replaceable for deterministic tests; application IPC never exposes this method.
trait GitHubService {
    fn get(&self, path: &str) -> Result<Value, String>;
}
struct GhCli {
    root: PathBuf,
}
impl GitHubService for GhCli {
    fn get(&self, path: &str) -> Result<Value, String> {
        // Intentionally fail closed. GH_CONFIG_DIR does not isolate upstream's
        // "gh:github.com" Keychain service. No hidden opt-in bypass.
        verify_credential_isolation()?;
        let binary = self.root.join("active/gh");
        let mut command = managed_command(&binary, &self.root);
        command.args(["api", "--hostname", "github.com", "--method", "GET", path]);
        serde_json::from_slice(&bounded_output(command, 30, 4 * 1024 * 1024)?)
            .map_err(|_| "GitHub returned invalid JSON".into())
    }
}
fn verify_credential_isolation() -> Result<(), String> {
    Err(COEXISTENCE.into())
}

fn managed_command(binary: &Path, root: &Path) -> Command {
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
        .env("GH_CONFIG_DIR", root.join("config"))
        .env("GH_HOST", "github.com")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("PAGER", "/bin/cat")
        .env("GH_PAGER", "/bin/cat")
        .current_dir(root)
        .stdin(Stdio::null());
    command
}

/// Drain both pipes with hard caps, bound time, and kill the subprocess group.
fn bounded_output(mut command: Command, seconds: u64, limit: usize) -> Result<Vec<u8>, String> {
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
        return Err("GitHub request failed. Check account access or retry later; diagnostic output was withheld to protect credentials.".into());
    }
    Ok(bytes)
}

fn install(root: &Path) -> Result<(), String> {
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
    let stage = root.join(format!("install-{}", Uuid::new_v4()));
    fs::create_dir(&stage).map_err(|_| "Cannot stage GitHub CLI")?;
    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot protect staging directory")?;
    let result = (|| {
        let response = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(90))
            .build()
            .get(artifact["url"].as_str().unwrap())
            .call()
            .map_err(|_| "GitHub CLI download failed")?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(80 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "GitHub CLI download interrupted")?;
        if bytes.len() > 80 * 1024 * 1024
            || format!("{:x}", Sha256::digest(&bytes)) != artifact["sha256"].as_str().unwrap()
        {
            return Err("GitHub CLI checksum verification failed".into());
        }
        let archive = stage.join("download.zip");
        fs::write(&archive, bytes).map_err(|_| "Could not save verified archive")?;
        // Extract only known regular file bytes to paths we create ourselves.
        // Never let an archive choose a destination or create a symlink.
        for (entry, destination, cap) in [
            ("bin/gh", "gh", 100 * 1024 * 1024),
            ("LICENSE", "LICENSE", 65536),
        ] {
            let mut command = Command::new("/usr/bin/unzip");
            command.args(["-p"]).arg(&archive).arg(format!(
                "{}/{}",
                artifact["directory"].as_str().unwrap(),
                entry
            ));
            let bytes = bounded_output(command, 30, cap)?;
            if destination == "gh" {
                let cpu = if architecture == "aarch64" {
                    0x0100000cu32
                } else {
                    0x01000007u32
                };
                if bytes.len() < 8
                    || bytes[..4] != [0xcf, 0xfa, 0xed, 0xfe]
                    || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != cpu
                {
                    return Err("GitHub executable architecture verification failed".into());
                }
            }
            fs::write(stage.join(destination), bytes)
                .map_err(|_| "Cannot write managed GitHub tool")?;
        }
        fs::set_permissions(stage.join("gh"), fs::Permissions::from_mode(0o700))
            .map_err(|_| "Cannot set GitHub executable permissions")?;
        let mut command = managed_command(&stage.join("gh"), root);
        command.arg("--version");
        let output = bounded_output(command, 10, 4096)?;
        if !String::from_utf8_lossy(&output).starts_with(&format!("gh version {VERSION} ")) {
            return Err("GitHub executable version verification failed".into());
        }
        let version = root.join(format!("gh-{VERSION}-{architecture}-{}", Uuid::new_v4()));
        fs::remove_file(&archive).map_err(|_| "Cannot finish staging")?;
        fs::rename(&stage, &version).map_err(|_| "Cannot activate GitHub CLI")?;
        let next = root.join("active.next");
        let _ = fs::remove_file(&next);
        std::os::unix::fs::symlink(&version, &next)
            .map_err(|_| "Cannot stage active GitHub link")?;
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
fn import_issues(
    s: &mut RuntimeState,
    project: ProjectId,
    repo: &GitHubRepository,
    issues: Vec<GitHubIssue>,
) -> Result<Value, String> {
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
            ids.push(id);
            continue;
        }
        let mut task = Task::new(
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
        )
        .map_err(|e| e.to_string())?;
        // Import is explicit scope admission, not execution or Ditch acceptance.
        // A remotely closed issue remains Backlog with its remote state badge.
        task.state = TaskState::Backlog;
        task.remote_pending = s
            .projects
            .values()
            .any(|p| p.id == project && p.is_remote());
        task.github_source = Some(GitHubTaskSource {
            repository_id: repo.id,
            issue_id: issue.id,
            repository: repo.full_name.clone(),
            number: issue.number,
            url: issue.html_url.clone(),
            state: issue.state.clone(),
            last_synced_at: Utc::now().to_rfc3339(),
        });
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
    s.store
        .save_task_group(NS, &changes, |tx| {
            for (id, issue) in &sources {
                tx.execute(
                    "INSERT INTO github_sources VALUES(?1,'github.com',?2,?3,?4,?5,?6)",
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
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    s.publish_tasks(changes);
    Ok(json!({"task_ids":ids}))
}

pub fn handle(state: Arc<Mutex<RuntimeState>>, request: GitHubRequest) -> ServerResponse {
    match handle_inner(state, request) {
        Ok(value) => ServerResponse::GitHub(value),
        Err(e) => protocol_error("github_unavailable", e),
    }
}
fn handle_inner(state: Arc<Mutex<RuntimeState>>, request: GitHubRequest) -> Result<Value, String> {
    let _operation = OPERATION
        .try_lock()
        .map_err(|_| "Another GitHub operation is in progress")?;
    let root = {
        let s = state.lock().unwrap();
        if s.remote_runtime {
            return Err("GitHub account management is available on the Mac only".into());
        }
        initialize(&s.store)?;
        s.paths.data_dir.join("tools/github")
    };
    let service = GhCli { root: root.clone() };
    handle_service(state, request, &service, &root)
}
fn handle_service(
    state: Arc<Mutex<RuntimeState>>,
    request: GitHubRequest,
    service: &dyn GitHubService,
    root: &Path,
) -> Result<Value, String> {
    match request {
        GitHubRequest::Status => Ok(
            json!({"version":VERSION,"installed":root.join("active/gh").is_file(),"connected":false,
            "connection_state":"unsupported_coexistence","detail":COEXISTENCE}),
        ),
        GitHubRequest::Install => {
            install(&root)?;
            Ok(
                json!({"installed":true,"connected":false,"connection_state":"unsupported_coexistence","detail":COEXISTENCE}),
            )
        }
        GitHubRequest::Connect => {
            verify_credential_isolation()?;
            unreachable!()
        }
        GitHubRequest::Disconnect => Ok(
            json!({"connected":false,"detail":"No GitHub credentials were changed. Imported tasks are retained."}),
        ),
        GitHubRequest::Links { project_id } => {
            let s = state.lock().unwrap();
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
            Ok(
                json!({"repositories":service.get(&format!("user/repos?affiliation=owner,collaborator,organization_member&per_page=50&page={page}"))?,"page":page}),
            )
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
            let repo = repository(&state.lock().unwrap(), project_id, repository_id)?;
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
            Ok(
                json!({"issues":issues,"page":page,"has_more":rows.len()==50,"last_synced_at":Utc::now()}),
            )
        }
        GitHubRequest::Import {
            project_id,
            repository_id,
            numbers,
        } => {
            check_registered(&state, project_id)?;
            if numbers.is_empty() || numbers.len() > 50 || numbers.contains(&0) {
                return Err("Select 1–50 issues".into());
            }
            let repo = repository(&state.lock().unwrap(), project_id, repository_id)?;
            let name = repository_name(&repo.full_name)?;
            let mut issues = Vec::new();
            for number in numbers {
                let raw = service.get(&format!("repos/{name}/issues/{number}"))?;
                if raw.get("pull_request").is_some() {
                    return Err("Pull requests cannot be imported as issues".into());
                }
                issues.push(serde_json::from_value(raw).map_err(|_| "Invalid issue response")?);
            }
            import_issues(&mut state.lock().unwrap(), project_id, &repo, issues)
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
            let repo = repository(&state.lock().unwrap(), project, source.repository_id)?;
            let name = repository_name(&repo.full_name)?;
            let raw = service.get(&format!("repos/{name}/issues/{}", source.number))?;
            let issue: GitHubIssue =
                serde_json::from_value(raw).map_err(|_| "Invalid issue response")?;
            if issue.id != source.issue_id {
                return Err("Issue identity changed; relink explicitly".into());
            }
            let mut s = state.lock().unwrap();
            let mut task = s.tasks.get(&task_id).cloned().ok_or("Task was removed")?;
            let source = task.github_source.as_mut().ok_or("Source was removed")?;
            source.state = issue.state.clone();
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
        let fake = Fake(HashMap::from([
            (
                "repos/owner/repo".into(),
                json!({"id":42,"full_name":"owner/repo","html_url":"https://github.com/owner/repo","has_issues":true}),
            ),
            (
                "repos/owner/repo/issues/7".into(),
                json!({"id":70,"number":7,"title":"Original title","body":"Original body","state":"closed","state_reason":"completed","html_url":"https://github.com/owner/repo/issues/7","updated_at":"2026-09-28T00:00:00Z"}),
            ),
        ]));
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
    fn pull_requests_and_partial_fetch_failures_cannot_create_tasks() {
        let (state, project, mut fake) = fixture();
        assert!(
            call(
                &state,
                &fake,
                GitHubRequest::Import {
                    project_id: project.id,
                    repository_id: 42,
                    numbers: vec![7, 8]
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
                    numbers: vec![7]
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
    fn credential_coexistence_is_a_hard_block_and_token_environment_is_absent() {
        assert!(verify_credential_isolation().is_err());
        let command = managed_command(Path::new("/owned/gh"), Path::new("/owned"));
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
        assert_eq!(env["GH_CONFIG_DIR"].unwrap(), "/owned/config");
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
