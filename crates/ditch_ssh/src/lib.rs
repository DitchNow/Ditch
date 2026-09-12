//! System-OpenSSH control plane for SSH-backed Ditch machines.
//!
//! This crate deliberately does not implement SSH, expose a generic remote
//! shell, or own project sessions.  It resolves the user's OpenSSH config and
//! builds the tightly-scoped administrative/bridge commands used by ditchd.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::io::{BufRead, BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use thiserror::Error;

pub const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const ADMIN_TIMEOUT: Duration = Duration::from_secs(45);
pub const SSH_PASSWORD_KEYCHAIN_SERVICE: &str = "dev.theditch.ssh-password.v1";
const ASKPASS_SOCKET_NAME: &str = "s";
// macOS has the smallest supported sockaddr_un.sun_path (104 bytes, including
// its terminating NUL). Leave a little headroom for platform encoding details.
const SAFE_UNIX_SOCKET_PATH_BYTES: usize = 100;

#[derive(Debug, Error)]
pub enum SshError {
    #[error("OpenSSH is unavailable: {0}")]
    OpenSshUnavailable(String),
    #[error("SSH configuration is invalid: {0}")]
    InvalidConfig(String),
    #[error("unsupported SSH configuration: {0}")]
    UnsupportedConfig(String),
    #[error("SSH operation timed out")]
    Timeout,
    #[error("SSH operation failed: {0}")]
    Failed(String),
    #[error("filesystem error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Clone, Debug, Default)]
pub struct AskpassPrompts(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl AskpassPrompts {
    pub fn values(&self) -> Vec<String> {
        self.0.lock().expect("askpass prompt lock poisoned").clone()
    }

    pub fn host_key_prompt(&self) -> Option<String> {
        self.values().into_iter().find(|prompt| {
            let lower = prompt.to_ascii_lowercase();
            lower.contains("authenticity of host") || lower.contains("fingerprint")
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SshHostSummary {
    pub alias: String,
    pub config_path: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedSshHost {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_files: Vec<String>,
    pub preferred_authentications: Vec<String>,
}

impl ResolvedSshHost {
    pub fn credential_id(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.alias, self.hostname, self.user, self.port
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostKeyState {
    Trusted,
    Unknown { fingerprint: String, prompt: String },
    Changed { detail: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NewSshHost {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: u16,
    pub identity_file: Option<String>,
}

pub fn default_config_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ssh/config")
}

/// Parses only concrete selector aliases. OpenSSH remains authoritative for
/// all effective values and precedence through [`resolve_host`].
pub fn discover_hosts(config: &Path) -> Result<Vec<SshHostSummary>, SshError> {
    let mut aliases = BTreeSet::new();
    let mut visited = HashSet::new();
    discover_file(config, &mut visited, &mut aliases)?;
    Ok(aliases
        .into_iter()
        .map(|alias| SshHostSummary {
            alias,
            config_path: config.to_string_lossy().into_owned(),
        })
        .collect())
}

fn discover_file(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    aliases: &mut BTreeSet<String>,
) -> Result<(), SshError> {
    let path = expand_home(path);
    let canonical = path.canonicalize().unwrap_or(path.clone());
    if !visited.insert(canonical) || !path.exists() {
        return Ok(());
    }
    let contents = fs::read_to_string(&path)?;
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keyword, value)) = split_directive(line) else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            for alias in split_words(value) {
                if concrete_alias(&alias) {
                    aliases.insert(alias);
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") {
            for include in split_words(value) {
                for included in expand_include(&path, &include) {
                    discover_file(&included, visited, aliases)?;
                }
            }
        }
    }
    Ok(())
}

fn split_directive(line: &str) -> Option<(&str, &str)> {
    let index = line.find(|character: char| character.is_whitespace() || character == '=')?;
    let (key, rest) = line.split_at(index);
    let value =
        rest.trim_start_matches(|character: char| character.is_whitespace() || character == '=');
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

fn split_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            word.push(character);
            escaped = false;
        } else if character == '\\' && quote != Some('\'') {
            escaped = true;
        } else if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            } else {
                word.push(character);
            }
        } else if character.is_whitespace() && quote.is_none() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(character);
        }
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn concrete_alias(alias: &str) -> bool {
    !alias.is_empty()
        && !alias.starts_with('!')
        && !alias
            .bytes()
            .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b']'))
        && alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn expand_include(config: &Path, pattern: &str) -> Vec<PathBuf> {
    let expanded = expand_home(Path::new(pattern));
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        config
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(expanded)
    };
    let Some(file_name) = absolute.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    if !file_name.contains(['*', '?']) {
        return vec![absolute];
    }
    let parent = absolute.parent().unwrap_or_else(|| Path::new("."));
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matches = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| wildcard_match(file_name, name))
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let (mut pattern_index, mut value_index, mut star, mut checkpoint) = (0, 0, None, 0);
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            checkpoint = value_index;
            pattern_index += 1;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            checkpoint += 1;
            value_index = checkpoint;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if (text == "~" || text.starts_with("~/"))
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(text.trim_start_matches("~/"));
    }
    path.to_path_buf()
}

pub fn resolve_host(alias: &str) -> Result<ResolvedSshHost, SshError> {
    if !concrete_alias(alias) {
        return Err(SshError::InvalidConfig(
            "host alias contains unsupported characters".into(),
        ));
    }
    let output = Command::new("/usr/bin/ssh")
        .args(["-G", "--", alias])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| SshError::OpenSshUnavailable(error.to_string()))?;
    if !output.status.success() {
        return Err(SshError::InvalidConfig(redacted_output(&output)));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    resolved_from_effective_config(alias, &text)
}

fn resolved_from_effective_config(alias: &str, text: &str) -> Result<ResolvedSshHost, SshError> {
    let mut hostname = None;
    let mut user = None;
    let mut port = None;
    let mut identity_files = Vec::new();
    let mut preferred = Vec::new();
    let mut proxy_jump = None;
    let mut proxy_command = None;
    let mut keyboard_interactive = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "hostname" => hostname = Some(value.to_owned()),
            "user" => user = Some(value.to_owned()),
            "port" => port = value.parse::<u16>().ok(),
            "identityfile" => identity_files.push(value.to_owned()),
            "preferredauthentications" => preferred = value.split(',').map(str::to_owned).collect(),
            "proxyjump" => proxy_jump = Some(value.to_owned()),
            "proxycommand" => proxy_command = Some(value.to_owned()),
            "kbdinteractiveauthentication" => keyboard_interactive = Some(value.to_owned()),
            _ => {}
        }
    }
    if proxy_jump.as_deref().is_some_and(|value| value != "none")
        || proxy_command
            .as_deref()
            .is_some_and(|value| value != "none")
    {
        return Err(SshError::UnsupportedConfig(
            "ProxyJump and ProxyCommand hosts aren't currently supported by The Ditch.".into(),
        ));
    }
    if !preferred.is_empty()
        && preferred
            .iter()
            .all(|value| value == "keyboard-interactive" || value == "hostbased")
    {
        return Err(SshError::UnsupportedConfig(
            "This host only allows keyboard-interactive authentication, which isn't supported yet."
                .into(),
        ));
    }
    if keyboard_interactive.as_deref() == Some("yes")
        && preferred == ["keyboard-interactive".to_owned()]
    {
        return Err(SshError::UnsupportedConfig(
            "Keyboard-interactive authentication isn't supported yet.".into(),
        ));
    }
    Ok(ResolvedSshHost {
        alias: alias.to_owned(),
        hostname: hostname.ok_or_else(|| SshError::InvalidConfig("missing HostName".into()))?,
        user: user.ok_or_else(|| SshError::InvalidConfig("missing User".into()))?,
        port: port.unwrap_or(22),
        identity_files,
        preferred_authentications: preferred,
    })
}

