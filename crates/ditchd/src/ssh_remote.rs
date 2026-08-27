use ditch_core::{Project, ProjectExecutionTarget, ProjectGitPolicy};
use ditch_protocol::{
    ClientRequest, Envelope, RemoteCheckState, RemoteDirectory, RemoteRuntimeHandshake,
    RemoteSetupCheck, RemoteSetupStatus, ServerEvent, ServerResponse,
};
use ditch_ssh::{
    AskpassGuard, NewSshHost, ResolvedSshHost, SshError, SshHostSummary, append_config_entry,
    base_ssh_command, bridge_command, configure_askpass, default_config_path, discover_hosts,
    load_password, render_config_entry, resolve_host, run_fixed_admin_authenticated, save_password,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

const PREFLIGHT_SCRIPT: &str = r#"set -eu
printf 'home\t%s\n' "$HOME"
printf 'os\t%s\n' "$(uname -s 2>/dev/null || printf unknown)"
printf 'arch\t%s\n' "$(uname -m 2>/dev/null || printf unknown)"
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then printf 'service\tsystemd-user\n';
elif [ "$(uname -s 2>/dev/null || true)" = Darwin ] && command -v launchctl >/dev/null 2>&1; then printf 'service\tlaunchd-user\n';
else printf 'service\tunavailable\n'; fi
if [ "$(uname -s 2>/dev/null || true)" = Linux ] && command -v loginctl >/dev/null 2>&1; then
  printf 'persistence\t%s\n' "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || printf unknown)"
else printf 'persistence\tnot-applicable\n'; fi
if command -v git >/dev/null 2>&1; then printf 'git\t%s\n' "$(git --version 2>/dev/null | head -n 1)"; else printf 'git\tmissing\n'; fi
codex_binary=$(command -v codex 2>/dev/null || true)
[ -n "$codex_binary" ] || [ ! -x "$HOME/.local/bin/codex" ] || codex_binary="$HOME/.local/bin/codex"
if [ -n "$codex_binary" ]; then
  printf 'codex\t%s\n' "$("$codex_binary" --version 2>/dev/null | head -n 1)"
  if "$codex_binary" login status >/dev/null 2>&1; then printf 'codex_auth\tready\n'; else printf 'codex_auth\trequired\n'; fi
else printf 'codex\tmissing\n'; printf 'codex_auth\tunavailable\n'; fi
if [ -x "$HOME/.ditch/bin/current/ditchd" ]; then
  printf 'runtime\t%s\n' "$("$HOME/.ditch/bin/current/ditchd" version-json 2>/dev/null || printf invalid)"
else printf 'runtime\tmissing\n'; fi
if command -v apt-get >/dev/null 2>&1; then printf 'package_manager\tapt\n';
elif command -v dnf >/dev/null 2>&1; then printf 'package_manager\tdnf\n';
elif command -v yum >/dev/null 2>&1; then printf 'package_manager\tyum\n';
elif command -v zypper >/dev/null 2>&1; then printf 'package_manager\tzypper\n';
elif command -v brew >/dev/null 2>&1; then printf 'package_manager\tbrew\n';
else printf 'package_manager\tunknown\n'; fi
"#;

#[derive(Clone, Default)]
pub struct RemoteConnectionManager {
    bridges: Arc<Mutex<HashMap<String, Arc<Mutex<Bridge>>>>>,
    session_passwords: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

struct Bridge {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    _askpass: AskpassGuard,
}

#[derive(Deserialize)]
struct RemoteSequencedEvent {
    #[allow(dead_code)]
    sequence: u64,
    event: ServerEvent,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl RemoteConnectionManager {
    pub fn remember_session_password(&self, alias: &str, password: Vec<u8>) {
        if let Some(mut old) = self
            .session_passwords
            .lock()
            .unwrap()
            .insert(alias.to_owned(), password)
        {
            old.fill(0);
        }
    }

    fn password_for(&self, resolved: &ResolvedSshHost) -> Option<Vec<u8>> {
        self.session_passwords
            .lock()
            .unwrap()
            .get(&resolved.alias)
            .cloned()
            .or_else(|| load_password(&resolved.credential_id()).ok().flatten())
    }

    fn bridge(&self, alias: &str) -> Result<Arc<Mutex<Bridge>>, SshError> {
        if let Some(value) = self.bridges.lock().unwrap().get(alias).cloned() {
            return Ok(value);
        }
        let resolved = resolve_host(alias)?;
        observe("remote_host_connect_started", alias, Some("bridge"));
        let mut command = bridge_command(alias)?;
        let (prompts, guard) =
            configure_askpass(&mut command, self.password_for(&resolved), false)?;
        let mut child = command
            .spawn()
            .map_err(|error| SshError::Failed(error.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SshError::Failed("SSH bridge has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SshError::Failed("SSH bridge has no stdout".into()))?;
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                // Drain to prevent transport backpressure. SSH diagnostics are
                // intentionally not copied into runtime logs because prompts
                // can contain private host/user details.
                for _ in BufReader::new(stderr).lines().take(512) {}
            });
        }
        if let Some(prompt) = prompts.host_key_prompt() {
            return Err(SshError::Failed(prompt));
        }
        let value = Arc::new(Mutex::new(Bridge {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            _askpass: guard,
        }));
        self.bridges
            .lock()
            .unwrap()
            .insert(alias.to_owned(), value.clone());
        observe("remote_daemon_connected", alias, None);
        Ok(value)
    }

    pub fn request(&self, alias: &str, request: ClientRequest) -> Result<ServerResponse, SshError> {
        let bridge = self.bridge(alias)?;
        let mut bridge = bridge.lock().unwrap();
        let envelope = Envelope::new(request);
        serde_json::to_writer(&mut bridge.stdin, &envelope)
            .map_err(|error| SshError::Failed(error.to_string()))?;
        bridge.stdin.write_all(b"\n")?;
        bridge.stdin.flush()?;
        let mut descriptor = libc::pollfd {
            fd: bridge.stdout.get_ref().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Timing out owns only this transport. It never terminates or retries
        // a remote mutation, daemon, PTY, or agent.
        let ready = unsafe { libc::poll(&mut descriptor, 1, 30_000) };
        if ready <= 0 {
            self.bridges.lock().unwrap().remove(alias);
            return Err(if ready == 0 {
                SshError::Timeout
            } else {
                SshError::Failed(std::io::Error::last_os_error().to_string())
            });
        }
        let mut line = String::new();
        if bridge.stdout.read_line(&mut line)? == 0 {
            self.bridges.lock().unwrap().remove(alias);
            observe("remote_daemon_disconnected", alias, None);
            return Err(SshError::Failed("SSH bridge disconnected".into()));
        }
        let response =
            serde_json::from_str::<Envelope<ServerResponse>>(&line).map_err(|error| {
                SshError::Failed(format!("invalid remote daemon response: {error}"))
            })?;
        Ok(response.body)
    }

    /// Opens a dedicated typed event subscription over OpenSSH. This is a
    /// second bridge process because a subscription is intentionally
    /// long-lived, while request/response RPCs remain serialized on the
    /// shared bridge. Neither bridge owns the remote daemon or its children.
    pub fn stream_events(
        &self,
        alias: &str,
        mut on_event: impl FnMut(ServerEvent),
    ) -> Result<(), SshError> {
        let resolved = resolve_host(alias)?;
        let mut command = bridge_command(alias)?;
        let (prompts, guard) =
            configure_askpass(&mut command, self.password_for(&resolved), false)?;
        let mut child = command
            .spawn()
            .map_err(|error| SshError::Failed(error.to_string()))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| SshError::Failed("SSH event bridge has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SshError::Failed("SSH event bridge has no stdout".into()))?;
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || for _ in BufReader::new(stderr).lines().take(512) {});
        }
        if let Some(prompt) = prompts.host_key_prompt() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SshError::Failed(prompt));
        }

        let envelope = Envelope::new(ClientRequest::SubscribeEvents { since_sequence: 0 });
        serde_json::to_writer(&mut stdin, &envelope)
            .map_err(|error| SshError::Failed(error.to_string()))?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;

        let result = (|| {
            for line in BufReader::new(stdout).lines() {
                let line = line?;
                let envelope = serde_json::from_str::<Envelope<RemoteSequencedEvent>>(&line)
                    .map_err(|error| {
                        SshError::Failed(format!("invalid remote daemon event: {error}"))
                    })?;
                on_event(envelope.body.event);
            }
            Err(SshError::Failed("SSH event bridge disconnected".into()))
        })();
        drop(guard);
        let _ = child.kill();
        let _ = child.wait();
        result
    }

    pub fn disconnect(&self, alias: &str) {
        self.bridges.lock().unwrap().remove(alias);
    }
}

pub fn ssh_hosts() -> Result<Vec<SshHostSummary>, SshError> {
    discover_hosts(&default_config_path())
}

pub fn ssh_host(alias: &str) -> Result<ResolvedSshHost, SshError> {
    resolve_host(alias)
}

pub fn preview_host(host: &NewSshHost) -> Result<String, SshError> {
    render_config_entry(host)
}

pub fn add_host(host: &NewSshHost) -> Result<(), SshError> {
    append_config_entry(&default_config_path(), host)
}

pub fn check_setup(
    connections: &RemoteConnectionManager,
    alias: &str,
    password: Option<String>,
    remember_password: bool,
    trust_unknown_host: bool,
) -> Result<RemoteSetupStatus, SshError> {
    observe("remote_host_connect_started", alias, Some("preflight"));
    let resolved = resolve_host(alias)?;
    let supplied = password.map(String::into_bytes);
    let credential = supplied
        .clone()
        .or_else(|| connections.password_for(&resolved));
    let (output, prompts) =
        run_fixed_admin_authenticated(alias, PREFLIGHT_SCRIPT, credential, trust_unknown_host)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        return Ok(failed_setup(
            alias,
            "Host key changed. Stop and verify the host's known_hosts entry before reconnecting.",
            Some(stderr.chars().take(4096).collect()),
        ));
    }
    if !output.status.success() {
        if let Some(prompt) = prompts.host_key_prompt() {
            return Ok(RemoteSetupStatus {
                ssh_host_alias: alias.to_owned(),
                remote_machine_id: None,
                ready: false,
                connection_state: "host_key_confirmation_required".into(),
                home_directory: None,
                checks: vec![check(
                    "ssh",
                    "SSH",
                    RemoteCheckState::AuthenticationRequired,
                    "Trust this host key to continue",
                    Some(prompt.clone()),
                )],
                handshake: empty_handshake(),
                host_key_fingerprint: fingerprint(&prompt),
                last_error: None,
            });
        }
        observe("remote_host_offline", alias, Some("preflight_failed"));
        return Ok(failed_setup(
            alias,
            "SSH authentication failed or the host is unavailable.",
            Some(stderr.chars().take(4096).collect()),
        ));
    }
    if let Some(secret) = supplied {
        connections.remember_session_password(alias, secret.clone());
        if remember_password {
            save_password(&resolved.credential_id(), &secret)?;
        }
    }
    let values = parse_preflight(&output.stdout);
    let os = values.get("os").cloned().unwrap_or_default();
    let arch = values.get("arch").cloned().unwrap_or_default();
    let supported = matches!(os.as_str(), "Linux" | "Darwin")
        && matches!(
            (os.as_str(), arch.as_str()),
            ("Linux", "x86_64" | "aarch64" | "arm64") | ("Darwin", "arm64" | "x86_64")
        );
    let runtime_value = values
        .get("runtime")
        .cloned()
        .unwrap_or_else(|| "missing".into());
    observe("remote_host_connected", alias, None);
    if runtime_value == "missing" {
        observe("remote_runtime_missing", alias, None);
    }
    let runtime_json = serde_json::from_str::<serde_json::Value>(&runtime_value).ok();
    let server_version = runtime_json
        .as_ref()
        .and_then(|value| value.get("version"))
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let server_protocol = runtime_json
        .as_ref()
        .and_then(|value| value.get("remote_runtime_protocol_version"))
        .and_then(|value| value.as_u64())
        .and_then(|value| u16::try_from(value).ok());
    let server_build = runtime_json
        .as_ref()
        .and_then(|value| value.get("build_identifier"))
        .and_then(|value| value.as_str());
    let local_build = option_env!("DITCH_BUILD_IDENTIFIER").unwrap_or(env!("CARGO_PKG_VERSION"));
    let exact = server_version.as_deref() == Some(env!("CARGO_PKG_VERSION"))
        && server_protocol == Some(ditch_protocol::REMOTE_RUNTIME_PROTOCOL_VERSION)
        && server_build == Some(local_build);
    let mut checks = vec![
        check(
            "ssh",
            "SSH",
            RemoteCheckState::Ready,
            "Connected securely",
            None,
        ),
        check(
            "platform",
            "Platform",
            if supported {
                RemoteCheckState::Ready
            } else {
                RemoteCheckState::Failed
            },
            &format!("{os} {arch}"),
            None,
        ),
        check(
            "runtime",
            "Ditch Runtime",
            if exact {
                RemoteCheckState::Ready
            } else if runtime_value == "missing" {
                RemoteCheckState::Missing
            } else {
                RemoteCheckState::InstallAvailable
            },
            if exact {
                "Exact version installed"
            } else if runtime_value == "missing" {
                "Not installed"
            } else {
                "Version update required"
            },
            None,
        ),
    ];
    let git = values.get("git").map(String::as_str).unwrap_or("missing");
    let package_manager = values
        .get("package_manager")
        .map(String::as_str)
        .unwrap_or("unknown");
    let git_instructions = match package_manager {
        "apt" => Some("sudo apt-get update && sudo apt-get install git"),
        "dnf" => Some("sudo dnf install git"),
        "yum" => Some("sudo yum install git"),
        "zypper" => Some("sudo zypper install git"),
        "brew" => Some("brew install git"),
        _ => None,
    }
    .map(str::to_owned);
    checks.push(check(
        "git",
        "Git",
        if git == "missing" {
            if package_manager == "brew" {
                RemoteCheckState::InstallAvailable
            } else {
                RemoteCheckState::ManualActionRequired
            }
        } else {
            RemoteCheckState::Ready
        },
        git,
        git_instructions,
    ));
    let codex = values.get("codex").map(String::as_str).unwrap_or("missing");
    checks.push(check(
        "codex",
        "Codex",
        if codex == "missing" {
            RemoteCheckState::InstallAvailable
        } else {
            RemoteCheckState::Ready
        },
        codex,
        None,
    ));
    let persistence = values
        .get("persistence")
        .map(String::as_str)
        .unwrap_or("unknown");
    let persistent = os != "Linux" || persistence.eq_ignore_ascii_case("yes");
    checks.push(check(
        "persistence",
        "Automatic Startup",
        if persistent {
            RemoteCheckState::Ready
        } else {
            RemoteCheckState::ManualActionRequired
        },
        if persistent {
            "Available"
        } else {
            "Enable systemd lingering for this user; Ditch never runs sudo automatically"
        },
        None,
    ));
    let codex_auth = values
        .get("codex_auth")
        .map(String::as_str)
        .unwrap_or("unavailable");
    checks.push(check(
        "codex_auth",
        "Codex Authentication",
        match codex_auth {
            "ready" => RemoteCheckState::Ready,
            "required" => RemoteCheckState::AuthenticationRequired,
            _ => RemoteCheckState::ManualActionRequired,
        },
        codex_auth,
        None,
    ));
    let service = values
        .get("service")
        .map(String::as_str)
        .unwrap_or("unavailable");
    checks.push(check(
        "service",
        "Background Service",
        if service == "unavailable" {
            RemoteCheckState::ManualActionRequired
        } else {
            RemoteCheckState::Ready
        },
        service,
        None,
    ));

    let mut machine_id = None;
    let mut daemon_epoch = None;
    if exact {
        connections.disconnect(alias);
        if let Ok(ServerResponse::RuntimeStatus(status)) =
            connections.request(alias, ClientRequest::RuntimeStatus)
        {
            daemon_epoch = Some(status.instance_id);
        }
        let identity_status =
            match connections.request(alias, ClientRequest::EnsureRemoteMachineIdentity) {
                Ok(ServerResponse::RemoteControlStatus(status)) => Some(status),
                _ => match connections.request(alias, ClientRequest::RemoteControlStatus) {
                    Ok(ServerResponse::RemoteControlStatus(status)) => Some(status),
                    _ => None,
                },
            };
        if let Some(status) = identity_status {
            machine_id = status.machine_id;
        }
    }
    let ready = desktop_project_setup_ready(
        supported,
        exact,
        git != "missing",
        codex != "missing",
        codex_auth == "ready",
        service != "unavailable",
    );
    Ok(RemoteSetupStatus {
        ssh_host_alias: alias.to_owned(),
        remote_machine_id: machine_id,
        ready,
        connection_state: if exact { "connected" } else { "setup_required" }.into(),
        home_directory: values.get("home").cloned(),
        checks,
        handshake: RemoteRuntimeHandshake {
            client_version: env!("CARGO_PKG_VERSION").into(),
            server_version,
            protocol_version: server_protocol.unwrap_or(0),
            build_identifier: runtime_json
                .as_ref()
                .and_then(|value| value.get("build_identifier"))
                .and_then(|value| value.as_str())
                .map(str::to_owned),
            daemon_epoch,
            os: Some(os),
            architecture: Some(arch),
        },
        host_key_fingerprint: None,
        last_error: None,
    })
}

