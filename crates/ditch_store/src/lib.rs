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

pub fn ensure_app_dirs(paths: &AppPaths) -> Result<(), StoreError> {
    fs::create_dir_all(&paths.data_dir)?;
    fs::create_dir_all(&paths.logs_dir)?;
    fs::create_dir_all(&paths.scrollback_dir)?;
    Ok(())
}

pub fn ensure_project_metadata(project: &Project) -> Result<(), StoreError> {
    let root = project.root.join(PROJECT_METADATA_DIR);
    fs::create_dir_all(root.join(PROJECT_HOOKS_DIR))?;
    fs::create_dir_all(root.join(PROJECT_MCP_DIR))?;
    fs::create_dir_all(root.join(PROJECT_AGENTS_DIR))?;
    Ok(())
}

pub fn is_project_metadata_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == PROJECT_METADATA_DIR)
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