pub fn base_ssh_command(alias: &str) -> Result<Command, SshError> {
    let _ = resolve_host(alias)?;
    let mut command = Command::new("/usr/bin/ssh");
    command.args([
        "-o",
        "PreferredAuthentications=publickey,password",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "NumberOfPasswordPrompts=1",
        "-o",
        "ConnectTimeout=15",
        "-o",
        "ServerAliveInterval=10",
        "-o",
        "ServerAliveCountMax=3",
        "--",
        alias,
    ]);
    Ok(command)
}

pub fn configure_askpass(
    command: &mut Command,
    password: Option<Vec<u8>>,
    trust_unknown_host: bool,
) -> Result<(AskpassPrompts, AskpassGuard), SshError> {
    let token = uuid::Uuid::new_v4().simple().to_string();
    let directory = askpass_directory_for(&std::env::temp_dir(), &token);
    fs::create_dir(&directory)?;
    if let Err(error) = fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)) {
        let _ = fs::remove_dir(&directory);
        return Err(error.into());
    }
    let socket_path = directory.join(ASKPASS_SOCKET_NAME);
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = fs::remove_dir(&directory);
            return Err(error.into());
        }
    };
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let prompts = AskpassPrompts::default();
    let captured = prompts.0.clone();
    let broker_path = socket_path.clone();
    let thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut password = password;
        while Instant::now() < deadline && broker_path.exists() {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut prompt = String::new();
                    let _ = BufReader::new(&mut stream).read_line(&mut prompt);
                    let prompt = prompt.trim_end().chars().take(4096).collect::<String>();
                    captured
                        .lock()
                        .expect("askpass prompt lock poisoned")
                        .push(prompt.clone());
                    let lower = prompt.to_ascii_lowercase();
                    let response: &[u8] = if lower.contains("authenticity of host")
                        || lower.contains("fingerprint")
                    {
                        if trust_unknown_host { b"yes" } else { b"no" }
                    } else if lower.contains("password") && !lower.contains("passphrase") {
                        password.as_deref().unwrap_or_default()
                    } else {
                        // Key passphrases stay with the user's SSH agent. Ditch
                        // never mistakes an SSH account password for a key secret.
                        b""
                    };
                    let _ = stream.write_all(response);
                    let _ = stream.write_all(b"\n");
                    let _ = stream.flush();
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
        if let Some(secret) = password.as_mut() {
            secret.fill(0);
        }
    });
    let helper = askpass_helper()?;
    command.env("SSH_ASKPASS", helper);
    command.env("SSH_ASKPASS_REQUIRE", "force");
    command.env("DISPLAY", "ditch-askpass");
    command.env("DITCH_SSH_ASKPASS_SOCKET", &socket_path);
    Ok((
        prompts,
        AskpassGuard {
            directory,
            thread: Some(thread),
        },
    ))
}

