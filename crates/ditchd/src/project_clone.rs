//! Daemon-owned, noninteractive clones. No network work holds RuntimeState.
use super::{RuntimeState, protocol_error, register_local_project};
use ditch_core::{Project, ProjectGitPolicy};
use ditch_protocol::{ClientRequest, CloneOperationStatus, ClonePhase, ServerResponse};
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const CLONE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const MAX_LINE: usize = 4096;

#[derive(Clone, Default)]
pub(super) struct Registry(Arc<Mutex<HashMap<Uuid, Arc<Mutex<Operation>>>>>);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Input {
    url: String,
    parent: String,
    folder: String,
    name: String,
}

struct Operation {
    input: Input,
    status: CloneOperationStatus,
    cancelled: bool,
    // Set only after an exclusive mkdir. Never adopt an existing directory.
    owned: Option<(PathBuf, u64, u64)>,
    project: Option<Project>,
}

fn terminal(phase: ClonePhase) -> bool {
    matches!(
        phase,
        ClonePhase::Completed | ClonePhase::Failed | ClonePhase::Cancelled
    )
}

pub(super) fn handle(request: ClientRequest, state: Arc<Mutex<RuntimeState>>) -> ServerResponse {
    let registry = {
        let runtime = state.lock().unwrap();
        if runtime.remote_runtime {
            return protocol_error(
                "clone_local_only",
                "Clone projects on this Mac, not a remote runtime.",
            );
        }
        runtime.project_clones.clone()
    };
    if let ClientRequest::StartProjectClone {
        operation_id,
        url,
        parent,
        folder,
        name,
    } = request
    {
        let input = Input {
            url,
            parent,
            folder,
            name,
        };
        let mut entries = registry.0.lock().unwrap();
        if let Some(operation) = entries.get(&operation_id) {
            let operation = operation.lock().unwrap();
            return if operation.input == input {
                ServerResponse::ProjectClone(operation.status.clone())
            } else {
                protocol_error(
                    "clone_conflict",
                    "This clone operation ID already has different inputs.",
                )
            };
        }
        // Bound session memory without evicting IDs and breaking idempotency.
        if entries.len() >= 256 {
            return protocol_error(
                "clone_limit",
                "Restart Ditch's runtime before starting more clones.",
            );
        }
        let status = CloneOperationStatus {
            operation_id,
            destination: Path::new(&input.parent)
                .join(&input.folder)
                .to_string_lossy()
                .into(),
            phase: ClonePhase::Validating,
            percentage: None,
            progress_label: None,
            error: None,
            project: None,
            checkout_ready: false,
        };
        let operation = Arc::new(Mutex::new(Operation {
            input,
            status: status.clone(),
            cancelled: false,
            owned: None,
            project: None,
        }));
        entries.insert(operation_id, operation.clone());
        thread::spawn(move || clone_worker(operation, state));
        return ServerResponse::ProjectClone(status);
    }
    let id = match request {
        ClientRequest::ProjectCloneStatus { operation_id }
        | ClientRequest::CancelProjectClone { operation_id }
        | ClientRequest::RetryProjectCloneRegistration { operation_id } => operation_id,
        _ => unreachable!(),
    };
    let Some(operation) = registry.0.lock().unwrap().get(&id).cloned() else {
        return protocol_error(
            "clone_unknown",
            "Clone status is unavailable, possibly after a runtime restart. Inspect the destination; existing files will never be overwritten. Add a completed checkout using Local Project.",
        );
    };
    let mut op = operation.lock().unwrap();
    match request {
        ClientRequest::CancelProjectClone { .. } if !terminal(op.status.phase) => {
            op.cancelled = true;
        }
        ClientRequest::RetryProjectCloneRegistration { .. }
            if matches!(op.status.phase, ClonePhase::Failed | ClonePhase::Cancelled)
                && op.status.checkout_ready =>
        {
            op.cancelled = false;
            op.status.phase = ClonePhase::Configuring;
            op.status.error = None;
            let operation = operation.clone();
            thread::spawn(move || finish_registration(operation, state));
        }
        _ => {}
    }
    ServerResponse::ProjectClone(op.status.clone())
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-_".contains(&c))
}