fn desktop_project_setup_ready(
    supported: bool,
    exact_runtime: bool,
    git_ready: bool,
    codex_ready: bool,
    codex_authenticated: bool,
    service_ready: bool,
) -> bool {
    supported && exact_runtime && git_ready && codex_ready && codex_authenticated && service_ready
}

/// Fixed user-local Codex setup action using OpenAI's current standalone
/// installer. Local Codex credentials and CODEX_HOME are never transferred.
pub fn install_remote_codex(
    connections: &RemoteConnectionManager,
    alias: &str,
) -> Result<RemoteSetupStatus, SshError> {
    observe("remote_codex_install_started", alias, None);
    const SCRIPT: &str = r#"set -eu
command -v curl >/dev/null 2>&1 || { echo 'curl is required to install Codex' >&2; exit 61; }
installer=$(mktemp "${TMPDIR:-/tmp}/ditch-codex-install.XXXXXX")
trap 'rm -f "$installer"' EXIT HUP INT TERM
curl --proto '=https' --tlsv1.2 -fsSL https://chatgpt.com/codex/install.sh -o "$installer"
chmod 700 "$installer"
sh "$installer"
[ -x "$HOME/.local/bin/codex" ] || command -v codex >/dev/null 2>&1
"#;
    let resolved = resolve_host(alias)?;
    let (output, _) =
        run_fixed_admin_authenticated(alias, SCRIPT, connections.password_for(&resolved), false)?;
    if !output.status.success() {
        return Err(SshError::Failed(
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4096)
                .collect(),
        ));
    }
    check_setup(connections, alias, None, false, false)
}