fn askpass_directory_for(temp_dir: &Path, token: &str) -> PathBuf {
    let name = format!("ditch-ap-{token}");
    let candidate = temp_dir.join(&name);
    if candidate
        .join(ASKPASS_SOCKET_NAME)
        .as_os_str()
        .as_bytes()
        .len()
        < SAFE_UNIX_SOCKET_PATH_BYTES
    {
        candidate
    } else {
        Path::new("/tmp").join(name)
    }
}

fn askpass_helper() -> Result<PathBuf, SshError> {
    if let Some(value) = std::env::var_os("DITCH_SSH_ASKPASS_HELPER") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
    }
    let current = std::env::current_exe()?;
    let candidate = current
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("ditch_cli");
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(SshError::Failed(
            "the bundled SSH credential helper is unavailable".into(),
        ))
    }
}

pub struct AskpassGuard {
    directory: PathBuf,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for AskpassGuard {
    fn drop(&mut self) {
        let socket = self.directory.join(ASKPASS_SOCKET_NAME);
        let _ = fs::remove_file(socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

pub fn askpass_main() -> Result<(), SshError> {
    let socket = std::env::var_os("DITCH_SSH_ASKPASS_SOCKET")
        .map(PathBuf::from)
        .ok_or_else(|| SshError::Failed("credential broker unavailable".into()))?;
    let prompt = std::env::args().nth(1).unwrap_or_default();
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(prompt.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    io::stdout().write_all(&response)?;
    response.fill(0);
    Ok(())
}

pub fn bridge_command(alias: &str) -> Result<Command, SshError> {
    let mut command = base_ssh_command(alias)?;
    command.arg("~/.ditch/bin/current/ditchd");
    command.arg("bridge");
    command.arg("--stdio");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

/// Runs a fixed administrative script. Callers may pass data only through
/// stdin/environment understood by that script; no user value is interpolated
/// into the script source.
pub fn run_fixed_admin(alias: &str, script: &str, input: &[u8]) -> Result<Output, SshError> {
    let mut command = base_ssh_command(alias)?;
    command.args(["sh", "-s", "--"]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| SshError::Failed(error.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
        stdin.write_all(input)?;
    }
    wait_output(child, ADMIN_TIMEOUT)
}

pub fn run_fixed_admin_authenticated(
    alias: &str,
    script: &str,
    password: Option<Vec<u8>>,
    trust_unknown_host: bool,
) -> Result<(Output, AskpassPrompts), SshError> {
    let mut command = base_ssh_command(alias)?;
    command.args(["sh", "-s", "--"]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (prompts, guard) = configure_askpass(&mut command, password, trust_unknown_host)?;
    let mut child = command
        .spawn()
        .map_err(|error| SshError::Failed(error.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes())?;
    }
    let output = wait_output(child, ADMIN_TIMEOUT);
    drop(guard);
    output.map(|output| (output, prompts))
}

pub fn wait_output(mut child: std::process::Child, timeout: Duration) -> Result<Output, SshError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(SshError::Io),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SshError::Timeout);
            }
            Err(error) => return Err(SshError::Io(error)),
        }
    }
}

pub fn render_config_entry(host: &NewSshHost) -> Result<String, SshError> {
    if !concrete_alias(&host.alias)
        || host.hostname.is_empty()
        || host.user.is_empty()
        || host.port == 0
        || !host.hostname.bytes().all(safe_host_character)
        || !host.user.bytes().all(safe_user_character)
    {
        return Err(SshError::InvalidConfig(
            "host fields contain unsupported characters".into(),
        ));
    }
    let mut value = format!(
        "Host {}\n    HostName {}\n    User {}\n    Port {}\n",
        host.alias, host.hostname, host.user, host.port
    );
    if let Some(identity) = host
        .identity_file
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        if identity.is_empty()
            || !identity.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'~')
            })
        {
            return Err(SshError::InvalidConfig("identity file is invalid".into()));
        }
        value.push_str(&format!("    IdentityFile {}\n", identity.trim()));
    }
    Ok(value)
}

