//! Bounded, non-executing skill collection inspection and promotion.
use base64::Engine;
use ditch_core::SkillFile;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const MAX_FILES: usize = 512;
pub const MAX_FILE: usize = 1024 * 1024;
pub const MAX_TOTAL: usize = 8 * 1024 * 1024;
pub fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
pub fn safe_relative(value: &str) -> io::Result<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || value.len() > 1024
        || value.contains(['\\', '\0', '\n', '\r'])
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || path.components().count() > 20
        || path.components().any(|c| c.as_os_str() == ".git")
    {
        return Err(error("Invalid skill-relative path"));
    }
    Ok(path.into())
}
pub fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > limit as u64 {
        return Err(error("Skill file is not a bounded regular file"));
    }
    let mut data = Vec::new();
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    // The entry may have changed since symlink_metadata. Validate the opened
    // descriptor as well so a replaced FIFO/device is never treated as content.
    if !file.metadata()?.is_file() {
        return Err(error("Opened skill content is not a regular file"));
    }
    file.take((limit + 1) as u64).read_to_end(&mut data)?;
    if data.len() > limit {
        return Err(error("Skill file grew beyond its size limit"));
    }
    Ok(data)
}
pub fn read_tree(root: &Path) -> io::Result<Vec<SkillFile>> {
    let root = fs::canonicalize(root)?;
    let mut files = Vec::new();
    let mut total = 0;
    let mut visited = 0;
    fn walk(
        root: &Path,
        path: &Path,
        files: &mut Vec<SkillFile>,
        total: &mut usize,
        visited: &mut usize,
        depth: usize,
    ) -> io::Result<()> {
        if depth > 16 {
            return Err(error("Skill directory nesting exceeds limit"));
        }
        for item in fs::read_dir(path)? {
            let item = item?;
            *visited += 1;
            if *visited > 4096 {
                return Err(error("Skill exceeds directory entry limit"));
            }
            let path = item.path();
            let relative = path
                .strip_prefix(root)
                .map_err(|_| error("Path escaped skill root"))?
                .to_str()
                .ok_or_else(|| error("Non-UTF8 skill path"))?
                .to_owned();
            safe_relative(&relative)?;
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                walk(root, &path, files, total, visited, depth + 1)?;
            } else if meta.file_type().is_file() {
                if files.len() >= MAX_FILES {
                    return Err(error("Skill exceeds file count limit"));
                }
                let data =
                    crate::acceptance_evidence::read_scoped_file(root, &relative, MAX_FILE as u32)
                        .map_err(error)?;
                *total += data.len();
                if *total > MAX_TOTAL {
                    return Err(error("Skill exceeds total size limit"));
                }
                files.push(SkillFile {
                    path: relative,
                    content_base64: base64::engine::general_purpose::STANDARD.encode(data),
                    executable: meta.permissions().mode() & 0o111 != 0,
                });
            } else {
                return Err(error(
                    "Symlinks and special files are not accepted in managed skills",
                ));
            }
        }
        Ok(())
    }
    walk(&root, &root, &mut files, &mut total, &mut visited, 0)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    validate_files(&files)?;
    Ok(files)
}
pub fn validate_files(files: &[SkillFile]) -> io::Result<(String, String, String, bool)> {
    if files.is_empty() || files.len() > MAX_FILES {
        return Err(error("Invalid skill file count"));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut total = 0;
    let mut manifest = None;
    let mut has_scripts = false;
    let mut ordered = files.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hash = Sha256::new();
    for file in ordered {
        safe_relative(&file.path)?;
        if !names.insert(&file.path) {
            return Err(error("Duplicate skill path"));
        }
        if file.content_base64.len() > MAX_FILE * 2 {
            return Err(error("Encoded file exceeds size limit"));
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(&file.content_base64)
            .map_err(|_| error("Invalid file encoding"))?;
        total += data.len();
        if data.len() > MAX_FILE || total > MAX_TOTAL {
            return Err(error("Skill exceeds size limit"));
        }
        hash.update((file.path.len() as u64).to_le_bytes());
        hash.update(file.path.as_bytes());
        hash.update([u8::from(file.executable)]);
        hash.update((data.len() as u64).to_le_bytes());
        hash.update(&data);
        if file.path == "SKILL.md" {
            if data.len() > 65536 {
                return Err(error("SKILL.md exceeds preview limit"));
            }
            manifest = Some(String::from_utf8(data).map_err(|_| error("SKILL.md is not UTF8"))?);
        }
        has_scripts |= file.executable
            || file.path.starts_with("scripts/")
            || [".sh", ".py", ".js", ".rb", ".exe"]
                .iter()
                .any(|ext| file.path.ends_with(ext));
    }
    let manifest = manifest.ok_or_else(|| error("Missing SKILL.md"))?;
    let (name, description) = manifest_fields(&manifest)?;
    Ok((
        format!("{:x}", hash.finalize()),
        name,
        description,
        has_scripts,
    ))
}
pub fn manifest_fields(text: &str) -> io::Result<(String, String)> {
    let text = text.replace("\r\n", "\n");
    let front = text
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---").map(|v| v.0))
        .ok_or_else(|| error("SKILL.md needs YAML frontmatter"))?;
    fn field(front: &str, key: &str) -> Option<String> {
        let lines = front.lines().collect::<Vec<_>>();
        let (index, line) = lines
            .iter()
            .enumerate()
            .find(|(_, l)| l.starts_with(&format!("{key}:")))?;
        let value = line.split_once(':')?.1.trim();
        let value = if matches!(value, "|" | ">" | "|-" | ">-") {
            lines[index + 1..]
                .iter()
                .take_while(|l| l.starts_with(' ') || l.is_empty())
                .map(|s| s.trim())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            value.trim_matches(['\'', '"']).to_owned()
        };
        (!value.trim().is_empty()).then_some(value)
    }
    let name = field(front, "name").ok_or_else(|| error("Missing skill name"))?;
    let description =
        field(front, "description").ok_or_else(|| error("Missing skill description"))?;
    if name.len() > 128 || description.len() > 4096 || name.chars().any(char::is_control) {
        return Err(error("Skill metadata exceeds limits"));
    }
    Ok((name, description))
}
pub fn write_tree(root: &Path, files: &[SkillFile]) -> io::Result<()> {
    validate_files(files)?;
    fs::create_dir(root)?;
    for file in files {
        let path = root.join(safe_relative(&file.path)?);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(&file.content_base64)
            .map_err(|_| error("Invalid encoding"))?;
        fs::write(&path, data)?;
        fs::set_permissions(
            &path,
            fs::Permissions::from_mode(if file.executable { 0o700 } else { 0o600 }),
        )?;
    }
    Ok(())
}
pub fn enumerate(root: &Path) -> io::Result<Vec<String>> {
    let root = fs::canonicalize(root)?;
    let mut found = Vec::new();
    let mut visited = 0;
    fn walk(
        root: &Path,
        path: &Path,
        found: &mut Vec<String>,
        visited: &mut usize,
        depth: usize,
    ) -> io::Result<()> {
        *visited += 1;
        if *visited > 10000 || depth > 16 {
            return Err(error("Collection exceeds enumeration limits"));
        }
        if path.join("SKILL.md").is_file() {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            found.push(if relative.is_empty() {
                ".".into()
            } else {
                relative
            });
            return Ok(());
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                continue;
            }
            let meta = entry.file_type()?;
            if meta.is_dir() {
                walk(root, &entry.path(), found, visited, depth + 1)?;
            }
        }
        Ok(())
    }
    walk(&root, &root, &mut found, &mut visited, 0)?;
    found.sort();
    Ok(found)
}

/// No shell, checkout, hooks, filters, submodules or repository programs run.
pub fn git(args: &[&str], cwd: &Path, max: usize) -> io::Result<Vec<u8>> {
    let mut child = Command::new("git")
        .process_group(0)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=never",
            "-c",
            "credential.interactive=false",
        ])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take((max + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.take(8192).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() > deadline
            || source_object_bytes(&cwd.join(".git/objects"), 0).unwrap_or(u64::MAX)
                > 64 * 1024 * 1024
        {
            // SAFETY: Git and its transport helpers own this freshly created group.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(error(
                "Source timed out or exceeded the temporary download limit; check network/authentication or use a smaller local collection",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let bytes = out.join().map_err(|_| error("Source reader failed"))??;
    let errors = err.join().unwrap_or_default();
    if !status.success() {
        return Err(error(format!(
            "Source fetch failed (network/auth/ref/rate limit): {}",
            String::from_utf8_lossy(&errors)
        )));
    }
    if bytes.len() > max {
        return Err(error("Source output exceeds limit"));
    }
    Ok(bytes)
}

fn source_object_bytes(path: &Path, depth: usize) -> io::Result<u64> {
    if depth > 3 {
        return Err(error("Excessive Git object nesting"));
    }
    if !path.exists() {
        return Ok(0);
    }
    let mut size = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = match entry.metadata() {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        size = size.saturating_add(if meta.is_dir() {
            source_object_bytes(&entry.path(), depth + 1)?
        } else {
            meta.len()
        });
        if size > 64 * 1024 * 1024 {
            return Ok(size);
        }
    }
    Ok(size)
}

#[cfg(test)]
mod hardening_tests {
    use super::*;
    #[test]
    fn empty_directories_cannot_bypass_skill_collection_limits() {
        let root =
            std::env::temp_dir().join(format!("ditch-skill-entries-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for i in 0..4097 {
            fs::create_dir(root.join(i.to_string())).unwrap();
        }
        let error = read_tree(&root).unwrap_err();
        assert!(error.to_string().contains("directory entry limit"));
        fs::remove_dir_all(root).unwrap();
    }
}