/// Homebrew is the only v1 automatic Git path that is predictably user-owned.
/// Linux package managers remain explicit manual instructions because Ditch
/// never silently invokes sudo or handles sudo credentials.
pub fn install_remote_git(
    connections: &RemoteConnectionManager,
    alias: &str,
) -> Result<RemoteSetupStatus, SshError> {
    observe("remote_git_install_started", alias, None);
    const SCRIPT: &str = r#"set -eu
command -v brew >/dev/null 2>&1 || { echo 'Automatic Git installation is only available with Homebrew; use the setup instructions for this host.' >&2; exit 62; }
brew install git
git --version
"#;
    let resolved = resolve_host(alias)?;
    let (output, _) =
        run_fixed_admin_authenticated(alias, SCRIPT, connections.password_for(&resolved), false)?;
    if !output.status.success() {
        return Err(SshError::Failed(
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4096)
                .collect(),
        ));
    }
    check_setup(connections, alias, None, false, false)
}

pub fn install_remote_runtime(
    connections: &RemoteConnectionManager,
    alias: &str,
) -> Result<RemoteSetupStatus, SshError> {
    observe("remote_runtime_install_started", alias, None);
    let resolved = resolve_host(alias)?;
    let (preflight, _) = run_fixed_admin_authenticated(
        alias,
        PREFLIGHT_SCRIPT,
        connections.password_for(&resolved),
        false,
    )?;
    if !preflight.status.success() {
        return Err(SshError::Failed(
            "SSH authentication is required before installing the runtime".into(),
        ));
    }
    let values = parse_preflight(&preflight.stdout);
    let target = remote_target(
        values.get("os").map(String::as_str).unwrap_or(""),
        values.get("arch").map(String::as_str).unwrap_or(""),
    )?;
    if let Ok(ServerResponse::RuntimeStatus(status)) =
        connections.request(alias, ClientRequest::RuntimeStatus)
        && status.active_session_count > 0
    {
        return Err(SshError::Failed(format!(
            "Remote runtime update required, but {} agent(s) are still running. Their work will not be interrupted.",
            status.active_session_count
        )));
    }

    let artifact = locate_artifact(target)?;
    let checksum = format!("{:x}", Sha256::digest(fs::read(&artifact)?));
    verify_artifact_metadata(&artifact, target, &checksum)?;
    let remote_upload = format!(
        "/tmp/.ditch-runtime-{}-{}",
        env!("CARGO_PKG_VERSION"),
        Uuid::new_v4()
    );
    upload_artifact(connections, &resolved, &artifact, &remote_upload)?;

    let mut command = base_ssh_command(alias)?;
    command.args([
        "sh",
        "-s",
        "--",
        env!("CARGO_PKG_VERSION"),
        target,
        &checksum,
        &remote_upload,
    ]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (_prompts, guard) =
        configure_askpass(&mut command, connections.password_for(&resolved), false)?;
    let mut child = command
        .spawn()
        .map_err(|error| SshError::Failed(error.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(INSTALL_SCRIPT.as_bytes())?;
    }
    let output = ditch_ssh::wait_output(child, Duration::from_secs(90));
    drop(guard);
    let output = output?;
    if !output.status.success() {
        let _ = rollback_remote_runtime(connections, alias);
        return Err(SshError::Failed(
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4096)
                .collect(),
        ));
    }
    connections.disconnect(alias);
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(status) = check_setup(connections, alias, None, false, false)
            && status
                .checks
                .iter()
                .any(|check| check.key == "runtime" && check.state == RemoteCheckState::Ready)
        {
            return Ok(status);
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = rollback_remote_runtime(connections, alias);
    Err(SshError::Failed(
        "runtime installed but its health handshake did not become ready".into(),
    ))
}

fn rollback_remote_runtime(
    connections: &RemoteConnectionManager,
    alias: &str,
) -> Result<(), SshError> {
    const SCRIPT: &str = r#"set -eu
root="$HOME/.ditch"
previous=$(cat "$root/run/previous-runtime" 2>/dev/null || true)
[ -n "$previous" ] || exit 0
ln -sfn "$previous" "$root/bin/current.next"
mv -f "$root/bin/current.next" "$root/bin/current"
case "$(uname -s)" in
  Linux) systemctl --user restart the-ditch.service ;;
  Darwin)
    uid=$(id -u)
    launchctl kickstart -k "user/$uid/dev.theditch.runtime.remote"
    ;;
esac
"#;
    let resolved = resolve_host(alias)?;
    let (output, _) =
        run_fixed_admin_authenticated(alias, SCRIPT, connections.password_for(&resolved), false)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SshError::Failed("runtime rollback failed".into()))
    }
}

