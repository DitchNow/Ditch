use ditch_core::{AgentProvider, CodexLaunchMode, Project};
use ditch_runtime::CommandSpec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent binary was not found: {0}")]
    MissingBinary(String),
    #[error("unsupported launch mode for provider")]
    UnsupportedLaunchMode,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentBinary {
    pub provider: AgentProvider,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CodexStartSpec {
    pub project: Project,
    pub prompt: String,
    pub mode: CodexLaunchMode,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentCommand {
    pub provider: AgentProvider,
    pub mode: CodexLaunchMode,
    pub command: CommandSpec,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexAdapter {
    binary: PathBuf,
}

impl CodexAdapter {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    pub fn command_for(&self, spec: CodexStartSpec) -> AgentCommand {
        let mut args = Vec::new();
        match spec.mode {
            CodexLaunchMode::InteractiveTui => {
                args.push(spec.prompt);
            }
            CodexLaunchMode::Exec => {
                args.push("exec".to_owned());
                args.push(spec.prompt);
            }
        }

        AgentCommand {
            provider: AgentProvider::Codex,
            mode: spec.mode,
            command: CommandSpec {
                program: self.binary.to_string_lossy().into_owned(),
                args,
                cwd: spec.project.root,
                env: Vec::new(),
            },
        }
    }
}

pub fn discover_codex_binary() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join("codex"))
            .find(|candidate| candidate.is_file())
    })
}

pub fn discover_claude_binary() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join("claude"))
            .find(|candidate| candidate.is_file())
    })
}
