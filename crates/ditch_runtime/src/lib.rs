use chrono::{DateTime, Utc};
use ditch_core::RuntimePaneId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime pane not found: {0:?}")]
    PaneNotFound(RuntimePaneId),
    #[error("process operation failed: {0}")]
    Process(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

impl CommandSpec {
    pub fn shell(cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned()),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub id: RuntimePaneId,
    pub title: String,
    pub cwd: PathBuf,
    pub foreground_process: Option<String>,
    pub rows: u16,
    pub cols: u16,
    pub scrollback_path: Option<PathBuf>,
    pub recent_visible_text: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PaneSnapshot {
    pub fn new(title: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        let now = Utc::now();
        Self {
            id: RuntimePaneId::new(),
            title: title.into(),
            cwd: cwd.into(),
            foreground_process: None,
            rows: 32,
            cols: 120,
            scrollback_path: None,
            recent_visible_text: String::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PaneInput {
    Text(String),
    Interrupt,
    EndOfTransmission,
}

pub trait RuntimeController {
    fn create_pane(&mut self, command: CommandSpec) -> Result<PaneSnapshot, RuntimeError>;
    fn send_input(&mut self, pane_id: RuntimePaneId, input: PaneInput) -> Result<(), RuntimeError>;
    fn read_pane(&self, pane_id: RuntimePaneId) -> Result<PaneSnapshot, RuntimeError>;
    fn stop_process_group(&mut self, pane_id: RuntimePaneId) -> Result<(), RuntimeError>;
}
