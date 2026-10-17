//! OS observations used during an idle runtime handoff. Signals target an
//! identified process, never a process group or an agent executable by name.
#[cfg(target_os = "linux")]
use std::fs;
use std::{io, time::Duration};

#[derive(Clone, Debug)]
pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub state: char,
    pub identity: String,
    pub session: u32,
    pub has_terminal: bool,
}

#[cfg(target_os = "linux")]
pub(super) fn processes() -> io::Result<Vec<Process>> {
    let mut result = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let text = match fs::read_to_string(entry.path().join("stat")) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            // Other users' processes are not descendants of this user runtime.
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => continue,
            Err(e) => return Err(e),
        };
        let (_, rest) = text
            .rsplit_once(") ")
            .ok_or_else(|| io::Error::other("Invalid process status"))?;
        let fields: Vec<_> = rest.split_whitespace().collect();
        let number = |index: usize| -> io::Result<i32> {
            fields
                .get(index)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| io::Error::other("Invalid process identity"))
        };
        result.push(Process {
            pid,
            parent: number(1)? as u32,
            session: number(3)? as u32,
            has_terminal: number(4)? != 0,
            state: fields[0].chars().next().unwrap_or('?'),
            identity: fields
                .get(19)
                .ok_or_else(|| io::Error::other("Missing process start time"))?
                .to_string(),
        });
    }
    Ok(result)
}

#[cfg(target_os = "macos")]
pub(super) fn processes() -> io::Result<Vec<Process>> {
    // proc_bsdinfo from the macOS SDK. Query only this user's processes.
    #[repr(C)]
    struct BsdInfo {
        header: [u32; 12],
        comm: [u8; 16],
        name: [u8; 32],
        terminal: [u32; 6],
        started: [u64; 2],
    }
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_listpids(kind: u32, id: u32, buffer: *mut libc::c_void, size: i32) -> i32;
        fn proc_pidinfo(
            pid: i32,
            flavor: i32,
            arg: u64,
            buffer: *mut libc::c_void,
            size: i32,
        ) -> i32;
    }
    let size = unsafe { proc_listpids(4, libc::getuid(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pids = vec![0i32; size as usize / 4 + 1024];
    let bytes = unsafe {
        proc_listpids(
            4,
            libc::getuid(),
            pids.as_mut_ptr().cast(),
            (pids.len() * 4) as i32,
        )
    };
    if bytes <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut result = Vec::new();
    for pid in pids.into_iter().take(bytes as usize / 4).filter(|p| *p > 0) {
        let mut info: BsdInfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<BsdInfo>() as i32;
        let count = unsafe { proc_pidinfo(pid, 3, 0, (&mut info as *mut BsdInfo).cast(), size) };
        if count != size {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::ESRCH)) {
                continue;
            }
            return Err(error);
        }
        result.push(Process {
            pid: info.header[3],
            parent: info.header[4],
            session: unsafe { libc::getsid(pid) }.max(0) as u32,
            has_terminal: info.terminal[3] != u32::MAX,
            state: match info.header[1] {
                4 => 'T',
                5 => 'Z',
                3 => 'S',
                _ => 'R',
            },
            identity: format!("{}:{}", info.started[0], info.started[1]),
        });
    }
    Ok(result)
}

pub(super) fn descendants(all: &[Process], parent: u32) -> Vec<Process> {
    let mut ids = vec![parent];
    let mut result = Vec::new();
    let mut i = 0;
    while i < ids.len() {
        let parent = ids[i];
        for process in all.iter().filter(|p| p.parent == parent) {
            if !ids.contains(&process.pid) {
                ids.push(process.pid);
                result.push(process.clone());
            }
        }
        i += 1;
    }
    result
}

#[cfg(target_os = "linux")]
fn arguments(pid: u32) -> io::Result<Vec<Vec<u8>>> {
    Ok(fs::read(format!("/proc/{pid}/cmdline"))?
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(Vec::from)
        .collect())
}

#[cfg(target_os = "macos")]
fn arguments(pid: u32) -> io::Result<Vec<Vec<u8>>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut data = vec![0u8; 1024 * 1024];
    let mut size = data.len();
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            data.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    data.truncate(size);
    if data.len() < 4 {
        return Err(io::Error::other("Missing process arguments"));
    }
    let count = i32::from_ne_bytes(data[..4].try_into().unwrap());
    if count <= 0 {
        return Err(io::Error::other("Missing process arguments"));
    }
    // KERN_PROCARGS2: argc, executable path, NUL padding, argv, environment.
    let mut offset = 4;
    while offset < data.len() && data[offset] != 0 {
        offset += 1;
    }
    while offset < data.len() && data[offset] == 0 {
        offset += 1;
    }
    let args: Vec<_> = data[offset..]
        .split(|b| *b == 0)
        .take(count as usize)
        .map(Vec::from)
        .collect();
    if args.len() != count as usize {
        return Err(io::Error::other("Incomplete process arguments"));
    }
    Ok(args)
}

