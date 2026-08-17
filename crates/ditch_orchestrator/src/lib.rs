use ditch_agents::{CodexAdapter, CodexStartSpec};
use ditch_core::{AgentProvider, AgentRun, AgentState, CodexLaunchMode, Project, TaskId};
use ditch_runtime::{RuntimeController, RuntimeError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartAgentRequest {
    pub project: Project,
    pub task_id: Option<TaskId>,
    pub prompt: String,
    pub mode: CodexLaunchMode,
}

pub struct Orchestrator<R> {
    runtime: R,
    codex: CodexAdapter,
}

impl<R> Orchestrator<R>
where
    R: RuntimeController,
{
    pub fn new(runtime: R, codex: CodexAdapter) -> Self {
        Self { runtime, codex }
    }

    pub fn start_codex(
        &mut self,
        request: StartAgentRequest,
    ) -> Result<AgentRun, OrchestratorError> {
        let command = self.codex.command_for(CodexStartSpec {
            project: request.project.clone(),
            prompt: request.prompt.clone(),
            mode: request.mode.clone(),
        });
        let pane = self.runtime.create_pane(command.command)?;
        let now = chrono::Utc::now();

        Ok(AgentRun {
            id: ditch_core::AgentId::new(),
            provider: AgentProvider::Codex,
            state: AgentState::Starting,
            launch_mode: request.mode,
            project_id: request.project.id,
            task_id: request.task_id,
            pane_id: Some(pane.id),
            native_session_id: None,
            origin_codex_home: None,
            current_prompt: Some(request.prompt),
            last_visible_action: None,
            state_confidence: 0.35,
            state_evidence: "Codex process was launched in an owned runtime pane; semantic lifecycle is not yet verified.".to_owned(),
            started_at: now,
            updated_at: now,
        })
    }
}
