use chrono::{DateTime, Utc};
use ditch_core::{AgentResumeBlockReason, AgentRun, AgentState, AppPaths, Project, ProjectId};
use ditch_protocol::{AgentChatMessage, AgentMessagePage, RuntimeAttention, SequencedAgentMessage};
use ditch_remote::{AttentionProjection, ProjectProjection, SessionProjection};
use rusqlite::{Connection, Transaction, params};
use std::collections::HashSet;
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
        connection.pragma_update(None, "user_version", 2)?;
        let mut store = Self { connection };
        store.cleanup_polluted_permission_alerts()?;
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

    fn cleanup_polluted_permission_alerts(&mut self) -> Result<(), StoreError> {
        let mut stmt = self.connection.prepare(
            "SELECT agent_id,attention_json FROM attention_events
             WHERE dismissed_at IS NULL AND agent_id IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    from_json::<RuntimeAttention>(&row.get::<_, String>(1)?)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut polluted_agents = HashSet::new();
        for (agent_id, attention) in &rows {
            if attention.title != "Codex cannot write this project" {
                continue;
            }
            let stored = self.connection.query_row(
                "SELECT run_json,terminal_failure FROM agents WHERE id=?1",
                params![agent_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            );
            let Ok((run_json, terminal_failure)) = stored else {
                continue;
            };
            let run: AgentRun = from_json(&run_json)?;
            if run.state == AgentState::Failed
                && run.exit_code == Some(0)
                && (looks_like_embedded_content(&attention.body)
                    || terminal_failure
                        .as_deref()
                        .is_some_and(looks_like_embedded_content))
            {
                polluted_agents.insert(agent_id.clone());
            }
        }
        if polluted_agents.is_empty() {
            return Ok(());
        }

        let now = Utc::now().to_rfc3339();
        let tx = self.connection.transaction()?;
        for agent_id in &polluted_agents {
            tx.execute(
                "UPDATE attention_events SET dismissed_at=?2
                 WHERE agent_id=?1 AND dismissed_at IS NULL
                   AND json_extract(attention_json,'$.kind') IN ('Blocked','Failed')",
                params![agent_id, now],
            )?;
            let stored = tx.query_row(
                "SELECT run_json,terminal_failure FROM agents WHERE id=?1",
                params![agent_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            );
            let Ok((run_json, terminal_failure)) = stored else {
                continue;
            };
            let mut run: AgentRun = from_json(&run_json)?;
            if run.state == AgentState::Failed
                && run.exit_code == Some(0)
                && terminal_failure
                    .as_deref()
                    .is_some_and(looks_like_embedded_content)
            {
                if let Some(failure) = terminal_failure.as_deref() {
                    tx.execute(
                        "DELETE FROM agent_messages
                         WHERE agent_id=?1
                           AND json_extract(message_json,'$.role')='System'
                           AND json_extract(message_json,'$.text')=?2",
                        params![agent_id, failure],
                    )?;
                }
                run.state = AgentState::Completed;
                run.state_evidence =
                    "Successful exit restored after removing a legacy false permission alert."
                        .to_owned();
                tx.execute(
                    "UPDATE agents SET state=?2,run_json=?3,terminal_failure=NULL WHERE id=?1",
                    params![agent_id, to_json(&AgentState::Completed)?, to_json(&run)?],
                )?;
            }
        }
        tx.commit()?;
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
                "SELECT message_json FROM (
                   SELECT sequence,message_json FROM agent_messages
                   WHERE agent_id = ?1 ORDER BY sequence DESC LIMIT 200
                 ) ORDER BY sequence",
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

    pub fn setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        match self.connection.query_row(
            "SELECT value FROM app_settings WHERE key=?1",
            params![key],
            |row| row.get(0),
        ) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO app_settings(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn upsert_project(&mut self, project: &Project) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO projects(id,name,root,created_at,archived_at,git_policy) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(id) DO UPDATE SET name=excluded.name,root=excluded.root,archived_at=excluded.archived_at,git_policy=excluded.git_policy",
            params![project.id.0.to_string(), project.name, project.root.to_string_lossy(), project.created_at.to_rfc3339(), project.archived_at.map(|v| v.to_rfc3339()), to_json(&project.git_policy)?],
        )?;
        Ok(())
    }

    /// Removes only Ditch's persisted state for a project. The project
    /// directory and its `.ditch` metadata are deliberately never touched.
    pub fn delete_project(&mut self, project_id: ProjectId) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let id = project_id.0.to_string();
        tx.execute(
            "DELETE FROM permission_requests
             WHERE project_id=?1 OR agent_id IN (SELECT id FROM agents WHERE project_id=?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM attention_events
             WHERE project_id=?1 OR agent_id IN (SELECT id FROM agents WHERE project_id=?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM agent_messages WHERE agent_id IN (SELECT id FROM agents WHERE project_id=?1)",
            params![id],
        )?;
        tx.execute("DELETE FROM agents WHERE project_id=?1", params![id])?;
        tx.execute("DELETE FROM tasks WHERE project_id=?1", params![id])?;
        tx.execute("DELETE FROM projects WHERE id=?1", params![id])?;
        tx.commit()?;
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

    pub fn list_agent_messages(
        &self,
        agent_id: ditch_core::AgentId,
        before_sequence: Option<u64>,
        limit: u16,
    ) -> Result<AgentMessagePage, StoreError> {
        let limit = usize::from(limit.clamp(1, 200));
        let before = before_sequence.unwrap_or(u64::MAX);
        let mut stmt = self.connection.prepare(
            "SELECT sequence,message_json FROM agent_messages
             WHERE agent_id=?1 AND sequence < ?2
             ORDER BY sequence DESC LIMIT ?3",
        )?;
        let mut newest_first = stmt
            .query_map(
                params![
                    agent_id.0.to_string(),
                    i64::try_from(before).unwrap_or(i64::MAX),
                    i64::try_from(limit + 1).unwrap_or(201),
                ],
                |row| {
                    Ok(SequencedAgentMessage {
                        sequence: u64::try_from(row.get::<_, i64>(0)?).unwrap_or_default(),
                        message: from_json(&row.get::<_, String>(1)?)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = newest_first.len() > limit;
        if has_more {
            newest_first.truncate(limit);
        }
        newest_first.reverse();
        Ok(AgentMessagePage {
            agent_id,
            next_before_sequence: has_more
                .then(|| newest_first.first().map(|item| item.sequence))
                .flatten(),
            has_more,
            messages: newest_first,
        })
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

    pub fn mark_attention_read(
        &mut self,
        attention: &[RuntimeAttention],
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        for item in attention {
            tx.execute(
                "UPDATE attention_events SET attention_json=?2 WHERE id=?1 AND dismissed_at IS NULL",
                params![item.id.to_string(), to_json(item)?],
            )?;
        }
        tx.commit()?;
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
                    | AgentState::Stopping
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
                self.append_message(&AgentChatMessage { agent_id: agent.run.id, role: ditch_protocol::AgentChatRole::System, text: "Ditch Runtime restarted while this session was active. Start a new prompt to resume it safely.".to_owned(), created_at: Utc::now() })?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn remote_machine(&self) -> Result<Option<RemoteMachineRecord>, StoreError> {
        match self.connection.query_row(
            "SELECT machine_id,owner_id,name,signing_public_key,agreement_public_key,key_version,enabled,projection_epoch,projection_sequence FROM remote_machine WHERE singleton=1",
            [],
            |row| Ok(RemoteMachineRecord { machine_id: parse_uuid(row.get(0)?)?, owner_id: row.get::<_, Option<String>>(1)?.map(parse_uuid).transpose()?, name: row.get(2)?, signing_public_key: row.get(3)?, agreement_public_key: row.get(4)?, key_version: row.get::<_, u32>(5)?, enabled: row.get::<_, i64>(6)? != 0, projection_epoch: parse_uuid(row.get(7)?)?, projection_sequence: u64::try_from(row.get::<_, i64>(8)?).unwrap_or_default() }),
        ) {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn upsert_remote_machine(
        &mut self,
        machine: &RemoteMachineRecord,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO remote_machine(singleton,machine_id,owner_id,name,signing_public_key,agreement_public_key,key_version,enabled,projection_epoch,projection_sequence,updated_at) VALUES(1,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(singleton) DO UPDATE SET machine_id=excluded.machine_id,owner_id=excluded.owner_id,name=excluded.name,signing_public_key=excluded.signing_public_key,agreement_public_key=excluded.agreement_public_key,key_version=excluded.key_version,enabled=excluded.enabled,projection_epoch=excluded.projection_epoch,projection_sequence=excluded.projection_sequence,updated_at=excluded.updated_at",
            params![machine.machine_id.to_string(), machine.owner_id.map(|value| value.to_string()), machine.name, machine.signing_public_key, machine.agreement_public_key, machine.key_version, i64::from(machine.enabled), machine.projection_epoch.to_string(), i64::try_from(machine.projection_sequence).unwrap_or(i64::MAX), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_remote_enabled(&mut self, enabled: bool) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE remote_machine SET enabled=?,updated_at=? WHERE singleton=1",
            params![i64::from(enabled), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn upsert_remote_device(&mut self, device: &RemoteDeviceRecord) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO remote_devices(device_id,name,signing_public_key,agreement_public_key,key_version,state,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?)
             ON CONFLICT(device_id) DO UPDATE SET name=excluded.name,signing_public_key=excluded.signing_public_key,agreement_public_key=excluded.agreement_public_key,key_version=excluded.key_version,state=excluded.state,last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at",
            params![device.device_id.to_string(), device.name, device.signing_public_key, device.agreement_public_key, device.key_version, device.state, device.last_seen_at.map(|value| value.to_rfc3339()), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn remote_devices(&self) -> Result<Vec<RemoteDeviceRecord>, StoreError> {
        let mut statement = self.connection.prepare("SELECT device_id,name,signing_public_key,agreement_public_key,key_version,state,last_seen_at FROM remote_devices ORDER BY name")?;
        Ok(statement
            .query_map([], |row| {
                Ok(RemoteDeviceRecord {
                    device_id: parse_uuid(row.get(0)?)?,
                    name: row.get(1)?,
                    signing_public_key: row.get(2)?,
                    agreement_public_key: row.get(3)?,
                    key_version: row.get(4)?,
                    state: row.get(5)?,
                    last_seen_at: row
                        .get::<_, Option<String>>(6)?
                        .map(parse_time)
                        .transpose()?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn delete_remote_device(&mut self, device_id: uuid::Uuid) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM remote_devices WHERE device_id=?",
            params![device_id.to_string()],
        )?;
        Ok(())
    }

    /// Replaces the local cache with the relay's complete owner-scoped active
    /// device roster. The transaction prevents command authorization from
    /// observing a partially reconciled list.
    pub fn replace_remote_devices(
        &mut self,
        devices: &[RemoteDeviceRecord],
    ) -> Result<(), StoreError> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM remote_devices", [])?;
        for device in devices {
            transaction.execute(
                "INSERT INTO remote_devices(device_id,name,signing_public_key,agreement_public_key,key_version,state,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?)",
                params![device.device_id.to_string(), device.name, device.signing_public_key, device.agreement_public_key, device.key_version, device.state, device.last_seen_at.map(|value| value.to_rfc3339()), Utc::now().to_rfc3339()],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Reserves an idempotency key before any remote effect. A false result is
    /// a deterministic duplicate and must return the stored outcome.
    pub fn begin_remote_command(
        &mut self,
        command_id: uuid::Uuid,
        idempotency_key: &str,
        command_type: &str,
        device_id: uuid::Uuid,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO remote_command_outcomes(command_id,idempotency_key,command_type,device_id,status,received_at,expires_at) VALUES(?,?,?,?,'received',?,?)",
            params![command_id.to_string(), idempotency_key, command_type, device_id.to_string(), Utc::now().to_rfc3339(), expires_at.to_rfc3339()],
        )?;
        Ok(changed == 1)
    }

    pub fn finish_remote_command(
        &mut self,
        command_id: uuid::Uuid,
        status: &str,
        error_code: Option<&str>,
        result_json: Option<&str>,
    ) -> Result<(), StoreError> {
        self.connection.execute("UPDATE remote_command_outcomes SET status=?,error_code=?,result_json=?,finished_at=? WHERE command_id=?", params![status, error_code, result_json, Utc::now().to_rfc3339(), command_id.to_string()])?;
        Ok(())
    }

    pub fn replace_remote_projection_cache(
        &mut self,
        epoch: uuid::Uuid,
        sequence: u64,
        projects: &[ProjectProjection],
        sessions: &[SessionProjection],
        attention: &[AttentionProjection],
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        tx.execute("DELETE FROM remote_projection_cache", [])?;
        for (kind, id, value) in projects
            .iter()
            .map(|value| ("project", value.project_id, serde_json::to_string(value)))
            .chain(
                sessions
                    .iter()
                    .map(|value| ("session", value.session_id, serde_json::to_string(value))),
            )
            .chain(attention.iter().map(|value| {
                (
                    "attention",
                    value.attention_id,
                    serde_json::to_string(value),
                )
            }))
        {
            tx.execute(
                "INSERT INTO remote_projection_cache(kind,record_id,projection_json) VALUES(?,?,?)",
                params![
                    kind,
                    id.to_string(),
                    value.map_err(|error| StoreError::InvalidData(error.to_string()))?
                ],
            )?;
        }
        tx.execute("UPDATE remote_machine SET projection_epoch=?,projection_sequence=?,updated_at=? WHERE singleton=1", params![epoch.to_string(), i64::try_from(sequence).unwrap_or(i64::MAX), Utc::now().to_rfc3339()])?;
        tx.commit()?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct RemoteMachineRecord {
    pub machine_id: uuid::Uuid,
    pub owner_id: Option<uuid::Uuid>,
    pub name: String,
    pub signing_public_key: String,
    pub agreement_public_key: String,
    pub key_version: u32,
    pub enabled: bool,
    pub projection_epoch: uuid::Uuid,
    pub projection_sequence: u64,
}

#[derive(Clone, Debug)]
pub struct RemoteDeviceRecord {
    pub device_id: uuid::Uuid,
    pub name: String,
    pub signing_public_key: String,
    pub agreement_public_key: String,
    pub key_version: u32,
    pub state: String,
    pub last_seen_at: Option<DateTime<Utc>>,
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

fn looks_like_embedded_content(value: &str) -> bool {
    value.chars().count() > 400
        || (value.contains('\n')
            && [
                "diff --git",
                "apply_patch",
                "fn ",
                "class ",
                "let ",
                "widget.",
                "workspace_permission_denial",
            ]
            .iter()
            .any(|marker| value.contains(marker)))
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
            can_stop: false,
            launch_mode: ditch_core::CodexLaunchMode::Exec,
            execution_profile: Default::default(),
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
            for index in 2..=5 {
                store
                    .append_message(&AgentChatMessage {
                        agent_id: run.id,
                        role: ditch_protocol::AgentChatRole::Assistant,
                        text: format!("message-{index}"),
                        created_at: now,
                    })
                    .unwrap();
            }
            let latest = store.list_agent_messages(run.id, None, 2).unwrap();
            assert_eq!(
                latest
                    .messages
                    .iter()
                    .map(|item| item.sequence)
                    .collect::<Vec<_>>(),
                vec![4, 5]
            );
            assert!(latest.has_more);
            let older = store
                .list_agent_messages(run.id, latest.next_before_sequence, 2)
                .unwrap();
            assert_eq!(
                older
                    .messages
                    .iter()
                    .map(|item| item.sequence)
                    .collect::<Vec<_>>(),
                vec![2, 3]
            );
            assert!(older.has_more);
        }
        {
            let mut store = DitchStore::open(&paths).unwrap();
            assert_eq!(store.reconcile_active_agents().unwrap(), 1);
            let restored = store.load().unwrap();
            assert_eq!(restored.projects, vec![project]);
            assert_eq!(restored.agents.len(), 1);
            assert_eq!(restored.agents[0].run.state, AgentState::Stale);
            assert_eq!(restored.agents[0].messages.len(), 6);
            assert!(restored.agents[0].messages[5].text.contains("restarted"));
            store.delete_agent(run.id).unwrap();
            let deleted = store.load().unwrap();
            assert!(deleted.agents.is_empty());
            assert!(deleted.attention.is_empty());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_device_roster_replacement_and_revocation_delete_stale_rows() {
        let root = std::env::temp_dir().join(format!(
            "ditch-store-remote-devices-{}",
            uuid::Uuid::new_v4()
        ));
        let paths = AppPaths {
            data_dir: root.clone(),
            database_path: root.join("ditch.sqlite3"),
            socket_path: root.join("ditchd.sock"),
            logs_dir: root.join("logs"),
            scrollback_dir: root.join("scrollback"),
        };
        ensure_app_dirs(&paths).unwrap();
        let first = RemoteDeviceRecord {
            device_id: uuid::Uuid::new_v4(),
            name: "First iPhone".into(),
            signing_public_key: "first-signing".into(),
            agreement_public_key: "first-agreement".into(),
            key_version: 1,
            state: "active".into(),
            last_seen_at: Some(Utc::now()),
        };
        let second = RemoteDeviceRecord {
            device_id: uuid::Uuid::new_v4(),
            name: "Second iPhone".into(),
            signing_public_key: "second-signing".into(),
            agreement_public_key: "second-agreement".into(),
            key_version: 2,
            state: "active".into(),
            last_seen_at: None,
        };
        let mut store = DitchStore::open(&paths).unwrap();
        store.upsert_remote_device(&first).unwrap();
        store
            .replace_remote_devices(std::slice::from_ref(&second))
            .unwrap();
        let devices = store.remote_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, second.device_id);

        store.delete_remote_device(second.device_id).unwrap();
        assert!(store.remote_devices().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persists_attention_read_state() {
        let root = std::env::temp_dir().join(format!(
            "ditch-store-attention-read-{}",
            uuid::Uuid::new_v4()
        ));
        let paths = AppPaths {
            data_dir: root.clone(),
            database_path: root.join("ditch.sqlite3"),
            socket_path: root.join("ditchd.sock"),
            logs_dir: root.join("logs"),
            scrollback_dir: root.join("scrollback"),
        };
        ensure_app_dirs(&paths).unwrap();
        let now = Utc::now();
        let mut attention = RuntimeAttention {
            id: uuid::Uuid::new_v4(),
            kind: ditch_core::AttentionKind::Completed,
            agent_id: None,
            project_id: None,
            project_name: None,
            agent_name: None,
            title: "Finished".into(),
            body: "Done".into(),
            created_at: now,
            read_at: None,
        };

        {
            let mut store = DitchStore::open(&paths).unwrap();
            store.upsert_attention(&attention).unwrap();
            attention.read_at = Some(now);
            store.mark_attention_read(&[attention.clone()]).unwrap();
        }

        let restored = DitchStore::open(&paths).unwrap().load().unwrap();
        assert_eq!(restored.attention, vec![attention]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deleting_project_removes_only_app_owned_state() {
        let root =
            std::env::temp_dir().join(format!("ditch-store-delete-{}", uuid::Uuid::new_v4()));
        let project_root = root.join("user-project");
        let paths = AppPaths {
            data_dir: root.join("app-data"),
            database_path: root.join("app-data/ditch.sqlite3"),
            socket_path: root.join("app-data/ditchd.sock"),
            logs_dir: root.join("app-data/logs"),
            scrollback_dir: root.join("app-data/scrollback"),
        };
        ensure_app_dirs(&paths).unwrap();
        fs::create_dir_all(&project_root).unwrap();
        fs::write(project_root.join("keep-me.txt"), "user data").unwrap();
        let project = Project::new("User project", &project_root);

        let mut store = DitchStore::open(&paths).unwrap();
        store.upsert_project(&project).unwrap();
        store.delete_project(project.id).unwrap();

        assert!(store.load().unwrap().projects.is_empty());
        assert_eq!(
            fs::read_to_string(project_root.join("keep-me.txt")).unwrap(),
            "user data"
        );
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

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS remote_machine (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1),
  machine_id TEXT NOT NULL UNIQUE,
  owner_id TEXT,
  name TEXT NOT NULL,
  signing_public_key TEXT NOT NULL,
  agreement_public_key TEXT NOT NULL,
  key_version INTEGER NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
  projection_epoch TEXT NOT NULL,
  projection_sequence INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS remote_devices (
  device_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  signing_public_key TEXT NOT NULL,
  agreement_public_key TEXT NOT NULL,
  key_version INTEGER NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('active','revoked')),
  last_seen_at TEXT,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS remote_command_outcomes (
  command_id TEXT PRIMARY KEY,
  idempotency_key TEXT NOT NULL UNIQUE,
  command_type TEXT NOT NULL,
  device_id TEXT NOT NULL,
  status TEXT NOT NULL,
  received_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  finished_at TEXT,
  error_code TEXT,
  result_json TEXT
);
CREATE INDEX IF NOT EXISTS remote_command_retention ON remote_command_outcomes(received_at);

CREATE TABLE IF NOT EXISTS remote_projection_cache (
  kind TEXT NOT NULL CHECK(kind IN ('project','session','attention')),
  record_id TEXT NOT NULL,
  projection_json TEXT NOT NULL,
  PRIMARY KEY(kind,record_id)
);
"#;
