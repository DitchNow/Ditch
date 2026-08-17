use ditch_core::{AppPaths, Project};
use std::fs;
use std::io;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("filesystem error: {0}")]
    Io(#[from] io::Error),
}

pub const PROJECT_METADATA_DIR: &str = ".ditch";
pub const PROJECT_HOOKS_DIR: &str = "hooks";
pub const PROJECT_MCP_DIR: &str = "mcp";
pub const PROJECT_AGENTS_DIR: &str = "agents";
pub const PROJECT_METADATA_DIRS: [&str; 3] =
    [PROJECT_AGENTS_DIR, PROJECT_HOOKS_DIR, PROJECT_MCP_DIR];
pub const PROJECT_CONFIG_FILE: &str = "project.json";
pub const PROJECT_REGISTRY_FILE: &str = "projects.json";

pub fn ensure_app_dirs(paths: &AppPaths) -> Result<(), StoreError> {
    fs::create_dir_all(&paths.data_dir)?;
    fs::create_dir_all(&paths.logs_dir)?;
    fs::create_dir_all(&paths.scrollback_dir)?;
    Ok(())
}

pub fn ensure_project_metadata(project: &Project) -> Result<(), StoreError> {
    let root = project.root.join(PROJECT_METADATA_DIR);
    for directory in PROJECT_METADATA_DIRS {
        fs::create_dir_all(root.join(directory))?;
    }
    verify_project_metadata(project)?;
    Ok(())
}

pub fn verify_project_metadata(project: &Project) -> Result<(), StoreError> {
    let root = project.root.join(PROJECT_METADATA_DIR);
    for directory in PROJECT_METADATA_DIRS {
        let path = root.join(directory);
        if !path.is_dir() {
            return Err(StoreError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "project metadata directory was not created: {}",
                    path.display()
                ),
            )));
        }
    }
    Ok(())
}

pub fn save_project_config(project: &Project) -> Result<(), StoreError> {
    let path = project.ditch_dir().join(PROJECT_CONFIG_FILE);
    let temporary = project.ditch_dir().join("project.json.tmp");
    let bytes = serde_json::to_vec_pretty(project)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn load_project_config(root: &Path) -> Result<Project, StoreError> {
    let bytes = fs::read(root.join(PROJECT_METADATA_DIR).join(PROJECT_CONFIG_FILE))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))
}

pub fn load_project_registry(paths: &AppPaths) -> Result<Vec<Project>, StoreError> {
    let path = paths.data_dir.join(PROJECT_REGISTRY_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))
}

pub fn save_project_registry(paths: &AppPaths, projects: &[Project]) -> Result<(), StoreError> {
    let path = paths.data_dir.join(PROJECT_REGISTRY_FILE);
    let temporary = paths.data_dir.join("projects.json.tmp");
    let bytes = serde_json::to_vec_pretty(projects)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn discover_legacy_projects(search_root: &Path) -> Result<Vec<Project>, StoreError> {
    let mut projects = Vec::new();
    discover_legacy_projects_at(search_root, 0, &mut projects)?;
    Ok(projects)
}

fn discover_legacy_projects_at(
    directory: &Path,
    depth: usize,
    projects: &mut Vec<Project>,
) -> Result<(), StoreError> {
    if depth > 5 || !directory.is_dir() {
        return Ok(());
    }
    let metadata = directory.join(PROJECT_METADATA_DIR);
    if metadata.is_dir() {
        let project = load_project_config(directory).unwrap_or_else(|_| {
            let name = directory
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Recovered project");
            Project::new(name, directory)
        });
        projects.push(project);
        return Ok(());
    }

    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || matches!(name.as_ref(), "build" | "node_modules" | "target" | "Pods")
        {
            continue;
        }
        discover_legacy_projects_at(&entry.path(), depth + 1, projects)?;
    }
    Ok(())
}

pub fn is_project_metadata_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == PROJECT_METADATA_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_verifies_all_project_metadata_directories() {
        let root = std::env::temp_dir().join(format!(
            "ditch-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after the Unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("temporary project root should be created");
        let project = Project::new("Test project", &root);
        let paths = AppPaths {
            data_dir: root.join("app-data"),
            database_path: root.join("app-data/ditch.sqlite3"),
            socket_path: root.join("app-data/ditchd.sock"),
            logs_dir: root.join("app-data/logs"),
            scrollback_dir: root.join("app-data/scrollback"),
        };
        ensure_app_dirs(&paths).expect("application directories should be created");

        ensure_project_metadata(&project).expect("metadata setup should succeed");
        save_project_config(&project).expect("project config should be saved");
        let restored = load_project_config(&root).expect("project config should load");
        save_project_registry(&paths, std::slice::from_ref(&project))
            .expect("project registry should be saved");
        let registered = load_project_registry(&paths).expect("project registry should load");
        let discovered =
            discover_legacy_projects(&root).expect("legacy project discovery should succeed");

        for directory in PROJECT_METADATA_DIRS {
            assert!(root.join(PROJECT_METADATA_DIR).join(directory).is_dir());
        }
        assert_eq!(restored, project);
        assert_eq!(registered, vec![project]);
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].root, root);
        fs::remove_dir_all(root).expect("temporary project root should be removed");
    }
}

pub const INITIAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  root TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  archived_at TEXT
);

CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  state TEXT NOT NULL,
  acceptance_criteria_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agents (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  state TEXT NOT NULL,
  launch_mode TEXT NOT NULL,
  project_id TEXT NOT NULL REFERENCES projects(id),
  task_id TEXT REFERENCES tasks(id),
  pane_id TEXT,
  native_session_id TEXT,
  current_prompt TEXT,
  last_visible_action TEXT,
  state_confidence REAL NOT NULL,
  state_evidence TEXT NOT NULL,
  started_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS permission_requests (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  agent_id TEXT REFERENCES agents(id),
  action TEXT NOT NULL,
  summary TEXT NOT NULL,
  target TEXT NOT NULL,
  command TEXT,
  created_at TEXT NOT NULL,
  expires_at TEXT
);
"#;