fn is_setup_command(args: &[Vec<u8>]) -> bool {
    args.iter().any(|arg| arg == b"login")
        || (args.get(1).is_some_and(|arg| arg == b"-c")
            && args.get(2).is_some_and(|arg| {
                String::from_utf8_lossy(arg).contains("Codex sandbox is ready.")
            }))
}

/// Released runtimes have exactly two direct PTY launch paths: project shells
/// and setup commands (Codex login / sandbox setup). Agent subprocesses use
/// pipes. Require a daemon-owned PTY session leader and exclude setup commands;
/// this also recognizes a project shell replaced with `exec command`.
pub(super) fn legacy_project_terminals(all: &[Process], daemon: u32) -> io::Result<Vec<Process>> {
    let mut terminals = Vec::new();
    for child in all.iter().filter(|p| p.parent == daemon && p.state != 'Z') {
        if !child.has_terminal || child.session != child.pid {
            return Err(io::Error::other(
                "runtime_busy: Waiting for remote agents or setup operations.",
            ));
        }
        let args = match arguments(child.pid) {
            Ok(args) => args,
            Err(error) => {
                // Shell exit can race the observation, even while its daemon
                // parent is paused. Do not turn that normal exit into a failed
                // repair or infer ownership from unavailable arguments.
                if !processes()?
                    .iter()
                    .any(|p| p.pid == child.pid && p.identity == child.identity && p.state != 'Z')
                {
                    continue;
                }
                return Err(error);
            }
        };
        if is_setup_command(&args) {
            return Err(io::Error::other(
                "runtime_busy: Waiting for remote setup operations.",
            ));
        }
        terminals.push(child.clone());
    }
    Ok(terminals)
}

/// Close only the selected Ditch PTY sessions. Process identities are captured
/// before signalling, so unrelated shells and reused PIDs cannot be targeted.
pub(super) fn close_terminal_sessions(roots: &[Process]) -> io::Result<()> {
    if roots.is_empty() {
        return Ok(());
    }
    let all = processes()?;
    let mut selected = Vec::new();
    for root in roots {
        if !all
            .iter()
            .any(|p| p.pid == root.pid && p.identity == root.identity && p.state != 'Z')
        {
            continue;
        }
        selected.extend(
            all.iter()
                .filter(|p| p.session == root.pid && p.state != 'Z')
                .cloned(),
        );
        selected.extend(
            descendants(&all, root.pid)
                .into_iter()
                .filter(|p| p.state != 'Z'),
        );
    }
    selected.sort_by_key(|p| p.pid);
    selected.dedup_by_key(|p| p.pid);
    let mut handles = Vec::new();
    for process in &selected {
        match Frozen::handle(process) {
            Ok(handle) => handles.push((process, handle)),
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
            Err(e) => return Err(e),
        }
    }
    for (_, handle) in &handles {
        handle.signal_if_alive(libc::SIGHUP)?;
        handle.signal_if_alive(libc::SIGCONT)?;
    }
    let mut deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut forced = false;
    loop {
        let all = processes()?;
        let live: Vec<_> = handles
            .iter()
            .filter(|(old, _)| {
                all.iter()
                    .any(|p| p.pid == old.pid && p.identity == old.identity && p.state != 'Z')
            })
            .collect();
        if live.is_empty() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            if forced {
                return Err(io::Error::other("Ditch terminal command did not exit"));
            }
            // Force applies exclusively to the user-authorized terminal jobs,
            // never the daemon or agent process trees.
            for (_, handle) in live {
                handle.signal_if_alive(libc::SIGKILL)?;
            }
            forced = true;
            deadline = std::time::Instant::now() + Duration::from_secs(1);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A stopped process is always resumed on an error, including unwinding.
/// Linux pidfds prevent PID reuse from redirecting a signal.
pub(super) struct Frozen {
    #[cfg(target_os = "macos")]
    process: Process,
    #[cfg(target_os = "linux")]
    pidfd: std::os::fd::OwnedFd,
    armed: bool,
}
impl Frozen {
    fn handle(process: &Process) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        let pidfd = {
            use std::os::fd::FromRawFd;
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, process.pid, 0) } as i32;
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }
        };
        let guard = Self {
            #[cfg(target_os = "macos")]
            process: process.clone(),
            #[cfg(target_os = "linux")]
            pidfd,
            armed: false,
        };
        if !processes()?
            .iter()
            .any(|p| p.pid == process.pid && p.identity == process.identity)
        {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        Ok(guard)
    }

    pub fn new(process: &Process) -> io::Result<Self> {
        let mut guard = Self::handle(process)?;
        // An independently detached watchdog resumes the process if the SSH
        // helper disappears. A Drop handler alone cannot recover SIGKILL.
        let mut watchdog = std::process::Command::new(std::env::current_exe()?);
        watchdog
            .args([
                "remote-handoff-watchdog",
                &process.pid.to_string(),
                &process.identity,
                &std::process::id().to_string(),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        use std::os::unix::process::CommandExt;
        unsafe {
            watchdog.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut child = watchdog.spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        guard.signal(libc::SIGSTOP)?;
        guard.armed = true;
        for _ in 0..100 {
            if processes()?.iter().any(|p| {
                p.pid == process.pid
                    && p.identity == process.identity
                    && matches!(p.state, 'T' | 't')
            }) {
                return Ok(guard);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::other(
            "Could not pause the idle runtime for handoff",
        ))
    }
    pub fn signal(&self, signal: i32) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        let result = {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.pidfd.as_raw_fd(),
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                ) as i32
            }
        };
        #[cfg(target_os = "macos")]
        let result = {
            if !processes()?
                .iter()
                .any(|p| p.pid == self.process.pid && p.identity == self.process.identity)
            {
                return Ok(());
            }
            unsafe { libc::kill(self.process.pid as i32, signal) }
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn signal_if_alive(&self, signal: i32) -> io::Result<()> {
        match self.signal(signal) {
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
            result => result,
        }
    }
}
impl Drop for Frozen {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.signal(libc::SIGCONT);
        }
    }
}