fn validate_url(value: &str) -> Result<(), String> {
    let invalid = || {
        "Use an HTTPS or SSH Git URL without passwords, tokens, query strings, or fragments."
            .to_owned()
    };
    if value.is_empty()
        || value.len() > 4096
        || value.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(invalid());
    }
    if value.starts_with("https://") || value.starts_with("ssh://") {
        let url = url::Url::parse(value).map_err(|_| invalid())?;
        if !url.host_str().is_some_and(valid_host)
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || (url.scheme() == "https" && !url.username().is_empty())
            || (url.scheme() == "ssh" && !url.username().is_empty() && !valid_host(url.username()))
            || url.path().trim_matches('/').is_empty()
        {
            return Err(invalid());
        }
        return Ok(());
    }
    // GitHub's git@github.com:owner/repository.git and SSH host aliases.
    if let Some((authority, path)) = value.split_once(':') {
        let (user, host) = authority.split_once('@').unwrap_or(("git", authority));
        if valid_host(user)
            && valid_host(host)
            && !path.is_empty()
            && !path.starts_with('-')
            && !path.starts_with('/')
            && !path.contains(':')
            && !path.contains('?')
            && !path.contains('#')
            && path
                .split('/')
                .all(|p| !p.is_empty() && p != "." && p != "..")
        {
            return Ok(());
        }
    }
    Err(invalid())
}

fn destination(input: &Input) -> Result<PathBuf, String> {
    if input.name.trim().is_empty()
        || input.name.len() > 256
        || input.name.chars().any(char::is_control)
    {
        return Err("Enter a project name (up to 256 characters).".into());
    }
    let folder = &input.folder;
    if folder.is_empty()
        || folder.len() > 255
        || folder == "."
        || folder == ".."
        || folder.starts_with('-')
        || folder.contains(['/', '\\'])
        || folder.chars().any(char::is_control)
    {
        return Err("Use a single new folder name, without slashes or traversal.".into());
    }
    let parent = Path::new(&input.parent);
    if !parent.is_absolute() {
        return Err("Choose an absolute destination parent directory.".into());
    }
    let parent = fs::canonicalize(parent)
        .map_err(|_| "Destination parent is unavailable. Check its permissions.".to_owned())?;
    if !parent.is_dir() {
        return Err("Destination parent must be a directory.".into());
    }
    Ok(parent.join(folder))
}

fn owns_destination(op: &Operation) -> bool {
    op.owned.as_ref().is_some_and(|(path, device, inode)| {
        fs::symlink_metadata(path)
            .is_ok_and(|m| m.is_dir() && m.dev() == *device && m.ino() == *inode)
            && fs::canonicalize(path).is_ok_and(|canonical| canonical == *path)
    })
}

fn clone_worker(operation: Arc<Mutex<Operation>>, state: Arc<Mutex<RuntimeState>>) {
    let binary = super::find_binary("git").unwrap_or_else(|| "git".into());
    clone_worker_with_command(operation, state, git_command(&binary), CLONE_TIMEOUT);
}

fn clone_worker_with_command(
    operation: Arc<Mutex<Operation>>,
    state: Arc<Mutex<RuntimeState>>,
    command: Command,
    timeout: Duration,
) {
    let result = clone_checkout(&operation, timeout, command);
    match result {
        Ok(()) => finish_registration(operation, state),
        Err(error) => {
            let mut op = operation.lock().unwrap();
            // A completed checkout is valuable even if registration failed.
            if !op.status.checkout_ready
                && owns_destination(&op)
                && fs::remove_dir_all(&op.owned.as_ref().unwrap().0).is_err()
            {
                op.status.error = Some(format!(
                    "{error} Partial checkout could not be removed; inspect {}.",
                    op.status.destination
                ));
            }
            if op.status.error.is_none() {
                op.status.error = Some(error);
            }
            op.status.phase = if op.cancelled {
                ClonePhase::Cancelled
            } else {
                ClonePhase::Failed
            };
            op.status.percentage = None;
            op.status.progress_label = None;
        }
    }
}

