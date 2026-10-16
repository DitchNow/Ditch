//! Host-side, journaled runtime activation. The desktop only uploads a verified
//! executable; this helper owns manager detection, quiescence, and recovery.
use ditch_core::AppPaths;
use ditch_protocol::{ClientRequest, Envelope, RuntimeStatus, ServerResponse};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

const SERVICE: &str = "the-ditch.service";
const LABEL: &str = "dev.theditch.runtime.remote";
const MAINTENANCE: &str = "remote_maintenance_v1";

fn fail(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn build_id() -> &'static str {
    option_env!("DITCH_BUILD_IDENTIFIER").unwrap_or(env!("CARGO_PKG_VERSION"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Manager {
    Systemd,
    Launchd,
    Direct,
}

#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    manager: Manager,
    previous: Option<PathBuf>,
    replacement: PathBuf,
    previous_build: Option<String>,
    previous_edition: Option<String>,
    replacement_build: String,
    token: Uuid,
    old_instance: Option<Uuid>,
    phase: String,
}

fn journal_path(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join("run/runtime-upgrade.json")
}
fn manager_path(paths: &AppPaths) -> PathBuf {
    paths.data_dir.join("run/runtime-manager.json")
}

pub(super) fn initial_maintenance(paths: &AppPaths) -> Option<(Uuid, Instant)> {
    let journal: Journal = read_json(&journal_path(paths)).ok()?;
    (journal.replacement_build == build_id())
        .then(|| (journal.token, Instant::now() + Duration::from_secs(180)))
}

fn lock(paths: &AppPaths, exclusive: bool) -> io::Result<File> {
    fs::create_dir_all(paths.data_dir.join("run"))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(paths.data_dir.join("run/runtime-upgrade.lock"))?;
    let kind = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    if unsafe { libc::flock(file.as_raw_fd(), kind | libc::LOCK_NB) } != 0 {
        return Err(fail(
            "Remote runtime repair is in progress. Wait for it to finish.",
        ));
    }
    Ok(file)
}

pub(super) fn bootstrap_lock(paths: &AppPaths) -> io::Result<File> {
    let guard = lock(paths, false)?;
    if journal_path(paths).exists() {
        return Err(fail(
            "A remote upgrade was interrupted. Use Install / Repair to recover it.",
        ));
    }
    Ok(guard)
}

// A slow but live upgrade must not reopen admission merely because its lease
// elapsed. If the helper dies, flock releases and the lease can recover normally.
pub(super) fn upgrade_lock_held(paths: &AppPaths) -> bool {
    let Ok(file) = File::open(paths.data_dir.join("run/runtime-upgrade.lock")) else {
        return false;
    };
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let temporary = path.with_extension("next");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
    file.sync_all()?;
    fs::rename(temporary, path)?;
    File::open(path.parent().unwrap())?.sync_all()
}

fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fs::read(path)?).map_err(io::Error::other)
}

