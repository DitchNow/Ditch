//! Read-only, bounded evidence capture. No checkout, staging, or cleanup operations.
use chrono::Utc;
use ditch_core::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const MAX_OUTPUT: usize = 2 * 1024 * 1024;
pub fn redact(value: &str) -> String {
    let mut text = value
        .lines()
        .map(|line| {
            let lower = line
                .to_ascii_lowercase()
                .replace(['"', '\'', ' ', '\t'], "");
            if [
                "sk-",
                "ghp_",
                "github_pat_",
                "authorization:",
                "bearer",
                "privatekey",
                "password=",
                "password:",
                "api_key",
                "api-key",
                "access_token",
                "secret=",
                "secret:",
                "token=",
            ]
            .iter()
            .any(|k| lower.contains(k))
            {
                "[sensitive line redacted]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    for (name, value) in std::env::vars() {
        if value.len() >= 8
            && ["TOKEN", "SECRET", "PASSWORD", "API_KEY"]
                .iter()
                .any(|s| name.to_uppercase().contains(s))
        {
            text = text.replace(&value, "[redacted]");
        }
    }
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}
pub fn contained(root: &Path, relative: &str) -> Result<PathBuf, String> {
    relative_check_path(relative)?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let path = root.join(relative);
    let mut existing = path.as_path();
    while !existing.exists() {
        existing = existing.parent().ok_or("Missing parent")?;
    }
    let resolved = fs::canonicalize(existing).map_err(|e| e.to_string())?;
    if !resolved.starts_with(&root) {
        return Err("Validator path escapes the canonical project".into());
    }
    // Reject links even if currently contained: a predicate must not follow a replaced alias.
    let mut current = root.clone();
    for part in Path::new(relative).components() {
        current.push(part);
        if fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Validator paths cannot traverse symlinks".into());
        }
    }
    Ok(path)
}
pub fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut child = Command::new("git")
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut data = Vec::new();
        stdout
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut data)
            .map(|_| data)
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            return Err("Git evidence timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let data = reader
        .join()
        .map_err(|_| "Git reader failed")?
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("Git evidence command failed".into());
    }
    if data.len() > MAX_OUTPUT {
        return Err("Git evidence exceeds 2 MiB; narrow the change before review".into());
    }
    Ok(data)
}
pub fn diff(root: &Path) -> Result<String, String> {
    let bytes = git(
        root,
        &["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--"],
    )?;
    Ok(redact(&String::from_utf8_lossy(&bytes)))
}
pub fn capture(root: &Path) -> Result<WorkspaceEvidence, String> {
    let head = git(root, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(|b| String::from_utf8_lossy(&b).trim().to_owned());
    let is_git = git(root, &["rev-parse", "--is-inside-work-tree"]).is_ok();
    let mut hash = Sha256::new();
    let mut warnings = vec![];
    let paths;
    let mut diff_stat = String::new();
    let mut diff_bytes = 0;
    if is_git {
        hash.update(head.as_deref().unwrap_or("unborn"));
        let status = git(
            root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        hash.update(&status);
        paths = status
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect();
        let diff = if head.is_some() {
            git(
                root,
                &["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--"],
            )?
        } else {
            git(
                root,
                &["diff", "--no-ext-diff", "--no-textconv", "--cached", "--"],
            )?
        };
        diff_bytes = diff.len() as u64;
        hash.update(&diff);
        diff_stat = String::from_utf8_lossy(&git(
            root,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--stat",
                if head.is_some() { "HEAD" } else { "--cached" },
                "--",
            ],
        )?)
        .chars()
        .take(16000)
        .collect();
        // Hash dirty and untracked content, including binary files; Git's textual diff alone is insufficient.
        let names = git(
            root,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )?;
        hash_files(
            root,
            names
                .split(|b| *b == 0)
                .filter(|p| !p.is_empty())
                .map(|p| String::from_utf8_lossy(p).into_owned())
                .collect(),
            &mut hash,
        )?;
        warnings.push("Ignored files are outside this evidence snapshot. Changes during a worker turn cannot be reliably attributed to the worker versus another process; no user work is reset.".into());
    } else {
        let mut names = vec![];
        enumerate(root, root, &mut names, 0)?;
        hash_files(root, names.clone(), &mut hash)?;
        paths = names;
        warnings.push("Non-Git project: bounded regular-file snapshot only; no HEAD, diff, or reliable attribution of changes.".into());
    }
    if paths.len() > 4096 {
        return Err("Workspace evidence exceeds 4096 paths".into());
    }
    Ok(WorkspaceEvidence {
        fingerprint: format!("{:x}", hash.finalize()),
        head,
        dirty_paths: paths,
        diff_stat: redact(&diff_stat),
        diff_bytes,
        warnings,
        captured_at: Utc::now(),
    })
}
fn enumerate(
    root: &Path,
    path: &Path,
    names: &mut Vec<String>,
    depth: usize,
) -> Result<(), String> {
    if depth > 20 || names.len() > 4096 {
        return Err("Workspace snapshot exceeds directory/file limits".into());
    }
    for item in fs::read_dir(path).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        let path = item.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            enumerate(root, &path, names, depth + 1)?;
        } else {
            names.push(path.strip_prefix(root).unwrap().to_string_lossy().into());
        }
    }
    Ok(())
}
fn hash_files(root: &Path, mut names: Vec<String>, hash: &mut Sha256) -> Result<(), String> {
    names.sort();
    names.dedup();
    if names.len() > 20000 {
        return Err("Workspace snapshot exceeds 20000 files".into());
    }
    let mut total = 0;
    for name in names {
        relative_check_path(&name)?;
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(&name);
        let path = root.join(&name);
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                hash.update(b"missing");
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        if meta.file_type().is_symlink() {
            hash.update(b"symlink");
            hash.update(
                fs::read_link(path)
                    .map_err(|e| e.to_string())?
                    .as_os_str()
                    .as_encoded_bytes(),
            );
            continue;
        }
        if meta.is_dir() {
            hash.update(capture(&path)?.fingerprint);
            continue;
        }
        hash.update(meta.permissions().mode().to_le_bytes());
        total += meta.len();
        if total > 256 * 1024 * 1024 {
            return Err("Workspace evidence exceeds 256 MiB".into());
        }
        let bytes = read_scoped_file(root, &name, 32 * 1024 * 1024)?;
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Ok(())
}
pub fn file_check(root: &Path, path: &str, predicate: &FilePredicate) -> Result<bool, String> {
    let resolved = contained(root, path)?;
    match predicate {
        FilePredicate::Exists => Ok(resolved.is_file()),
        FilePredicate::Absent => Ok(!resolved.exists()),
        FilePredicate::Contains(text) | FilePredicate::NotContains(text) => {
            let bytes = read_scoped_file(root, path, 1024 * 1024)?;
            let content = String::from_utf8(bytes).map_err(|_| "File predicate requires UTF-8")?;
            Ok(content.contains(text) == matches!(predicate, FilePredicate::Contains(_)))
        }
    }
}

/// Open each component relative to an already-open directory, so a concurrent
/// rename/symlink swap cannot redirect an automatic read outside its root.
pub fn read_scoped_file(root: &Path, relative: &str, max_bytes: u32) -> Result<Vec<u8>, String> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    relative_check_path(relative)?;
    let root = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let root_name = CString::new(root.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: CString is NUL terminated and the returned fd is owned exactly once.
    let fd = unsafe {
        libc::open(
            root_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: successful open above transferred a fresh descriptor to this File.
    let mut directory = unsafe { fs::File::from_raw_fd(fd) };
    let parts = Path::new(relative).components().collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let last = index + 1 == parts.len();
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if last {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        // SAFETY: directory owns a live directory fd; name is a single validated component.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: successful openat returned a fresh owned descriptor.
        let file = unsafe { fs::File::from_raw_fd(fd) };
        if last {
            if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err("Context must be a regular file".into());
            }
            let mut bytes = Vec::new();
            std::io::Read::take(file, u64::from(max_bytes) + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > max_bytes as usize {
                return Err("Context byte limit exceeded".into());
            }
            return Ok(bytes);
        }
        directory = file;
    }
    Err("Context needs a relative file path".into())
}

#[cfg(test)]
mod hardening_tests {
    use super::*;
    #[test]
    fn redaction_matches_normalized_bearer_and_private_key_markers() {
        for text in [
            "Bearer test-value",
            "-----BEGIN PRIVATE KEY-----",
            "-----BEGIN RSA PRIVATE KEY-----",
            "Authorization: sample",
        ] {
            assert_eq!(redact(text), "[sensitive line redacted]");
        }
        assert_eq!(redact("ordinary output"), "ordinary output");
    }
    #[test]
    fn scoped_read_rejects_intermediate_links_and_special_files() {
        let root = std::env::temp_dir().join(format!("ditch-scoped-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("inside")).unwrap();
        fs::write(root.join("inside/plain"), "data").unwrap();
        std::os::unix::fs::symlink(root.join("inside"), root.join("alias")).unwrap();
        assert!(read_scoped_file(&root, "alias/plain", 100).is_err());
        assert!(
            file_check(
                &root,
                "alias/plain",
                &FilePredicate::Contains("data".into())
            )
            .is_err()
        );
        assert_eq!(read_scoped_file(&root, "inside/plain", 4).unwrap(), b"data");
        assert!(read_scoped_file(&root, "inside/plain", 3).is_err());
        let name =
            std::ffi::CString::new(root.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: create a FIFO at the owned fixture path; no external reader/writer.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(read_scoped_file(&root, "fifo", 100).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