fn remote_target(os: &str, arch: &str) -> Result<&'static str, SshError> {
    match (os, arch) {
        ("Linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("Linux", "aarch64" | "arm64") => Ok("aarch64-unknown-linux-gnu"),
        ("Darwin", "arm64") => Ok("aarch64-apple-darwin"),
        ("Darwin", "x86_64") => Ok("x86_64-apple-darwin"),
        _ => Err(SshError::UnsupportedConfig(format!(
            "unsupported remote platform {os} {arch}"
        ))),
    }
}

fn locate_artifact(target: &str) -> Result<PathBuf, SshError> {
    let mut candidates = Vec::new();
    if let Some(directory) = std::env::var_os("DITCH_REMOTE_ARTIFACT_DIR") {
        candidates.push(PathBuf::from(directory).join(format!("ditchd-{target}")));
    }
    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        candidates.push(directory.join(format!("ditchd-remote-{target}")));
        candidates.push(
            directory
                .parent()
                .unwrap_or(directory)
                .join("Resources/RemoteRuntimes")
                .join(format!("ditchd-{target}")),
        );
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("ditchd must be in a workspace");
    candidates.push(workspace.join(format!("target/{target}/release/ditchd")));
    if target == host_target() {
        candidates.push(workspace.join("target/release/ditchd"));
        candidates.push(workspace.join("target/debug/ditchd"));
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            SshError::Failed(format!(
                "No exact-version runtime artifact is available for {target}. Build it with scripts/build-remote-artifacts."
            ))
        })
}