pub fn append_config_entry(config: &Path, host: &NewSshHost) -> Result<(), SshError> {
    let entry = render_config_entry(host)?;
    if discover_hosts(config)?
        .iter()
        .any(|candidate| candidate.alias == host.alias)
    {
        return Err(SshError::InvalidConfig(
            "that SSH alias already exists".into(),
        ));
    }
    let parent = config
        .parent()
        .ok_or_else(|| SshError::InvalidConfig("config has no parent".into()))?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    if config.exists() {
        let backup = config.with_extension(format!("ditch-backup-{}", uuid::Uuid::new_v4()));
        fs::copy(config, backup)?;
    }
    let needs_newline = fs::read(config)
        .ok()
        .and_then(|bytes| bytes.last().copied())
        .is_some_and(|byte| byte != b'\n');
    let mut file = OpenOptions::new().create(true).append(true).open(config)?;
    if needs_newline {
        file.write_all(b"\n")?;
    }
    file.write_all(b"\n# Added by The Ditch\n")?;
    file.write_all(entry.as_bytes())?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(config, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn safe_host_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b':' | b'_' | b'-')
}
fn safe_user_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

fn redacted_output(output: &Output) -> String {
    let text = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    String::from_utf8_lossy(text)
        .trim()
        .chars()
        .take(2048)
        .collect()
}

#[cfg(target_os = "macos")]
pub fn load_password(credential_id: &str) -> Result<Option<Vec<u8>>, SshError> {
    match security_framework::passwords::get_generic_password(
        SSH_PASSWORD_KEYCHAIN_SERVICE,
        credential_id,
    ) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code() == -25300 => Ok(None),
        Err(error) => Err(SshError::Failed(format!("Keychain unavailable: {error}"))),
    }
}

#[cfg(target_os = "macos")]
pub fn save_password(credential_id: &str, password: &[u8]) -> Result<(), SshError> {
    security_framework::passwords::set_generic_password(
        SSH_PASSWORD_KEYCHAIN_SERVICE,
        credential_id,
        password,
    )
    .map_err(|error| SshError::Failed(format!("Keychain unavailable: {error}")))
}