fn clone_checkout(
    operation: &Arc<Mutex<Operation>>,
    timeout: Duration,
    mut command: Command,
) -> Result<(), String> {
    let input = operation.lock().unwrap().input.clone();
    validate_url(&input.url)?;
    let destination = destination(&input)?;
    {
        let mut op = operation.lock().unwrap();
        if op.cancelled {
            return Err("Clone cancelled.".into());
        }
        // Atomic reservation rejects existing files, directories, and symlinks,
        // including races with another operation or another Ditch process.
        fs::create_dir(&destination).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                "Destination already exists. Choose a new folder; Ditch never overwrites it."
                    .to_owned()
            } else {
                "Cannot create destination. Check parent directory permissions and free space."
                    .to_owned()
            }
        })?;
        let metadata = fs::symlink_metadata(&destination)
            .map_err(|_| "Cannot inspect destination.".to_owned())?;
        op.owned = Some((destination.clone(), metadata.dev(), metadata.ino()));
        op.status.destination = destination.to_string_lossy().into();
        op.status.phase = ClonePhase::Cloning;
    }
    command
        .args(["clone", "--progress", "--template=", "--", &input.url])
        .arg(&destination);
    run_git(command, operation, timeout)?;
    {
        let mut op = operation.lock().unwrap();
        if !owns_destination(&op)
            || !fs::symlink_metadata(destination.join(".git"))
                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        {
            return Err(
                "Clone destination changed or is not a repository; inspect it before retrying."
                    .into(),
            );
        }
        // Git exited successfully, including checkout. Keep it on later errors.
        op.status.checkout_ready = true;
        let mut project = Project::new(&input.name, &destination);
        project.git_policy = ProjectGitPolicy::RequireRepository;
        op.project = Some(project);
    }
    Ok(())
}

fn git_command(binary: &str) -> Command {
    let mut command = Command::new(binary);
    command
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.ssh.allow=always",
            "-c",
            "credential.interactive=false",
            "-c",
            "submodule.recurse=false",
        ])
        .env("GIT_ALLOW_PROTOCOL", "https:ssh")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -oBatchMode=yes -oStrictHostKeyChecking=yes",
        )
        .env("GIT_SSH_VARIANT", "ssh")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .process_group(0);
    // Avoid inheriting an unrelated repository context from the app launcher.
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
    ] {
        command.env_remove(key);
    }
    command
}

/// Keep only recognized progress and error categories. Raw remote diagnostics
/// can contain credentials or terminal escapes and never cross the protocol.
#[derive(Default)]
struct OutputParser {
    line: Vec<u8>,
    error: Option<&'static str>,
}

impl OutputParser {
    fn feed(&mut self, bytes: &[u8], operation: &Arc<Mutex<Operation>>) {
        for &byte in bytes {
            if byte == b'\r' || byte == b'\n' {
                self.line(operation);
                self.line.clear();
            } else if self.line.len() < MAX_LINE {
                self.line.push(byte);
            }
        }
    }