fn command(program: &str, args: &[&str]) -> io::Result<String> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let output = ditch_ssh::wait_output(child, Duration::from_secs(15))
        .map_err(|e| fail(format!("{program}: {e}")))?;
    if !output.status.success() {
        return Err(fail(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1500)
                .collect::<String>()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn rpc(paths: &AppPaths, body: ClientRequest) -> io::Result<ServerResponse> {
    // Never bootstrap here: status must describe the process already running.
    let mut stream = UnixStream::connect(&paths.socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = Envelope::new(body);
    serde_json::to_writer(&mut stream, &request).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream)
        .take(1024 * 1024)
        .read_line(&mut line)?;
    let response: Envelope<ServerResponse> =
        serde_json::from_str(&line).map_err(io::Error::other)?;
    if response.id != request.id {
        return Err(fail("Runtime response identity did not match."));
    }
    match response.body {
        ServerResponse::Error(error) => Err(fail(format!("{}: {}", error.code, error.message))),
        body => Ok(body),
    }
}

fn status(paths: &AppPaths) -> io::Result<Option<RuntimeStatus>> {
    match rpc(paths, ClientRequest::RuntimeStatus) {
        Ok(ServerResponse::RuntimeStatus(status)) => Ok(Some(status)),
        Ok(_) => Err(fail(
            "Unexpected runtime status response; nothing was stopped.",
        )),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(fail(format!("Cannot establish runtime state: {e}"))),
    }
}

fn maintenance(
    paths: &AppPaths,
    status: &RuntimeStatus,
    token: Uuid,
    action: &str,
) -> io::Result<()> {
    match rpc(
        paths,
        ClientRequest::RemoteRuntimeMaintenance {
            instance_id: status.instance_id,
            token,
            action: action.into(),
        },
    )? {
        ServerResponse::Accepted => Ok(()),
        _ => Err(fail("Runtime did not acknowledge maintenance.")),
    }
}

fn modern(status: &RuntimeStatus) -> bool {
    // The first maintenance implementation counted every registered terminal
    // as busy, even after its process exited. Migrate those builds through the
    // same fenced handoff as 140; their maintenance endpoint cannot finish it.
    status.capabilities.iter().any(|s| s == MAINTENANCE)
        && status
            .capabilities
            .iter()
            .any(|s| s == "remote_terminal_maintenance_v1")
}
fn check_idle(status: &RuntimeStatus, _confirmed: bool) -> io::Result<()> {
    if status.active_session_count != 0 {
        return Err(fail(format!(
            "runtime_busy: Waiting for {} active remote agent(s) or execution reservations.",
            status.active_session_count
        )));
    }
    Ok(())
}

// Old releases persist reservations before spawning agent work. Holding the
// writer transaction prevents another client from passing that reservation
// while the old runtime is paused and its children are inspected. No rows are
// changed. This is deliberately not the old Shutdown RPC, which kills agents.
struct LegacyHandoff {
    _database: rusqlite::Connection,
    daemon: super::remote_process::Frozen,
}
fn legacy_database_fence(paths: &AppPaths) -> io::Result<rusqlite::Connection> {
    use rusqlite::{Connection, OpenFlags};
    let db = Connection::open_with_flags(&paths.database_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| fail(format!("Cannot inspect legacy runtime reservations: {e}")))?;
    db.busy_timeout(Duration::from_millis(250))
        .map_err(io::Error::other)?;
    db.execute_batch("BEGIN IMMEDIATE")
        .map_err(|_| fail("runtime_busy: Waiting for remote operations."))?;
    let reservations: i64 = db
        .query_row("SELECT count(*) FROM writer_reservations", [], |r| r.get(0))
        .map_err(|e| fail(format!("Cannot inspect legacy runtime reservations: {e}")))?;
    let active: i64 = db.query_row(
        "SELECT count(*) FROM agents WHERE json_extract(run_json,'$.state') IN ('Starting','Working','Stopping','AwaitingApproval','Blocked')",
        [], |r| r.get(0)).map_err(io::Error::other)?;
    if reservations > 0 || active > 0 {
        return Err(fail("runtime_busy: Waiting for remote agents."));
    }
    Ok(db)
}
fn prepare_legacy(paths: &AppPaths, running: &RuntimeStatus) -> io::Result<LegacyHandoff> {
    let database = legacy_database_fence(paths)?;
    let all = super::remote_process::processes()?;
    let process = all
        .iter()
        .find(|p| p.pid == running.pid)
        .ok_or_else(|| fail("Remote runtime exited before handoff"))?;
    let daemon = super::remote_process::Frozen::new(process)?;
    let all = super::remote_process::processes()?;
    let terminals = super::remote_process::legacy_project_terminals(&all, running.pid)?;
    super::remote_process::close_terminal_sessions(&terminals)?;
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let all = super::remote_process::processes()?;
        if !super::remote_process::descendants(&all, running.pid)
            .iter()
            .any(|p| p.state != 'Z')
        {
            break;
        }
        if Instant::now() >= until {
            return Err(fail("runtime_busy: Waiting for remote processes to exit."));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(LegacyHandoff {
        _database: database,
        daemon,
    })
}

fn systemd_pid() -> io::Result<u32> {
    command(
        "systemctl",
        &["--user", "show", SERVICE, "--property=MainPID", "--value"],
    )?
    .trim()
    .parse()
    .map_err(|_| fail("Invalid systemd MainPID"))
}
fn launchd_domain() -> String {
    format!("user/{}", unsafe { libc::getuid() })
}
fn launchd_label() -> String {
    format!("{}/{LABEL}", launchd_domain())
}
fn launchd_pid() -> io::Result<u32> {
    let output = command("launchctl", &["print", &launchd_label()])?;
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid = ")?.parse().ok())
        .ok_or_else(|| fail("launchd service has no running PID"))
}

fn manager_for(paths: &AppPaths, running: Option<&RuntimeStatus>) -> io::Result<Manager> {
    let supervised = if cfg!(target_os = "linux") {
        systemd_pid()
            .ok()
            .filter(|p| *p != 0)
            .map(|pid| (Manager::Systemd, pid))
    } else {
        launchd_pid().ok().map(|pid| (Manager::Launchd, pid))
    };
    if let Some((manager, pid)) = supervised {
        return owned_manager(running.map(|s| s.pid), manager, pid);
    }
    if running.is_some() {
        #[cfg(target_os = "linux")]
        if let Some(running) = running {
            let group = fs::read_to_string(format!("/proc/{}/cgroup", running.pid))?;
            if group.lines().any(|line| {
                line.rsplit('/')
                    .next()
                    .is_some_and(|unit| unit.ends_with(".service") && !unit.starts_with("user@"))
            }) {
                return Err(fail(
                    "Runtime belongs to a service, but its ownership could not be verified. Nothing was stopped.",
                ));
            }
        }
        // Presence of an inactive service is not proof it owns this process.
        return Ok(Manager::Direct);
    }
    if manager_path(paths).exists() {
        return read_json(&manager_path(paths));
    }
    if cfg!(target_os = "linux") && command("systemctl", &["--user", "show-environment"]).is_ok() {
        Ok(Manager::Systemd)
    } else if cfg!(target_os = "macos")
        && command("launchctl", &["print", &launchd_domain()]).is_ok()
    {
        Ok(Manager::Launchd)
    } else {
        Ok(Manager::Direct)
    }
}

fn owned_manager(running: Option<u32>, manager: Manager, service_pid: u32) -> io::Result<Manager> {
    if service_pid != 0 && running == Some(service_pid) {
        Ok(manager)
    } else {
        Err(fail(
            "The service and runtime disagree about process ownership. Nothing was stopped.",
        ))
    }
}

fn home(paths: &AppPaths) -> io::Result<&Path> {
    paths
        .data_dir
        .parent()
        .ok_or_else(|| fail("Missing remote home directory"))
}

fn install_service(paths: &AppPaths, manager: Manager) -> io::Result<()> {
    fs::create_dir_all(&paths.logs_dir)?;
    match manager {
        Manager::Systemd => {
            let file = home(paths)?.join(".config/systemd/user/the-ditch.service");
            let expected = "ExecStart=%h/.ditch/bin/current/ditchd daemon --remote";
            if file.exists() && !fs::read_to_string(&file)?.contains(expected) {
                return Err(fail(
                    "Existing the-ditch.service has a custom executable. Repair will not overwrite it.",
                ));
            }
            if !file.exists() {
                fs::create_dir_all(file.parent().unwrap())?;
                fs::write(
                    &file,
                    format!(
                        "[Unit]\nDescription=The Ditch Remote Runtime\n[Service]\nType=simple\n{expected}\nRestart=on-failure\nRestartSec=2\nStandardOutput=append:%h/.ditch/logs/runtime.log\nStandardError=append:%h/.ditch/logs/runtime.log\n[Install]\nWantedBy=default.target\n"
                    ),
                )?;
            }
            command("systemctl", &["--user", "daemon-reload"])?;
            command("systemctl", &["--user", "enable", SERVICE])?;
        }
        Manager::Launchd => {
            let file = home(paths)?.join("Library/LaunchAgents/dev.theditch.runtime.remote.plist");
            let executable = paths.data_dir.join("bin/current/ditchd");
            let escaped = executable
                .to_string_lossy()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let log = paths
                .logs_dir
                .join("runtime.log")
                .to_string_lossy()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            if file.exists()
                && !fs::read_to_string(&file)?.contains(&format!("<string>{escaped}</string>"))
            {
                return Err(fail(
                    "Existing launchd service has a custom executable. Repair will not overwrite it.",
                ));
            }
            if !file.exists() {
                fs::create_dir_all(file.parent().unwrap())?;
                fs::write(
                    &file,
                    format!(
                        "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Label</key><string>{LABEL}</string><key>ProgramArguments</key><array><string>{escaped}</string><string>daemon</string><string>--remote</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string></dict></plist>"
                    ),
                )?;
                fs::set_permissions(&file, fs::Permissions::from_mode(0o600))?;
            }
        }
        Manager::Direct => {}
    }
    Ok(())
}

fn start(paths: &AppPaths, manager: Manager) -> io::Result<()> {
    match manager {
        Manager::Systemd => {
            command("systemctl", &["--user", "reset-failed", SERVICE])?;
            command("systemctl", &["--user", "start", SERVICE])?;
        }
        Manager::Launchd => {
            let file = home(paths)?.join("Library/LaunchAgents/dev.theditch.runtime.remote.plist");
            command("launchctl", &["enable", &launchd_label()])?;
            if command("launchctl", &["print", &launchd_label()]).is_err() {
                command(
                    "launchctl",
                    &["bootstrap", &launchd_domain(), &file.to_string_lossy()],
                )?;
            }
            command("launchctl", &["kickstart", &launchd_label()])?;
        }
        Manager::Direct => {
            fs::create_dir_all(&paths.logs_dir)?;
            let log = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(paths.logs_dir.join("runtime.log"))?;
            let mut command = Command::new(paths.data_dir.join("bin/current/ditchd"));
            command
                .env("HOME", home(paths)?)
                .args(["daemon", "--remote"])
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
            let mut child = command.spawn()?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
    Ok(())
}

pub(super) fn start_service_if_configured(paths: &AppPaths) -> io::Result<bool> {
    if !manager_path(paths).exists() {
        return Ok(false);
    }
    let manager: Manager = read_json(&manager_path(paths))?;
    if manager == Manager::Direct {
        return Ok(false);
    }
    start(paths, manager)?;
    Ok(true)
}

pub(super) fn wait_for_socket(paths: &AppPaths) -> io::Result<()> {
    let until = Instant::now() + Duration::from_secs(15);
    loop {
        if UnixStream::connect(&paths.socket_path).is_ok() {
            return Ok(());
        }
        if Instant::now() >= until {
            return Err(fail(
                "Service did not start the remote runtime. Inspect the service and ~/.ditch/logs/runtime.log.",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn prepare(
    paths: &AppPaths,
    running: &RuntimeStatus,
    token: Uuid,
    confirmed: bool,
) -> io::Result<()> {
    check_idle(running, confirmed)?;
    if modern(running) {
        maintenance(paths, running, token, "begin")?;
    }
    Ok(())
}

fn stop(
    paths: &AppPaths,
    running: &RuntimeStatus,
    manager: Manager,
    token: Uuid,
    confirmed: bool,
) -> io::Result<()> {
    let current =
        status(paths)?.ok_or_else(|| fail("Daemon exited before the handoff; retry repair."))?;
    if current.instance_id != running.instance_id || current.pid != running.pid {
        return Err(fail("Daemon changed before restart; nothing was stopped."));
    }
    let process_identity = super::remote_process::processes()?
        .into_iter()
        .find(|p| p.pid == current.pid)
        .ok_or_else(|| fail("Daemon exited before restart"))?
        .identity;
    prepare(paths, &current, token, confirmed)?;
    let legacy = if modern(&current) {
        None
    } else {
        Some(prepare_legacy(paths, &current)?)
    };
    match manager {
        Manager::Systemd => {
            if systemd_pid()? != current.pid {
                return Err(fail("systemd ownership changed; nothing was stopped."));
            }
            if let Some(legacy) = &legacy {
                command("systemctl", &["--user", "stop", "--no-block", SERVICE])?;
                // Queue termination before resuming the paused legacy daemon;
                // do not reopen an admission window while systemd schedules it.
                legacy.daemon.signal_if_alive(libc::SIGTERM)?;
                legacy.daemon.signal_if_alive(libc::SIGCONT)?;
            } else {
                command("systemctl", &["--user", "stop", SERVICE])?;
            }
        }
        Manager::Launchd => {
            if launchd_pid()? != current.pid {
                return Err(fail("launchd ownership changed; nothing was stopped."));
            }
            if let Some(legacy) = &legacy {
                legacy.daemon.signal_if_alive(libc::SIGTERM)?;
                legacy.daemon.signal_if_alive(libc::SIGCONT)?;
            }
            command("launchctl", &["bootout", &launchd_label()])?;
        }
        Manager::Direct => {
            if modern(&current) {
                maintenance(paths, &current, token, "exit")?;
            } else {
                let legacy = legacy.as_ref().unwrap();
                legacy.daemon.signal_if_alive(libc::SIGTERM)?;
                legacy.daemon.signal_if_alive(libc::SIGCONT)?;
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while super::remote_process::processes()?
        .iter()
        .any(|p| p.pid == current.pid && p.identity == process_identity && p.state != 'Z')
    {
        if Instant::now() >= deadline {
            return Err(fail(
                "Old runtime did not exit. No force-kill was attempted.",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Process exit and kernel descriptor teardown need not become observable
    // in the same instant. Ownership, not a transient zombie entry, is final.
    loop {
        match super::remote_daemon_lock(paths) {
            Ok(_released) => break,
            Err(error) if Instant::now() >= deadline => {
                return Err(fail(format!(
                    "Old process exited but runtime ownership is still held: {error}"
                )));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    Ok(())
}

fn validate_release(path: &Path) -> io::Result<()> {
    if !path.starts_with("versions")
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
    {
        return Err(fail(
            "Invalid runtime release path; repair will not follow it.",
        ));
    }
    Ok(())
}

fn activate(paths: &AppPaths, release: Option<&Path>) -> io::Result<()> {
    let current = paths.data_dir.join("bin/current");
    if let Some(release) = release {
        validate_release(release)?;
        let next = paths.data_dir.join("bin/current.upgrade-next");
        if next.symlink_metadata().is_ok() {
            fs::remove_file(&next)?;
        }
        symlink(release, &next)?;
        fs::rename(next, current)?;
    } else if current.symlink_metadata().is_ok() {
        fs::remove_file(current)?;
    }
    File::open(paths.data_dir.join("bin"))?.sync_all()
}

fn wait_build(
    paths: &AppPaths,
    build: &str,
    edition: &str,
    old_instance: Option<Uuid>,
) -> io::Result<RuntimeStatus> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = "daemon socket is unavailable".to_owned();
    loop {
        match status(paths) {
            Ok(Some(s))
                if s.build_identifier == build
                    && s.edition == edition
                    && Some(s.instance_id) != old_instance
                    && s.capabilities
                        .iter()
                        .any(|c| c == "remote_runtime_protocol_v4") =>
            {
                return Ok(s);
            }
            Ok(Some(s)) => {
                last = format!(
                    "running build {} ({}) does not match expected {build}",
                    s.build_identifier, s.edition
                )
            }
            Err(e) => last = e.to_string(),
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(fail(format!(
                "New runtime health check failed: {last}. Inspect ~/.ditch/logs/runtime.log."
            )));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn clear_journal(paths: &AppPaths) -> io::Result<()> {
    fs::remove_file(journal_path(paths))
}

fn release_maintenance(paths: &AppPaths, running: &RuntimeStatus, token: Uuid) -> io::Result<()> {
    match maintenance(paths, running, token, "cancel") {
        Err(e) if e.to_string().contains("maintenance_expired:") => Ok(()),
        result => result,
    }
}

fn recover(paths: &AppPaths, journal: &Journal, confirmed: bool) -> io::Result<bool> {
    validate_release(&journal.replacement)?;
    if let Some(previous) = &journal.previous {
        validate_release(previous)?;
    }
    if let Some(running) = status(paths)? {
        if running.build_identifier == journal.replacement_build
            && running.edition == super::RUNTIME_EDITION
            && running
                .capabilities
                .iter()
                .any(|c| c == "remote_runtime_protocol_v4")
            && Some(running.instance_id) != journal.old_instance
        {
            // A completed activation with a lost SSH reply needs no restart.
            write_json(&manager_path(paths), &journal.manager)?;
            if modern(&running) {
                release_maintenance(paths, &running, journal.token)?;
            }
            clear_journal(paths)?;
            return Ok(true);
        }
        if Some(running.instance_id) != journal.old_instance {
            // Do not stop a different process, even if it occupies our socket.
            return Err(fail(
                "Recovery found an unexpected runtime instance. Nothing was stopped.",
            ));
        }
        if modern(&running) {
            let _ = maintenance(paths, &running, journal.token, "cancel");
        }
        if journal.phase == "prepared" {
            // No activation occurred. In particular, a busy legacy daemon must
            // never be stopped merely to recover a failed prepare operation.
            clear_journal(paths)?;
            return Ok(false);
        }
        check_idle(&running, confirmed)?;
        // The original process is still alive: restore only its executable link.
        activate(paths, journal.previous.as_deref())?;
        clear_journal(paths)?;
        return Ok(false);
    }
    activate(paths, journal.previous.as_deref())?;
    if journal.previous.is_some() {
        start(paths, journal.manager)?;
        wait_build(
            paths,
            journal.previous_build.as_deref().ok_or_else(|| {
                fail("Previous build is unknown; inspect the runtime before recovery.")
            })?,
            journal
                .previous_edition
                .as_deref()
                .ok_or_else(|| fail("Previous runtime edition is unknown"))?,
            None,
        )?;
    }
    clear_journal(paths)?;
    Ok(false)
}

pub(super) fn upgrade(target: &str, checksum: &str, confirmed: bool) -> io::Result<()> {
    let expected_target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => return Err(fail("Unsupported remote platform")),
    };
    if target != expected_target
        || checksum.len() != 64
        || !checksum.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(fail("Invalid remote artifact target or digest"));
    }
    let executable = std::env::current_exe()?;
    let bytes = fs::read(&executable)?;
    if format!("{:x}", Sha256::digest(&bytes)) != checksum {
        return Err(fail("Uploaded runtime checksum mismatch"));
    }
    let paths = AppPaths::for_remote_user();
    let _lock = lock(&paths, true)?;
    if journal_path(&paths).exists() {
        let journal: Journal = read_json(&journal_path(&paths))?;
        let activated = recover(&paths, &journal, confirmed)
            .map_err(|e| fail(format!("Interrupted upgrade recovery failed: {e}")))?;
        if activated && journal.replacement_build == build_id() {
            println!("Recovered the successfully activated runtime.");
            return Ok(());
        }
    }
    let running = status(&paths)?;
    if let Some(running) = &running {
        if running.build_identifier == build_id() && running.edition == super::RUNTIME_EDITION {
            println!("Requested runtime is already running.");
            return Ok(());
        }
        let local_number = option_env!("DITCH_BUILD_NUMBER").and_then(|s| s.parse::<u64>().ok());
        let remote_number = running.build_number.parse::<u64>().ok();
        if running.edition == super::RUNTIME_EDITION
            && remote_number.is_some_and(|remote| remote > local_number.unwrap_or(0) && remote > 0)
        {
            return Err(fail(
                "client_update_required: The remote runtime is newer. Update this Mac's Ditch app; the remote was not downgraded.",
            ));
        }
    }
    if running.is_none() {
        // An unreachable socket is not proof the daemon has exited.
        // Respect the same lifetime lock used by old and new remote runtimes.
        let _owner = super::remote_daemon_lock(&paths).map_err(|e| {
            fail(format!(
                "Cannot prove that the previous daemon has exited: {e}. Nothing was stopped."
            ))
        })?;
    }
    if let Some(running) = &running {
        check_idle(running, confirmed)?;
    }
    let manager = manager_for(&paths, running.as_ref())?;
    install_service(&paths, manager)?;
    let previous = match fs::read_link(paths.data_dir.join("bin/current")) {
        Ok(path) => {
            validate_release(&path)?;
            Some(path)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    // Stage the executable without changing the selected release.
    let replacement = PathBuf::from(format!(
        "versions/{}/{target}/{checksum}",
        env!("CARGO_PKG_VERSION")
    ));
    let destination = paths.data_dir.join("bin").join(&replacement).join("ditchd");
    fs::create_dir_all(destination.parent().unwrap())?;
    if !destination.exists() {
        let temporary = destination.with_extension(format!("next-{}", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &destination)?;
    } else if format!("{:x}", Sha256::digest(fs::read(&destination)?)) != checksum {
        return Err(fail("Existing staged runtime does not match its checksum"));
    }
    let (previous_build, previous_edition) = if let Some(running) = &running {
        (
            Some(running.build_identifier.clone()),
            Some(running.edition.clone()),
        )
    } else if let Some(previous) = &previous {
        let binary = paths.data_dir.join("bin").join(previous).join("ditchd");
        let output = command(&binary.to_string_lossy(), &["version-json"])?;
        let metadata: serde_json::Value =
            serde_json::from_str(&output).map_err(io::Error::other)?;
        (
            Some(
                metadata["build_identifier"]
                    .as_str()
                    .ok_or_else(|| fail("Previous runtime build is unknown"))?
                    .to_owned(),
            ),
            Some(
                metadata["edition"]
                    .as_str()
                    .ok_or_else(|| fail("Previous runtime edition is unknown"))?
                    .to_owned(),
            ),
        )
    } else {
        (None, None)
    };
    let journal = Journal {
        manager,
        previous,
        replacement,
        previous_build,
        previous_edition,
        replacement_build: build_id().into(),
        token: Uuid::new_v4(),
        old_instance: running.as_ref().map(|s| s.instance_id),
        phase: "prepared".into(),
    };
    handoff(&paths, journal, running.as_ref(), confirmed)
}

fn handoff(
    paths: &AppPaths,
    mut journal: Journal,
    running: Option<&RuntimeStatus>,
    confirmed: bool,
) -> io::Result<()> {
    let manager = journal.manager;
    write_json(&journal_path(paths), &journal)?;
    let result: io::Result<()> = (|| {
        if let Some(running) = running {
            prepare(paths, running, journal.token, confirmed)?;
        }
        // Switch first while admission is closed. New SSH bridges now execute
        // the replacement's upgrade-lock-aware bootstrap, not the legacy one.
        journal.phase = "switching".into();
        write_json(&journal_path(paths), &journal)?;
        activate(paths, Some(&journal.replacement))?;
        journal.phase = "activated".into();
        write_json(&journal_path(paths), &journal)?;
        if let Some(running) = running {
            stop(paths, running, manager, journal.token, confirmed)?;
        }
        journal.phase = "stopped".into();
        write_json(&journal_path(paths), &journal)?;
        write_json(&manager_path(paths), &manager)?;
        start(paths, manager)?;
        let replacement = wait_build(
            paths,
            &journal.replacement_build,
            super::RUNTIME_EDITION,
            journal.old_instance,
        )?;
        release_maintenance(paths, &replacement, journal.token)?;
        clear_journal(paths)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            println!("Remote runtime activated successfully ({manager:?}).");
            Ok(())
        }
        Err(error) => match recover(paths, &journal, confirmed) {
            Ok(true) => {
                println!("Replacement verified after interrupted activation.");
                Ok(())
            }
            Ok(false) => Err(fail(format!(
                "{error}; previous runtime restored or existing process retained."
            ))),
            Err(recovery) => Err(fail(format!(
                "{error}; recovery incomplete: {recovery}. Retry Install / Repair; saved projects and sessions were not deleted."
            ))),
        },
    }
}

#[cfg(test)]
#[path = "remote_lifecycle_tests.rs"]
mod tests;