fn verify_artifact_metadata(artifact: &Path, target: &str, checksum: &str) -> Result<(), SshError> {
    let manifest = artifact
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("remote-artifacts.json");
    if manifest.is_file() {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(manifest)?).map_err(|error| {
                SshError::Failed(format!("invalid remote artifact manifest: {error}"))
            })?;
        if value.get("version").and_then(|value| value.as_str()) != Some(env!("CARGO_PKG_VERSION"))
        {
            return Err(SshError::Failed(
                "remote artifact manifest version does not match this Ditch build".into(),
            ));
        }
        let artifact_name = artifact
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let valid = value
            .get("artifacts")
            .and_then(|value| value.as_array())
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry.get("target").and_then(|value| value.as_str()) == Some(target)
                        && entry.get("artifact").and_then(|value| value.as_str())
                            == Some(artifact_name)
                        && entry.get("sha256").and_then(|value| value.as_str()) == Some(checksum)
                })
            });
        if !valid {
            return Err(SshError::Failed(
                "remote artifact checksum is not present in the exact-version manifest".into(),
            ));
        }
    }
    Ok(())
}

fn host_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        _ => "unsupported",
    }
}

fn upload_artifact(
    connections: &RemoteConnectionManager,
    resolved: &ResolvedSshHost,
    artifact: &Path,
    remote_upload: &str,
) -> Result<(), SshError> {
    let mut command = Command::new("/usr/bin/scp");
    command.args([
        "-q",
        "-o",
        "PreferredAuthentications=publickey,password",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "ConnectTimeout=15",
        "--",
    ]);
    command.arg(artifact);
    command.arg(format!("{}:{remote_upload}", resolved.alias));
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    let (_prompts, guard) =
        configure_askpass(&mut command, connections.password_for(resolved), false)?;
    let child = command
        .spawn()
        .map_err(|error| SshError::Failed(error.to_string()))?;
    let output = ditch_ssh::wait_output(child, Duration::from_secs(120));
    drop(guard);
    let output = output?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SshError::Failed(
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4096)
                .collect(),
        ))
    }
}