    fn line(&mut self, operation: &Arc<Mutex<Operation>>) {
        let line = String::from_utf8_lossy(&self.line);
        let lower = line.to_ascii_lowercase();
        if lower.contains("host key verification failed") {
            self.error = Some(
                "SSH host key verification failed. Verify this host in Terminal before retrying.",
            );
        } else if lower.contains("authentication failed")
            || lower.contains("permission denied (publickey)")
            || lower.contains("could not read username")
            || lower.contains("repository not found")
        {
            self.error = Some(
                "Git authentication or repository access failed. Check the URL and your existing credential helper or SSH agent in Terminal.",
            );
        } else if lower.contains("permission denied") || lower.contains("no space left") {
            self.error = Some(
                "Git cannot write the checkout. Check destination permissions and free space.",
            );
        } else if lower.contains("could not resolve")
            || lower.contains("unable to access")
            || lower.contains("connection")
            || lower.contains("timed out")
        {
            self.error = Some(
                "Git could not reach the repository. Check your network, URL, and SSH configuration.",
            );
        }
        if let Some((phase, label, percent)) = progress(&line) {
            let mut op = operation.lock().unwrap();
            op.status.phase = phase;
            op.status.progress_label = Some(label.into());
            op.status.percentage = percent;
        }
    }
}

fn progress(line: &str) -> Option<(ClonePhase, &'static str, Option<u8>)> {
    for label in [
        "Enumerating objects",
        "Counting objects",
        "Compressing objects",
        "Receiving objects",
        "Resolving deltas",
        "Updating files",
        "Filtering content",
    ] {
        let line = line.strip_prefix("remote: ").unwrap_or(line).trim();
        if let Some(rest) = line.strip_prefix(&format!("{label}:")) {
            let percentage = rest
                .split_once('%')
                .and_then(|(n, _)| n.trim().parse::<u8>().ok())
                .filter(|p| *p <= 100);
            let phase = if matches!(label, "Updating files" | "Filtering content") {
                ClonePhase::Checkout
            } else {
                ClonePhase::Cloning
            };
            return Some((phase, label, percentage));
        }
    }
    None
}

fn run_git(
    mut command: Command,
    operation: &Arc<Mutex<Operation>>,
    timeout: Duration,
) -> Result<(), String> {
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            "Git is not installed or is unavailable to Ditch. Install Git and restart the runtime."
                .to_owned()
        } else {
            "Git could not start. Check executable permissions.".to_owned()
        }
    })?;
    let mut stderr = child.stderr.take().unwrap();
    // Nonblocking reads keep cancellation and deadlines effective even when a
    // helper leaves its inherited stderr open after Git exits.
    let fd = stderr.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
        return Err("Cannot monitor Git output.".into());
    }
    let mut parser = OutputParser::default();
    let mut drain = |stderr: &mut std::process::ChildStderr| {
        let mut buffer = [0; 8192];
        // A noisy server cannot starve the timeout/cancellation check.
        for _ in 0..16 {
            match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => parser.feed(&buffer[..count], operation),
            }
        }
    };
    let deadline = Instant::now() + timeout;
    let result = loop {
        drain(&mut stderr);
        if operation.lock().unwrap().cancelled {
            break Err("Clone cancelled.");
        }
        if Instant::now() >= deadline {
            break Err("Clone timed out. Check your connection and retry.");
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                break if status.success() {
                    Ok(())
                } else {
                    Err(
                        "Git clone failed. Check repository access and your Git configuration in Terminal.",
                    )
                };
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => break Err("Unable to monitor Git clone."),
        }
    };
    // Kill the entire group even after the leader exits, so a transport/helper
    // cannot keep running after this operation. Reap the direct child.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    drain(&mut stderr);
    parser.line(operation);
    let diagnostic = parser.error;
    result.map_err(|e| {
        if e == "Clone cancelled." || e.contains("timed out") {
            e.to_owned()
        } else {
            diagnostic.unwrap_or(e).to_owned()
        }
    })
}

