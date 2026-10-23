use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn config(root: &Path, file: Option<&Path>, pattern: &str) -> Vec<(String, String)> {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root);
    command.args(["config", "--no-includes", "--null"]);
    if let Some(file) = file {
        command.arg("--file").arg(file);
    } else {
        command.arg("--local");
    }
    command.args(["--get-regexp", pattern]);
    bounded_output_cancel(command, 5, 65536, &AtomicBool::new(false))
        .unwrap_or_default()
        .split(|b| *b == 0)
        .filter_map(|bytes| {
            let text = std::str::from_utf8(bytes).ok()?;
            let (key, value) = text.split_once('\n')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect()
}

fn github_url(value: &str, base: Option<&str>) -> Option<String> {
    let resolved;
    let value = if value.starts_with("./") || value.starts_with("../") {
        resolved = url::Url::parse(&format!("https://github.com/{}/", base?))
            .ok()?
            .join(value)
            .ok()?
            .to_string();
        resolved.as_str()
    } else {
        value
    };
    if !(value.starts_with("https://github.com/")
        || value.starts_with("git@github.com:")
        || value.starts_with("ssh://git@github.com/"))
    {
        return None;
    }
    repository_name(value).ok()
}

pub(crate) fn discover(root: &Path) -> Result<Value, String> {
    let root = fs::canonicalize(root).map_err(|_| "Project directory is unavailable")?;
    let remotes = config(&root, None, "^remote\\..*\\.url$");
    let base = remotes
        .iter()
        .find(|(k, _)| k == "remote.origin.url")
        .and_then(|(_, v)| github_url(v, None))
        .or_else(|| remotes.iter().find_map(|(_, v)| github_url(v, None)));
    let roots: BTreeSet<_> = remotes
        .iter()
        .filter_map(|(_, v)| github_url(v, None))
        .collect();
    let mut repos: BTreeMap<String, BTreeSet<String>> = roots
        .iter()
        .map(|name| (name.clone(), BTreeSet::from([".".into()])))
        .collect();
    let mut warnings = Vec::new();
    let mut queue = vec![(root.clone(), base, 0)];
    let mut visited = HashSet::new();
    while let Some((dir, parent_repo, depth)) = queue.pop() {
        if !visited.insert(dir.clone()) {
            continue;
        }
        if visited.len() > 64 || depth > 8 {
            warnings.push("Submodule discovery limit reached".to_owned());
            break;
        }
        let file = dir.join(".gitmodules");
        if !fs::symlink_metadata(&file).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
            continue;
        }
        let values = config(&dir, Some(&file), "^submodule\\..*\\.(path|url)$");
        for (key, path) in values.iter().filter(|(k, _)| k.ends_with(".path")) {
            if path.is_empty()
                || !Path::new(path)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                warnings.push("Skipped a submodule with an invalid path".into());
                continue;
            }
            let url_key = format!("{}.url", key.trim_end_matches(".path"));
            let Some((_, value)) = values.iter().find(|(k, _)| k == &url_key) else {
                continue;
            };
            let repo = github_url(value, parent_repo.as_deref());
            let child = dir.join(path);
            let relative = child
                .strip_prefix(&root)
                .map_err(|_| "Invalid submodule path")?
                .to_string_lossy()
                .into_owned();
            if fs::canonicalize(&child).is_ok_and(|p| !p.starts_with(&root)) {
                warnings.push(format!("{relative}: skipped checkout outside project"));
                continue;
            }
            if let Some(name) = &repo {
                repos
                    .entry(name.clone())
                    .or_default()
                    .insert(relative.clone());
            }
            match fs::canonicalize(&child) {
                Ok(canonical)
                    if canonical.starts_with(&root) && canonical.join(".git").exists() =>
                {
                    queue.push((canonical, repo, depth + 1))
                }
                _ => warnings.push(format!(
                    "{relative}: checkout unavailable; nested submodules were not inspected"
                )),
            }
        }
    }
    let candidates: Vec<_> = repos.into_iter().map(|(repository, paths)| json!({"repository":repository,"paths":paths,"is_root":roots.contains(&repository)})).collect();
    let mut ordered: Vec<_> = roots.into_iter().collect();
    if let Some(primary) = config(&root, None, "^remote\\.origin\\.url$")
        .iter()
        .find_map(|(_, v)| github_url(v, None))
    {
        ordered.retain(|v| v != &primary);
        ordered.insert(0, primary);
    }
    Ok(json!({"repositories":ordered,"candidates":candidates,"warnings":warnings}))
}

pub(super) fn paths_for(discovery: &Value, repository: &str) -> Vec<String> {
    discovery["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| {
            v["repository"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(repository))
        })
        .and_then(|v| v["paths"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_relative_nested_and_uninitialized_modules_without_changing_checkout() {
        let root = std::env::temp_dir().join(format!("ditch-discovery-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join(".git/refs")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let config = "[core]\nrepositoryformatversion=0\n[remote \"origin\"]\nurl=git@github.com:owner/main.git\n";
        fs::write(root.join(".git/config"), config).unwrap();
        let modules = "[submodule \"lib\"]\npath=packages/lib\nurl=../lib.git\n[submodule \"missing\"]\npath=missing\nurl=../../other/missing.git\n[submodule \"outside\"]\npath=../outside\nurl=../outside.git\n";
        fs::write(root.join(".gitmodules"), modules).unwrap();
        fs::create_dir_all(root.join("packages/lib/.git")).unwrap();
        fs::write(
            root.join("packages/lib/.gitmodules"),
            "[submodule \"nested\"]\npath=nested\nurl=../nested.git\n",
        )
        .unwrap();
        let found = discover(&root).unwrap();
        assert_eq!(paths_for(&found, "owner/main"), vec!["."]);
        assert_eq!(paths_for(&found, "owner/lib"), vec!["packages/lib"]);
        assert_eq!(
            paths_for(&found, "owner/nested"),
            vec!["packages/lib/nested"]
        );
        assert_eq!(paths_for(&found, "other/missing"), vec!["missing"]);
        assert!(paths_for(&found, "owner/outside").is_empty());
        assert!(!root.join("missing").exists());
        assert_eq!(
            fs::read_to_string(root.join(".git/config")).unwrap(),
            config
        );
        assert_eq!(
            fs::read_to_string(root.join(".gitmodules")).unwrap(),
            modules
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn relative_github_urls_are_resolved_without_network() {
        assert_eq!(
            github_url("../library.git", Some("owner/main")),
            Some("owner/library".into())
        );
        assert_eq!(
            github_url("../../other/library.git", Some("owner/main")),
            Some("other/library".into())
        );
        assert_eq!(
            github_url("https://elsewhere.example/owner/repo", None),
            None
        );
    }
}