pub(super) fn watchdog(pid: u32, identity: &str, owner: u32) -> io::Result<()> {
    let all = processes()?;
    let Some(_process) = all.iter().find(|p| p.pid == pid && p.identity == identity) else {
        return Ok(());
    };
    let owner_identity = all
        .iter()
        .find(|p| p.pid == owner)
        .map(|p| p.identity.clone());
    #[cfg(target_os = "linux")]
    let pidfd = {
        use std::os::fd::FromRawFd;
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }
    };
    if !processes()?
        .iter()
        .any(|p| p.pid == pid && p.identity == identity)
    {
        return Ok(());
    }
    let mut guard = Frozen {
        #[cfg(target_os = "macos")]
        process: _process.clone(),
        #[cfg(target_os = "linux")]
        pidfd,
        armed: true,
    };
    let mut stopped = false;
    for _ in 0..300 {
        let all = processes()?;
        if !all.iter().any(|p| p.pid == pid && p.identity == identity) {
            return Ok(());
        }
        if let Some(target) = all.iter().find(|p| p.pid == pid && p.identity == identity) {
            if matches!(target.state, 'T' | 't') {
                stopped = true;
            } else if stopped {
                guard.armed = false;
                return Ok(());
            }
        }
        if !all
            .iter()
            .any(|p| p.pid == owner && Some(&p.identity) == owner_identity.as_ref())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observes_own_process_identity() {
        assert!(
            processes()
                .unwrap()
                .iter()
                .any(|p| p.pid == std::process::id() && !p.identity.is_empty())
        );
        assert!(!arguments(std::process::id()).unwrap().is_empty());
    }

    #[test]
    fn legacy_setup_commands_are_not_project_terminal_sessions() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(|s| s.as_bytes().to_vec())
                .collect::<Vec<_>>()
        };
        assert!(is_setup_command(&args(&["codex", "login"])));
        assert!(is_setup_command(&args(&[
            "sh",
            "-c",
            "printf 'Codex sandbox is ready.'"
        ])));
        assert!(!is_setup_command(&args(&["zsh", "-l"])));
        assert!(!is_setup_command(&args(&["sleep", "300"])));
    }
    fn process(pid: u32, parent: u32) -> Process {
        Process {
            pid,
            parent,
            state: 'S',
            identity: pid.to_string(),
            session: pid,
            has_terminal: false,
        }
    }
    #[test]
    fn finds_background_jobs_and_nested_children() {
        let all = vec![process(2, 1), process(3, 2), process(4, 3), process(5, 9)];
        assert_eq!(descendants(&all, 1).len(), 3);
    }
}