fn finish_registration(operation: Arc<Mutex<Operation>>, state: Arc<Mutex<RuntimeState>>) {
    let project = {
        let mut op = operation.lock().unwrap();
        op.status.phase = ClonePhase::Configuring;
        op.status.percentage = None;
        op.status.progress_label = None;
        op.project
            .clone()
            .expect("completed checkout has a fresh identity")
    };
    // Verify again on registration retries in case the retained checkout changed.
    let mut verify = git_command(&super::find_binary("git").unwrap_or_else(|| "git".into()));
    verify
        .arg("-C")
        .arg(&project.root)
        .args(["rev-parse", "--git-dir"]);
    let verified = run_git(verify, &operation, Duration::from_secs(30));
    // Cancellation and registration share this lock. Once registration begins,
    // cancellation observes its final result and cannot relabel a saved project.
    let mut op = operation.lock().unwrap();
    if op.cancelled {
        op.status.phase = ClonePhase::Cancelled;
        op.status.error = Some("Clone cancelled after checkout. The completed checkout was retained; retry registration or add it using Local Project.".into());
        return;
    }
    let result = if !owns_destination(&op)
        || !fs::symlink_metadata(project.root.join(".git"))
            .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        Err("The checkout destination changed.".to_owned())
    } else {
        verified
            .and_then(|()| {
                ditch_store::validate_cloned_project_metadata(&project.root)
                    .map_err(|e| e.to_string())
            })
            .and_then(|()| register_local_project(&state, project).map_err(|error| error.message))
    };
    match result {
        Ok(project) => {
            op.status.phase = ClonePhase::Completed;
            op.status.project = Some(project);
            op.status.error = None;
        }
        Err(_) => {
            op.status.phase = ClonePhase::Failed;
            op.status.error = Some("Checkout completed, but Ditch could not register it. Check permissions, free space, and .ditch metadata for symlinks or special files, then retry registration. The checkout has been retained.".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("ditch-clone-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            Self(fs::canonicalize(root).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn operation(root: &Path) -> Arc<Mutex<Operation>> {
        Arc::new(Mutex::new(Operation {
            input: Input {
                url: "https://example.invalid/repo".into(),
                parent: root.to_string_lossy().into(),
                folder: "checkout".into(),
                name: "Cloned project".into(),
            },
            status: CloneOperationStatus {
                operation_id: Uuid::new_v4(),
                destination: root.join("checkout").to_string_lossy().into(),
                phase: ClonePhase::Validating,
                percentage: None,
                progress_label: None,
                error: None,
                project: None,
                checkout_ready: false,
            },
            cancelled: false,
            owned: None,
            project: None,
        }))
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script, "fake-git"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0);
        command
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Clone test",
                "-c",
                "user.email=clone@example.invalid",
            ])
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn source(f: &Fixture) -> (PathBuf, Project) {
        let root = f.0.join("source");
        fs::create_dir(&root).unwrap();
        git(&root, &["init"]);
        let project = Project::new("Original", &root);
        ditch_store::ensure_project_metadata(&project).unwrap();
        ditch_store::save_project_config(&project).unwrap();
        fs::write(root.join("README.md"), "example").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", "fixture"]);
        (root, project)
    }

    fn local_clone(source: &Path) -> Command {
        let mut command = git_command("git");
        command.env("GIT_ALLOW_PROTOCOL", "https:ssh:file");
        // Only tests allow file transport. Production accepts HTTPS and SSH.
        command
            .args(["-c", "protocol.file.allow=always", "-c"])
            .arg(format!(
                "url.file://{}.insteadOf=https://example.invalid/repo",
                source.display()
            ));
        command
    }

    #[test]
    fn accepts_https_ssh_and_scp_but_rejects_credentials_and_helpers() {
        for url in [
            "https://github.com/owner/repo.git",
            "ssh://git@github.com/owner/repo",
            "ssh://git@host:2222/repo",
            "git@github.com:owner/repo.git",
            "my-host:owner/repo",
        ] {
            assert!(validate_url(url).is_ok(), "{url}");
        }
        for url in [
            "",
            "--upload-pack=evil",
            "file:///tmp/repo",
            "ext::sh evil",
            "git://host/repo",
            "https://token@github.com/o/r",
            "https://u:p@github.com/o/r",
            "ssh://u:p@host/r",
            "https://host/r?token=secret",
            "https://host/r#secret",
            "git@host:../repo",
            "git@host:repo\n--evil",
            "ssh://-evil/repo",
            "git@host:-repo",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn validates_single_child_and_canonicalizes_parent() {
        let f = Fixture::new();
        symlink(&f.0, f.0.join("alias")).unwrap();
        let op = operation(&f.0.join("alias"));
        let mut op = op.lock().unwrap();
        assert_eq!(destination(&op.input).unwrap(), f.0.join("checkout"));
        for folder in [
            "",
            ".",
            "..",
            "../outside",
            "/absolute",
            "a/b",
            "a\\b",
            "-option",
            "bad\nname",
        ] {
            op.input.folder = folder.into();
            assert!(destination(&op.input).is_err(), "{folder}");
        }
    }

    #[test]
    fn parses_cr_lf_and_bounds_untrusted_output_without_exposing_it() {
        let f = Fixture::new();
        let op = operation(&f.0);
        let mut parser = OutputParser::default();
        parser.feed(
            b"remote: Counting objects: 10% (1/10)\rReceiving objects: 52% (5/10)\n",
            &op,
        );
        assert_eq!(op.lock().unwrap().status.percentage, Some(52));
        parser.feed(b"Updating files: 90% (9/10)\r", &op);
        assert_eq!(op.lock().unwrap().status.phase, ClonePhase::Checkout);
        parser.feed(b"Enumerating objects: 12, done.\n", &op);
        assert_eq!(op.lock().unwrap().status.percentage, None);
        parser.feed(&vec![b'x'; MAX_LINE * 50], &op);
        assert_eq!(parser.line.len(), MAX_LINE);
        parser.feed(
            b"\nfatal: Authentication failed for https://user:secret@host/repo\n",
            &op,
        );
        assert!(!parser.error.unwrap().contains("secret"));
        assert_eq!(
            progress("Receiving objects: 101%"),
            Some((ClonePhase::Cloning, "Receiving objects", None))
        );
    }

    #[test]
    fn occupied_and_symlink_destinations_are_never_touched() {
        let f = Fixture::new();
        let path = f.0.join("checkout");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), "user data").unwrap();
        let op = operation(&f.0);
        assert!(clone_checkout(&op, Duration::from_secs(1), shell("exit 0")).is_err());
        assert!(op.lock().unwrap().owned.is_none());
        assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "user data");
        fs::remove_dir_all(&path).unwrap();
        symlink(f.0.join("missing"), &path).unwrap();
        assert!(clone_checkout(&op, Duration::from_secs(1), shell("exit 0")).is_err());
        assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    }

    #[test]
    fn timeout_and_cancellation_kill_transport_children_and_clean_partial_checkout() {
        for cancel in [false, true] {
            let f = Fixture::new();
            let op = operation(&f.0);
            let state = Arc::new(Mutex::new(super::super::tests::test_runtime()));
            let pid = f.0.join("child.pid");
            let mut command =
                shell("(sleep 1; echo survived > \"$1.survived\") & echo $! > \"$1\"; wait");
            command.arg(&pid);
            let worker_op = op.clone();
            let worker_state = state.clone();
            let start = Instant::now();
            let worker = thread::spawn(move || {
                clone_worker_with_command(
                    worker_op,
                    worker_state,
                    command,
                    Duration::from_millis(500),
                )
            });
            while !pid.exists() && start.elapsed() < Duration::from_secs(3) {
                thread::sleep(Duration::from_millis(10));
            }
            if cancel {
                op.lock().unwrap().cancelled = true;
            }
            // Other runtime requests do not wait on Git's network work.
            assert!(matches!(
                super::super::handle_request(ClientRequest::Health, state),
                ServerResponse::Health(_)
            ));
            worker.join().unwrap();
            assert!(start.elapsed() < Duration::from_secs(3));
            assert!(!f.0.join("checkout").exists());
            assert_eq!(
                op.lock().unwrap().status.phase,
                if cancel {
                    ClonePhase::Cancelled
                } else {
                    ClonePhase::Failed
                }
            );
            // A transport grandchild must never execute its delayed write.
            thread::sleep(Duration::from_millis(1100));
            assert!(!pid.with_extension("pid.survived").exists());
        }
    }

    #[test]
    fn reservation_rejects_concurrent_clone_and_cleanup_checks_inode() {
        let f = Fixture::new();
        let op = operation(&f.0);
        assert!(clone_checkout(&op, Duration::from_millis(50), shell("sleep 30")).is_err());
        let second = operation(&f.0);
        assert!(clone_checkout(&second, Duration::from_secs(1), shell("exit 0")).is_err());
        assert!(second.lock().unwrap().owned.is_none());
        fs::rename(f.0.join("checkout"), f.0.join("moved")).unwrap();
        fs::create_dir(f.0.join("checkout")).unwrap();
        assert!(!owns_destination(&op.lock().unwrap()));
    }

    #[test]
    fn cloned_project_gets_fresh_identity_metadata_and_registry_entry() {
        let f = Fixture::new();
        let (source, original) = source(&f);
        let op = operation(&f.0);
        let state = Arc::new(Mutex::new(super::super::tests::test_runtime()));
        // Original ID already exists; clone must not overwrite it.
        register_local_project(&state, original.clone()).unwrap();
        clone_worker_with_command(
            op.clone(),
            state.clone(),
            local_clone(&source),
            Duration::from_secs(10),
        );
        let status = op.lock().unwrap().status.clone();
        assert_eq!(status.phase, ClonePhase::Completed, "{:?}", status.error);
        let project = status.project.unwrap();
        assert_ne!(project.id, original.id);
        assert_eq!(
            ditch_store::load_project_config(&project.root).unwrap().id,
            project.id
        );
        for directory in ["agents", "hooks", "mcp"] {
            assert!(project.root.join(".ditch").join(directory).is_dir());
        }
        let runtime = state.lock().unwrap();
        assert_eq!(runtime.projects.len(), 2);
        assert!(
            runtime
                .store
                .load()
                .unwrap()
                .projects
                .iter()
                .any(|p| p.id == project.id)
        );
        assert_eq!(runtime.event_replay.len(), 2);
        drop(runtime);
        let id = status.operation_id;
        state
            .lock()
            .unwrap()
            .project_clones
            .0
            .lock()
            .unwrap()
            .insert(id, op.clone());
        let ServerResponse::ProjectClone(cancelled) = handle(
            ClientRequest::CancelProjectClone { operation_id: id },
            state,
        ) else {
            panic!()
        };
        assert_eq!(cancelled.phase, ClonePhase::Completed);
    }

    #[test]
    fn malicious_metadata_is_retained_and_registration_can_be_retried() {
        let f = Fixture::new();
        let (source, _) = source(&f);
        let outside = f.0.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(
            &outside,
            source.join(".ditch/agents").with_file_name("bad-link"),
        )
        .unwrap();
        // Replace an empty metadata directory with a tracked symlink.
        fs::remove_dir(source.join(".ditch/agents")).unwrap();
        symlink(&outside, source.join(".ditch/agents")).unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "malicious metadata"]);
        let op = operation(&f.0);
        let state = Arc::new(Mutex::new(super::super::tests::test_runtime()));
        clone_worker_with_command(
            op.clone(),
            state.clone(),
            local_clone(&source),
            Duration::from_secs(10),
        );
        assert_eq!(op.lock().unwrap().status.phase, ClonePhase::Failed);
        assert!(op.lock().unwrap().status.checkout_ready);
        assert!(f.0.join("checkout/README.md").is_file());
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
        assert!(state.lock().unwrap().projects.is_empty());
        let id = op.lock().unwrap().status.operation_id;
        state
            .lock()
            .unwrap()
            .project_clones
            .0
            .lock()
            .unwrap()
            .insert(id, op.clone());
        fs::remove_file(f.0.join("checkout/.ditch/agents")).unwrap();
        handle(
            ClientRequest::RetryProjectCloneRegistration { operation_id: id },
            state.clone(),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while !terminal(op.lock().unwrap().status.phase) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(op.lock().unwrap().status.phase, ClonePhase::Completed);
        assert_eq!(state.lock().unwrap().projects.len(), 1);
    }

    #[test]
    fn start_is_idempotent_conflicts_are_rejected_and_unknown_is_explicit() {
        let f = Fixture::new();
        let state = Arc::new(Mutex::new(super::super::tests::test_runtime()));
        let id = Uuid::new_v4();
        let request = ClientRequest::StartProjectClone {
            operation_id: id,
            url: "invalid".into(),
            parent: f.0.to_string_lossy().into(),
            folder: "checkout".into(),
            name: "test".into(),
        };
        assert!(matches!(
            handle(request.clone(), state.clone()),
            ServerResponse::ProjectClone(_)
        ));
        assert!(matches!(
            handle(request, state.clone()),
            ServerResponse::ProjectClone(_)
        ));
        let conflict = ClientRequest::StartProjectClone {
            operation_id: id,
            url: "different".into(),
            parent: "".into(),
            folder: "".into(),
            name: "".into(),
        };
        assert!(matches!(
            handle(conflict, state.clone()),
            ServerResponse::Error(_)
        ));
        assert_eq!(
            state.lock().unwrap().project_clones.0.lock().unwrap().len(),
            1
        );
        assert!(matches!(
            handle(
                ClientRequest::ProjectCloneStatus {
                    operation_id: Uuid::new_v4()
                },
                state
            ),
            ServerResponse::Error(_)
        ));
    }

    #[test]
    fn database_failure_keeps_checkout_and_retries_with_same_fresh_identity() {
        let f = Fixture::new();
        let (source, original) = source(&f);
        let op = operation(&f.0);
        let state = Arc::new(Mutex::new(super::super::tests::test_runtime()));
        let database = state.lock().unwrap().paths.database_path.clone();
        let connection = rusqlite::Connection::open(database).unwrap();
        connection.execute_batch("CREATE TRIGGER reject_clone BEFORE INSERT ON projects BEGIN SELECT RAISE(FAIL, 'simulated store failure'); END;").unwrap();
        clone_worker_with_command(
            op.clone(),
            state.clone(),
            local_clone(&source),
            Duration::from_secs(10),
        );
        assert_eq!(op.lock().unwrap().status.phase, ClonePhase::Failed);
        assert!(op.lock().unwrap().status.checkout_ready);
        assert!(state.lock().unwrap().projects.is_empty());
        let saved = ditch_store::load_project_config(&f.0.join("checkout")).unwrap();
        assert_ne!(saved.id, original.id);
        connection
            .execute_batch("DROP TRIGGER reject_clone;")
            .unwrap();
        finish_registration(op.clone(), state.clone());
        assert_eq!(
            op.lock().unwrap().status.project.as_ref().unwrap().id,
            saved.id
        );
        assert_eq!(state.lock().unwrap().projects.len(), 1);
    }

    #[test]
    fn every_written_metadata_path_rejects_symlinks_including_dangling_links() {
        for relative in [
            ".ditch",
            ".ditch/agents",
            ".ditch/hooks",
            ".ditch/mcp",
            ".ditch/project.json",
        ] {
            let f = Fixture::new();
            let path = f.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink(f.0.join("missing"), &path).unwrap();
            assert!(
                ditch_store::validate_cloned_project_metadata(&f.0).is_err(),
                "{relative}"
            );
            assert!(!f.0.join("missing").exists());
        }
    }
}