fn parse_preflight(bytes: &[u8]) -> HashMap<String, String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn check(
    key: &str,
    label: &str,
    state: RemoteCheckState,
    detail: &str,
    technical_detail: Option<String>,
) -> RemoteSetupCheck {
    RemoteSetupCheck {
        key: key.into(),
        label: label.into(),
        state,
        detail: detail.into(),
        technical_detail,
    }
}

fn empty_handshake() -> RemoteRuntimeHandshake {
    RemoteRuntimeHandshake {
        client_version: env!("CARGO_PKG_VERSION").into(),
        server_version: None,
        protocol_version: ditch_protocol::REMOTE_RUNTIME_PROTOCOL_VERSION,
        build_identifier: None,
        daemon_epoch: None,
        os: None,
        architecture: None,
    }
}

fn failed_setup(alias: &str, message: &str, detail: Option<String>) -> RemoteSetupStatus {
    RemoteSetupStatus {
        ssh_host_alias: alias.into(),
        remote_machine_id: None,
        ready: false,
        connection_state: "offline".into(),
        home_directory: None,
        checks: vec![check(
            "ssh",
            "SSH",
            RemoteCheckState::Failed,
            message,
            detail.clone(),
        )],
        handshake: empty_handshake(),
        host_key_fingerprint: None,
        last_error: detail.or_else(|| Some(message.into())),
    }
}

