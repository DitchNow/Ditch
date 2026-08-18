use chrono::{DateTime, Utc};
use ditch_core::{AgentResumeBlockReason, AgentRun, AgentState, AppPaths, Project, ProjectId};
use ditch_protocol::{AgentChatMessage, RuntimeAttention};
use rusqlite::{Connection, Transaction, params};
use std::fs;
use std::io;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored data is invalid: {0}")]
    InvalidData(String),
}

pub struct DurableAgent {
    pub run: AgentRun,
    pub messages: Vec<AgentChatMessage>,
    pub terminal_failure: Option<String>,
    pub codex_home: Option<String>,
}

pub struct DurableState {
    pub projects: Vec<Project>,
    pub agents: Vec<DurableAgent>,
    pub attention: Vec<RuntimeAttention>,
}

pub struct DitchStore {
    connection: Connection,
}

impl DitchStore {
    pub fn open(paths: &AppPaths) -> Result<Self, StoreError> {
        let connection = Connection::open(&paths.database_path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(SCHEMA)?;
        ensure_column(
            &connection,
            "projects",
            "git_policy",
            "TEXT NOT NULL DEFAULT '\"RequireRepository\"'",
        )?;
        ensure_column(&connection, "agents", "run_json", "TEXT")?;
        ensure_column(&connection, "agents", "terminal_failure", "TEXT")?;
        ensure_column(&connection, "agents", "codex_home", "TEXT")?;
        connection.pragma_update(None, "user_version", 1)?;
        let mut store = Self { connection };
        store.import_legacy_registry_if_empty(paths)?;
        Ok(store)
    }

    fn import_legacy_registry_if_empty(&mut self, paths: &AppPaths) -> Result<(), StoreError> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0))?;
        if count == 0 {
            for project in load_project_registry(paths)? {
                self.upsert_project(&project)?;
            }
        }
        Ok(())
    }

    pub fn load(&self) -> Result<DurableState, StoreError> {
        let mut projects_stmt = self.connection.prepare(
            "SELECT id, name, root, created_at, archived_at, git_policy FROM projects ORDER BY created_at",
        )?;
        let projects = projects_stmt
            .query_map([], |row| {
                Ok(Project {
                    id: ProjectId(parse_uuid(row.get::<_, String>(0)?)?),
                    name: row.get(1)?,
                    root: std::path::PathBuf::from(row.get::<_, String>(2)?),
                    created_at: parse_time(row.get::<_, String>(3)?)?,
                    archived_at: row
                        .get::<_, Option<String>>(4)?
                        .map(parse_time)
                        .transpose()?,
                    git_policy: from_json(&row.get::<_, String>(5)?)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut agents_stmt = self.connection.prepare(
            "SELECT run_json, terminal_failure, codex_home FROM agents ORDER BY started_at",
        )?;
        let mut agents = agents_stmt
            .query_map([], |row| {
                Ok(DurableAgent {
                    run: from_json(&row.get::<_, String>(0)?)?,
                    messages: Vec::new(),
                    terminal_failure: row.get(1)?,
                    codex_home: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for agent in &mut agents {
            let mut stmt = self.connection.prepare(
                "SELECT message_json FROM agent_messages WHERE agent_id = ?1 ORDER BY sequence",
            )?;
            agent.messages = stmt
                .query_map(params![agent.run.id.0.to_string()], |row| {
                    from_json(&row.get::<_, String>(0)?)
                })?
                .collect::<Result<Vec<_>, _>>()?;
        }

        let mut attention_stmt = self.connection.prepare(
            "SELECT attention_json FROM attention_events WHERE dismissed_at IS NULL ORDER BY created_at",
        )?;
        let attention = attention_stmt
            .query_map([], |row| from_json(&row.get::<_, String>(0)?))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DurableState {
            projects,
            agents,
            attention,
        })
    }

    pub fn upsert_project(&mut self, project: &Project) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO projects(id,name,root,created_at,archived_at,git_policy) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name,root=excluded.root,archived_at=excluded.archived_at,git_policy=excluded.git_policy",
            params![project.id.0.to_string(), project.name, project.root.to_string_lossy(), project.created_at.to_rfc3339(), project.archived_at.map(|v| v.to_rfc3339()), to_json(&project.git_policy)?],
        )?;
        Ok(())
    }

    pub fn upsert_agent(
        &mut self,
        run: &AgentRun,
        terminal_failure: Option<&str>,
        codex_home: Option<&str>,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO agents(id,provider,state,launch_mode,project_id,native_session_id,current_prompt,last_visible_action,state_confidence,state_evidence,started_at,updated_at,run_json,terminal_failure,codex_home)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
             ON CONFLICT(id) DO UPDATE SET state=excluded.state,native_session_id=excluded.native_session_id,current_prompt=excluded.current_prompt,last_visible_action=excluded.last_visible_action,state_confidence=excluded.state_confidence,state_evidence=excluded.state_evidence,updated_at=excluded.updated_at,run_json=excluded.run_json,terminal_failure=excluded.terminal_failure,codex_home=COALESCE(agents.codex_home,excluded.codex_home)",
            params![run.id.0.to_string(), to_json(&run.provider)?, to_json(&run.state)?, to_json(&run.launch_mode)?, run.project_id.0.to_string(), run.native_session_id, run.current_prompt, run.last_visible_action, run.state_confidence, run.state_evidence, run.started_at.to_rfc3339(), run.updated_at.to_rfc3339(), to_json(run)?, terminal_failure, codex_home],
        )?;
        Ok(())
    }

    pub fn append_message(&mut self, message: &AgentChatMessage) -> Result<(), StoreError> {
        let sequence: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(sequence),0)+1 FROM agent_messages WHERE agent_id=?1",
            params![message.agent_id.0.to_string()],
            |r| r.get(0),
        )?;
        self.connection.execute(
            "INSERT INTO agent_messages(agent_id,sequence,message_json,created_at) VALUES(?1,?2,?3,?4)",
            params![message.agent_id.0.to_string(), sequence, to_json(message)?, message.created_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn persist_new_agent(
        &mut self,
        run: &AgentRun,
        message: &AgentChatMessage,
        codex_home: Option<&str>,
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        upsert_agent_tx(&tx, run, None, codex_home)?;
        tx.execute("INSERT INTO agent_messages(agent_id,sequence,message_json,created_at) VALUES(?1,1,?2,?3)", params![run.id.0.to_string(), to_json(message)?, message.created_at.to_rfc3339()])?;
        tx.commit()?;
        Ok(())
    }

    pub fn persist_failed_agent(
        &mut self,
        run: &AgentRun,
        user_message: &AgentChatMessage,
        system_message: &AgentChatMessage,
        attention: &RuntimeAttention,
        codex_home: Option<&str>,
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        upsert_agent_tx(&tx, run, Some(&system_message.text), codex_home)?;
        for (sequence, message) in [(1_i64, user_message), (2_i64, system_message)] {
            tx.execute(
                "INSERT INTO agent_messages(agent_id,sequence,message_json,created_at) VALUES(?1,?2,?3,?4)",
                params![run.id.0.to_string(), sequence, to_json(message)?, message.created_at.to_rfc3339()],
            )?;
        }
        tx.execute(
            "INSERT INTO attention_events(id,project_id,agent_id,attention_json,created_at,dismissed_at) VALUES(?1,?2,?3,?4,?5,NULL)",
            params![attention.id.to_string(), attention.project_id.map(|v| v.0.to_string()), attention.agent_id.map(|v| v.0.to_string()), to_json(attention)?, attention.created_at.to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_attention(&mut self, attention: &RuntimeAttention) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO attention_events(id,project_id,agent_id,attention_json,created_at,dismissed_at) VALUES(?1,?2,?3,?4,?5,NULL)
             ON CONFLICT(id) DO UPDATE SET attention_json=excluded.attention_json,dismissed_at=NULL",
            params![attention.id.to_string(), attention.project_id.map(|v| v.0.to_string()), attention.agent_id.map(|v| v.0.to_string()), to_json(attention)?, attention.created_at.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn dismiss_attention(&mut self, id: uuid::Uuid) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE attention_events SET dismissed_at=?2 WHERE id=?1",
            params![id.to_string(), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn delete_agent(&mut self, agent_id: ditch_core::AgentId) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let id = agent_id.0.to_string();
        tx.execute(
            "DELETE FROM permission_requests WHERE agent_id=?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM attention_events WHERE agent_id=?1",
            params![id],
        )?;
        tx.execute("DELETE FROM agents WHERE id=?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn reconcile_active_agents(&mut self) -> Result<usize, StoreError> {
        let state = self.load()?;
        let mut count = 0;
        for mut agent in state.agents {
            if matches!(
                agent.run.state,
                AgentState::Starting
                    | AgentState::Working
                    | AgentState::AwaitingApproval
                    | AgentState::Blocked
            ) {
                agent.run.state = AgentState::Stale;
                agent.run.updated_at = Utc::now();
                agent.run.finished_at = Some(agent.run.updated_at);
                agent.run.resume_block_reason = agent
                    .run
                    .native_session_id
                    .is_none()
                    .then_some(AgentResumeBlockReason::NoCodexThread);
                agent.run.last_visible_action = Some(
                    "Runtime restarted; the Codex process can no longer be controlled".to_owned(),
                );
                self.upsert_agent(
                    &agent.run,
                    agent.terminal_failure.as_deref(),
                    agent.codex_home.as_deref(),
                )?;
                self.append_message(&AgentChatMessage { agent_id: agent.run.id, role: ditch_protocol::AgentChatRole::System, text: "The Ditch Runtime restarted while this session was active. Start a new prompt to resume it safely.".to_owned(), created_at: Utc::now() })?;
                count += 1;
            }
        }
        Ok(count)
    }
}

fn upsert_agent_tx(
    tx: &Transaction<'_>,
    run: &AgentRun,
    terminal_failure: Option<&str>,
    codex_home: Option<&str>,
) -> Result<(), StoreError> {
    tx.execute("INSERT INTO agents(id,provider,state,launch_mode,project_id,native_session_id,current_prompt,last_visible_action,state_confidence,state_evidence,started_at,updated_at,run_json,terminal_failure,codex_home) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)", params![run.id.0.to_string(),to_json(&run.provider)?,to_json(&run.state)?,to_json(&run.launch_mode)?,run.project_id.0.to_string(),run.native_session_id,run.current_prompt,run.last_visible_action,run.state_confidence,run.state_evidence,run.started_at.to_rfc3339(),run.updated_at.to_rfc3339(),to_json(run)?,terminal_failure,codex_home])?;
    Ok(())
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::InvalidData(e.to_string()))
}
fn from_json<T: serde::de::DeserializeOwned>(value: &str) -> rusqlite::Result<T> {
    serde_json::from_str(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}
fn parse_uuid(value: String) -> rusqlite::Result<uuid::Uuid> {
    uuid::Uuid::parse_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}
fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|v| v.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })
}
fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<(), StoreError> {
    let mut stmt = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| name.as_deref() == Ok(column));
    if !exists {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))?;
    }
    Ok(())
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

    #[test]
    fn persists_and_reconciles_agent_history() {
        let root =
            std::env::temp_dir().join(format!("ditch-store-persistence-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths {
            data_dir: root.clone(),
            database_path: root.join("ditch.sqlite3"),
            socket_path: root.join("ditchd.sock"),
            logs_dir: root.join("logs"),
            scrollback_dir: root.join("scrollback"),
        };
        ensure_app_dirs(&paths).unwrap();
        let project = Project::new("Persistent", root.join("project"));
        let now = Utc::now();
        let run = AgentRun {
            id: ditch_core::AgentId::new(),
            provider: ditch_core::AgentProvider::Codex,
            state: AgentState::Working,
            launch_mode: ditch_core::CodexLaunchMode::Exec,
            project_id: project.id,
            task_id: None,
            pane_id: None,
            native_session_id: Some("thread-1".into()),
            codex_title: Some("Codex title".into()),
            user_title: None,
            origin_codex_home: Some("/tmp/codex-home".into()),
            current_prompt: Some("hello".into()),
            last_visible_action: Some("Working".into()),
            state_confidence: 1.0,
            state_evidence: "test".into(),
            started_at: now,
            updated_at: now,
            finished_at: None,
            exit_code: None,
            resume_block_reason: None,
        };
        let message = AgentChatMessage {
            agent_id: run.id,
            role: ditch_protocol::AgentChatRole::User,
            text: "hello".into(),
            created_at: now,
        };
        {
            let mut store = DitchStore::open(&paths).unwrap();
            store.upsert_project(&project).unwrap();
            store
                .persist_new_agent(&run, &message, Some("/tmp/codex-home"))
                .unwrap();
        }
        {
            let mut store = DitchStore::open(&paths).unwrap();
            assert_eq!(store.reconcile_active_agents().unwrap(), 1);
            let restored = store.load().unwrap();
            assert_eq!(restored.projects, vec![project]);
            assert_eq!(restored.agents.len(), 1);
            assert_eq!(restored.agents[0].run.state, AgentState::Stale);
            assert_eq!(restored.agents[0].messages.len(), 2);
            assert!(restored.agents[0].messages[1].text.contains("restarted"));
            store.delete_agent(run.id).unwrap();
            let deleted = store.load().unwrap();
            assert!(deleted.agents.is_empty());
            assert!(deleted.attention.is_empty());
        }
        fs::remove_dir_all(root).unwrap();
    }
}

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  root TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  archived_at TEXT,
  git_policy TEXT NOT NULL
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
  ,run_json TEXT NOT NULL
  ,terminal_failure TEXT
  ,codex_home TEXT
);

CREATE TABLE IF NOT EXISTS agent_messages (
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  sequence INTEGER NOT NULL,
  message_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(agent_id, sequence)
);

CREATE TABLE IF NOT EXISTS attention_events (
  id TEXT PRIMARY KEY,
  project_id TEXT REFERENCES projects(id),
  agent_id TEXT REFERENCES agents(id),
  attention_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  dismissed_at TEXT
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