#[cfg(target_os = "macos")]
pub fn delete_password(credential_id: &str) -> Result<(), SshError> {
    match security_framework::passwords::delete_generic_password(
        SSH_PASSWORD_KEYCHAIN_SERVICE,
        credential_id,
    ) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(error) => Err(SshError::Failed(format!("Keychain unavailable: {error}"))),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn load_password(_credential_id: &str) -> Result<Option<Vec<u8>>, SshError> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
pub fn save_password(_credential_id: &str, _password: &[u8]) -> Result<(), SshError> {
    Err(SshError::UnsupportedConfig(
        "SSH password storage requires macOS Keychain".into(),
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn delete_password(_credential_id: &str) -> Result<(), SshError> {
    Ok(())
}

pub fn openssh_path() -> OsString {
    OsString::from("/usr/bin/ssh")
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temporary(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("ditch-ssh-{name}-{}", Uuid::new_v4()))
    }

    #[test]
    fn discovers_concrete_hosts_and_includes_without_wildcard_targets() {
        let root = temporary("config");
        fs::create_dir_all(root.join("conf.d")).unwrap();
        fs::create_dir_all(root.join("more configs")).unwrap();
        fs::write(
            root.join("config"),
            "# keep\nInclude conf.d/* \"more configs/extra\"\nHost *.corp\n  User ignored\nHost dev-box ubuntu_box\n",
        )
        .unwrap();
        fs::write(root.join("more configs/extra"), "Host spaced-include\n").unwrap();
        fs::write(
            root.join("conf.d/mac"),
            "Host mac-mini\n HostName 10.0.0.4\n",
        )
        .unwrap();
        let aliases = discover_hosts(&root.join("config"))
            .unwrap()
            .into_iter()
            .map(|host| host.alias)
            .collect::<Vec<_>>();
        assert_eq!(
            aliases,
            ["dev-box", "mac-mini", "spaced-include", "ubuntu_box"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_rendering_rejects_shell_metacharacters_and_never_contains_passwords() {
        let valid = NewSshHost {
            alias: "dev-box".into(),
            hostname: "203.0.113.10".into(),
            user: "mtn".into(),
            port: 22,
            identity_file: Some("~/.ssh/id_ed25519".into()),
        };
        let rendered = render_config_entry(&valid).unwrap();
        assert!(rendered.contains("Host dev-box"));
        assert!(!rendered.to_ascii_lowercase().contains("password"));
        let mut invalid = valid;
        invalid.alias = "dev;touch /tmp/pwned".into();
        assert!(render_config_entry(&invalid).is_err());
    }

    #[test]
    fn append_preserves_existing_bytes_and_creates_backup() {
        let root = temporary("append");
        fs::create_dir_all(&root).unwrap();
        let config = root.join("config");
        let original = "# personal comment\nHost old\n  HostName old.example\n";
        fs::write(&config, original).unwrap();
        append_config_entry(
            &config,
            &NewSshHost {
                alias: "new".into(),
                hostname: "new.example".into(),
                user: "dev".into(),
                port: 2222,
                identity_file: None,
            },
        )
        .unwrap();
        assert!(fs::read_to_string(&config).unwrap().starts_with(original));
        assert!(fs::read_dir(&root).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("config.ditch-backup-")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolved_config_rejects_proxy_and_keyboard_interactive_only_hosts() {
        let base = "hostname example.test\nuser dev\nport 22\n";
        for directive in [
            "proxyjump bastion\npreferredauthentications publickey\n",
            "proxycommand ssh relay\npreferredauthentications publickey\n",
            "kbdinteractiveauthentication yes\npreferredauthentications keyboard-interactive\n",
        ] {
            let error = resolved_from_effective_config("dev", &format!("{base}{directive}"))
                .expect_err("unsupported effective config should be rejected");
            assert!(matches!(error, SshError::UnsupportedConfig(_)));
        }
    }

    #[test]
    fn askpass_socket_falls_back_to_short_path_for_long_temp_directory() {
        let long_temp = Path::new("/tmp").join("x".repeat(256));
        let directory = askpass_directory_for(&long_temp, "0123456789abcdef0123456789abcdef");
        let socket = directory.join(ASKPASS_SOCKET_NAME);

        assert_eq!(directory.parent(), Some(Path::new("/tmp")));
        assert!(socket.as_os_str().as_bytes().len() < SAFE_UNIX_SOCKET_PATH_BYTES);
    }
}