fn fingerprint(prompt: &str) -> Option<String> {
    let start = prompt.find("SHA256:")?;
    Some(
        prompt[start..]
            .split_whitespace()
            .next()?
            .trim_matches(['.', '\'', '"'])
            .to_owned(),
    )
}

pub fn list_remote_directory(
    connections: &RemoteConnectionManager,
    alias: &str,
    absolute_path: &str,
) -> Result<RemoteDirectory, SshError> {
    if !absolute_path.starts_with('/') || absolute_path.len() > 4096 || absolute_path.contains('\0')
    {
        return Err(SshError::Failed(
            "remote path must be an absolute path".into(),
        ));
    }
    match connections.request(
        alias,
        ClientRequest::ListFilesystemDirectory {
            absolute_path: absolute_path.to_owned(),
        },
    )? {
        ServerResponse::RemoteDirectory(mut directory) => {
            directory.ssh_host_alias = alias.to_owned();
            Ok(directory)
        }
        ServerResponse::Error(error) => Err(SshError::Failed(error.message)),
        _ => Err(SshError::Failed(
            "remote daemon returned an unexpected directory response".into(),
        )),
    }
}

pub fn wrap_remote_project(mut project: Project, alias: &str, machine_id: Uuid) -> Project {
    project.execution_target = ProjectExecutionTarget::Remote {
        remote_machine_id: machine_id,
        ssh_host_alias: alias.to_owned(),
    };
    project
}

pub fn remote_alias(project: &Project) -> Option<&str> {
    match &project.execution_target {
        ProjectExecutionTarget::Remote { ssh_host_alias, .. } => Some(ssh_host_alias),
        ProjectExecutionTarget::Local => None,
    }
}

pub fn create_remote_project(
    connections: &RemoteConnectionManager,
    alias: &str,
    machine_id: Uuid,
    name: String,
    root: String,
    git_policy: ProjectGitPolicy,
) -> Result<Project, SshError> {
    match connections.request(
        alias,
        ClientRequest::CreateProject {
            name,
            root,
            git_policy,
        },
    )? {
        ServerResponse::ProjectCreated(project) => {
            Ok(wrap_remote_project(project, alias, machine_id))
        }
        ServerResponse::Error(error) => Err(SshError::Failed(error.message)),
        _ => Err(SshError::Failed(
            "remote daemon returned an unexpected project response".into(),
        )),
    }
}

