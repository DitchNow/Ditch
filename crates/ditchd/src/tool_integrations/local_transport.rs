use super::*;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::{OpenOptionsExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};
use std::time::Duration;

pub fn peer_pid(stream: &UnixStream) -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        let mut pid: i32 = 0;
        let mut len = std::mem::size_of::<i32>() as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&mut pid as *mut i32).cast(),
                &mut len,
            )
        } == 0
            && pid > 0
        {
            return Some(pid);
        }
    }
    None
}

pub fn start(lease: Arc<Lease>) -> io::Result<()> {
    let listener = UnixListener::bind(&lease.socket)?;
    if let Err(error) =
        std::fs::set_permissions(&lease.socket, std::fs::Permissions::from_mode(0o600))
            .and_then(|_| listener.set_nonblocking(true))
    {
        let _ = std::fs::remove_file(&lease.socket);
        return Err(error);
    }
    let mut credential = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lease.credential_path)
    {
        Ok(file) => file,
        Err(error) => {
            let _ = std::fs::remove_file(&lease.socket);
            return Err(error);
        }
    };
    if let Err(error) = credential.write_all(lease.credential.as_bytes()) {
        let _ = std::fs::remove_file(&lease.socket);
        let _ = std::fs::remove_file(&lease.credential_path);
        return Err(error);
    }
    std::thread::spawn(move || {
        let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        while !lease.cancelled.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if connections.fetch_add(1, Ordering::AcqRel) >= 8 {
                        connections.fetch_sub(1, Ordering::AcqRel);
                        continue;
                    }
                    let connections = connections.clone();
                    let lease = lease.clone();
                    std::thread::spawn(move || {
                        struct Count(Arc<std::sync::atomic::AtomicUsize>);
                        impl Drop for Count {
                            fn drop(&mut self) {
                                self.0.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        let _count = Count(connections);
                        let _ = handle(stream, lease);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => break,
            }
        }
        let _ = std::fs::remove_file(&lease.socket);
        let _ = std::fs::remove_file(&lease.credential_path);
    });
    Ok(())
}

fn handle(mut stream: UnixStream, lease: Arc<Lease>) -> io::Result<()> {
    let pid = peer_pid(&stream).ok_or_else(|| io::Error::other("peer unavailable"))?;
    if !native::verify_peer(pid, 1, &lease.cli) {
        return Err(io::Error::other("unauthorized peer"));
    }
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut line = String::new();
    BufReader::new((&stream).take((MAX_REQUEST_BYTES + 1) as u64)).read_line(&mut line)?;
    if line.len() > MAX_REQUEST_BYTES || !line.ends_with('\n') {
        return Err(io::Error::other("invalid request"));
    }
    let request: AdapterRequest = serde_json::from_str(&line).map_err(io::Error::other)?;
    if request.credential.len() != lease.credential.len()
        || request
            .credential
            .bytes()
            .zip(lease.credential.bytes())
            .fold(0u8, |v, (a, b)| v | (a ^ b))
            != 0
    {
        return Err(io::Error::other("invalid credential"));
    }
    let previous = lease
        .adapter_pid
        .compare_exchange(0, pid, Ordering::AcqRel, Ordering::Acquire)
        .unwrap_or_else(|p| p);
    if previous != 0 && previous != pid {
        return Err(io::Error::other("adapter changed"));
    }
    let completed = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        let monitor = &stream;
        let done = &completed;
        let lease = &lease;
        scope.spawn(move || {
            while !done.load(Ordering::Acquire) {
                let mut byte = 0u8;
                let count = unsafe {
                    libc::recv(
                        monitor.as_raw_fd(),
                        (&mut byte as *mut u8).cast(),
                        1,
                        libc::MSG_PEEK | libc::MSG_DONTWAIT,
                    )
                };
                if count == 0 {
                    lease.revoke();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let result = executor::execute(lease, request);
        completed.store(true, Ordering::Release);
        result
    });
    serde_json::to_writer(&mut stream, &result).map_err(io::Error::other)?;
    stream.write_all(b"\n")
}