const INSTALL_SCRIPT: &str = r#"set -eu
version=$1
target=$2
expected=$3
upload=$4
umask 077
root="$HOME/.ditch"
mkdir -p "$root/bin/versions/$version/$target" "$root/state" "$root/run" "$root/logs" "$root/identity"
chmod 700 "$root" "$root/bin" "$root/bin/versions" "$root/state" "$root/run" "$root/logs" "$root/identity"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$upload" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$upload" | awk '{print $1}')
else
  echo 'No SHA-256 utility is available' >&2
  exit 41
fi
[ "$actual" = "$expected" ] || { echo 'Runtime artifact checksum mismatch' >&2; rm -f "$upload"; exit 42; }
destination="$root/bin/versions/$version/$target/ditchd"
temporary="$destination.tmp"
mv "$upload" "$temporary"
chmod 700 "$temporary"
mv "$temporary" "$destination"
[ ! -L "$root/bin/current" ] || readlink "$root/bin/current" > "$root/run/previous-runtime"
ln -sfn "versions/$version/$target" "$root/bin/current.next"
mv -f "$root/bin/current.next" "$root/bin/current"
case "$(uname -s)" in
  Linux)
    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
      mkdir -p "$HOME/.config/systemd/user"
      cat > "$HOME/.config/systemd/user/the-ditch.service" <<'UNIT'
[Unit]
Description=The Ditch Remote Runtime
After=network-online.target
[Service]
Type=simple
ExecStart=%h/.ditch/bin/current/ditchd daemon --remote
Restart=on-failure
RestartSec=2
StandardOutput=append:%h/.ditch/logs/runtime.log
StandardError=append:%h/.ditch/logs/runtime.log
NoNewPrivileges=true
[Install]
WantedBy=default.target
UNIT
      systemctl --user daemon-reload
      systemctl --user enable --now the-ditch.service
    else
      echo 'A persistent user service manager is unavailable' >&2
      exit 43
    fi
    ;;
  Darwin)
    uid=$(id -u)
    mkdir -p "$HOME/Library/LaunchAgents"
    cat > "$HOME/Library/LaunchAgents/dev.theditch.runtime.remote.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.theditch.runtime.remote</string>
<key>ProgramArguments</key><array><string>$HOME/.ditch/bin/current/ditchd</string><string>daemon</string><string>--remote</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>StandardOutPath</key><string>$HOME/.ditch/logs/runtime.log</string>
<key>StandardErrorPath</key><string>$HOME/.ditch/logs/runtime.log</string>
</dict></plist>
PLIST
    chmod 600 "$HOME/Library/LaunchAgents/dev.theditch.runtime.remote.plist"
    launchctl bootout "user/$uid/dev.theditch.runtime.remote" >/dev/null 2>&1 || true
    launchctl bootstrap "user/$uid" "$HOME/Library/LaunchAgents/dev.theditch.runtime.remote.plist" || {
      echo 'launchd user domain is unavailable without a login session' >&2
      exit 44
    }
    launchctl enable "user/$uid/dev.theditch.runtime.remote"
    ;;
  *)
    echo 'Unsupported remote OS' >&2
    exit 45
    ;;
esac
"#;

pub fn observe(event: &str, alias: &str, detail: Option<&str>) {
    eprintln!(
        "{}",
        serde_json::json!({
            "event": event,
            "ssh_host_alias": alias,
            "detail": detail,
            "timestamp": chrono::Utc::now(),
        })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_taken_from_the_openssh_prompt() {
        assert_eq!(
            fingerprint(
                "The authenticity of host x can't be established. ED25519 key fingerprint is SHA256:abc123. Continue?"
            ),
            Some("SHA256:abc123".into())
        );
    }

    #[test]
    fn desktop_project_setup_does_not_require_cloud_or_lingering() {
        assert!(desktop_project_setup_ready(
            true, true, true, true, true, true
        ));
    }

    #[test]
    fn remote_project_keeps_remote_identity_separate_from_path() {
        let remote_id = Uuid::new_v4();
        let project = wrap_remote_project(Project::new("API", "/srv/api"), "dev-box", remote_id);
        assert_eq!(project.root, PathBuf::from("/srv/api"));
        assert!(project.is_remote());
        assert_eq!(remote_alias(&project), Some("dev-box"));
    }
}
