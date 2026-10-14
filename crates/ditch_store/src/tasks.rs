use super::*;
use ditch_core::{
    Task, TaskActor, TaskAudit, TaskCondition, TaskDetail, TaskError, TaskErrorCode, TaskRequest,
    TaskResponse,
};
use rusqlite::OptionalExtension;

impl DitchStore {
    pub(super) fn initialize_tasks(&mut self) -> Result<(), StoreError> {
        ensure_column(&self.connection, "tasks", "task_json", "TEXT")?;
        ensure_column(
            &self.connection,
            "tasks",
            "revision",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        ensure_column(
            &self.connection,
            "tasks",
            "order_key",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_audit (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
            task_id TEXT NOT NULL REFERENCES tasks(id), audit_json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS task_audit_task ON task_audit(task_id,sequence);
            CREATE INDEX IF NOT EXISTS task_project_order ON tasks(project_id,order_key,id);
            CREATE TABLE IF NOT EXISTS writer_reservations (
                agent_id TEXT PRIMARY KEY, lease_json TEXT NOT NULL, process_group_id INTEGER);
            CREATE TABLE IF NOT EXISTS task_requests (
                id TEXT PRIMARY KEY, request_json TEXT NOT NULL, response_json TEXT NOT NULL);",
        )?;
        ensure_column(
            &self.connection,
            "writer_reservations",
            "validator",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        let tx = self.connection.transaction()?;
        // The relational legacy columns remain valid. Backfill only missing JSON.
        let legacy = {
            let mut stmt = tx.prepare("SELECT id,project_id,title,description,state,acceptance_criteria_json,created_at FROM tasks WHERE task_json IS NULL ORDER BY created_at,id")?;
            stmt.query_map([], |r| {
                let value = serde_json::json!({
                    "id": r.get::<_,String>(0)?, "project_id":r.get::<_,String>(1)?,
                    "title":r.get::<_,String>(2)?, "description":r.get::<_,String>(3)?,
                    "state":serde_json::from_str::<serde_json::Value>(&r.get::<_,String>(4)?).unwrap_or_else(|_| serde_json::Value::String(r.get::<_,String>(4).unwrap_or_default())),
                    "acceptance_criteria":serde_json::from_str::<serde_json::Value>(&r.get::<_,String>(5)?).unwrap_or(serde_json::json!([])),
                    "created_at":r.get::<_,String>(6)? });
                from_json::<Task>(&value.to_string())
            })?.collect::<Result<Vec<_>,_>>()?
        };
        for (index, mut task) in legacy.into_iter().enumerate() {
            task.order_key = (index as i64 + 1) * 1024;
            task.updated_at = Some(task.created_at);
            match task.state {
                ditch_core::TaskState::Cancelled => {
                    task.archived = true;
                    task.condition = TaskCondition::Cancelled;
                }
                ditch_core::TaskState::Blocked | ditch_core::TaskState::Running => {
                    task.condition = TaskCondition::Blocked
                }
                _ => {}
            }
            write_task(&tx, &task)?;
            write_audit(
                &tx,
                &task.audit(
                    TaskActor::Daemon,
                    "legacy_import",
                    None,
                    Some("Imported legacy task state without inventing review evidence.".into()),
                ),
            )?;
        }
        // Existing serialized links are authoritative only when their task exists.
        tx.execute("UPDATE agents SET task_id=json_extract(run_json,'$.task_id') WHERE json_valid(run_json) AND json_extract(run_json,'$.task_id') IN (SELECT id FROM tasks)", [])?;
        tx.execute(
            "INSERT OR IGNORE INTO community_schema_migrations(version,applied_at) VALUES(4,?1),(5,?1)",
            params![Utc::now().to_rfc3339()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn load_tasks(&self) -> Result<Vec<Task>, StoreError> {
        let mut stmt = self
            .connection
            .prepare("SELECT task_json FROM tasks ORDER BY order_key,id")?;
        Ok(stmt
            .query_map([], |r| from_json(&r.get::<_, String>(0)?))?
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub fn task_detail(&self, task: &Task) -> Result<TaskDetail, StoreError> {
        let mut stmt = self
            .connection
            .prepare("SELECT audit_json FROM task_audit WHERE task_id=?1 ORDER BY sequence")?;
        let history = stmt
            .query_map(params![task.id.0.to_string()], |r| {
                from_json(&r.get::<_, String>(0)?)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TaskDetail {
            task: task.clone(),
            history,
        })
    }

    pub fn set_task_setting_receipt(
        &mut self,
        key: &str,
        value: &str,
        request: &TaskRequest,
        response: &TaskResponse,
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        tx.execute("INSERT INTO app_settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
        tx.execute(
            "INSERT INTO task_requests(id,request_json,response_json) VALUES(?1,?2,?3)",
            params![
                request.request_id.to_string(),
                to_json(request)?,
                to_json(response)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn task_replay(&self, request: &TaskRequest) -> Result<Option<TaskResponse>, StoreError> {
        let row: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT request_json,response_json FROM task_requests WHERE id=?1",
                params![request.request_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((original, response)) if original == to_json(request)? => {
                let saved: TaskResponse = from_json(&response)?;
                let current = match saved {
                    TaskResponse::Changed(task) => self
                        .load_tasks()?
                        .into_iter()
                        .find(|current| current.id == task.id)
                        .map(TaskResponse::Changed)
                        .unwrap_or(TaskResponse::Deleted(task.id)),
                    other => other,
                };
                Ok(Some(current))
            }
            Some(_) => Ok(Some(TaskResponse::Error(TaskError::new(
                TaskErrorCode::IdempotencyConflict,
                "This request ID was already used for different task input.",
            )))),
            None => Ok(None),
        }
    }

    /// Persist projection, append-only history, optional agent link and retry receipt
    /// together. Publish only after this transaction commits.
    /// Compose validated task creation with an edition-owned record in one transaction.
    pub fn save_task_group<T>(
        &mut self,
        namespace: &str,
        changes: &[(Task, TaskAudit)],
        extension: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.with_extension_transaction(namespace, |tx| {
            for (task, audit) in changes {
                write_task(tx, task)?;
                write_audit(tx, audit)?;
            }
            extension(tx)
        })
    }

    pub fn save_task_changes(
        &mut self,
        changes: &[(Task, TaskAudit)],
        receipt: Option<(&TaskRequest, &TaskResponse)>,
        linked_agent: Option<&AgentRun>,
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        for (task, audit) in changes {
            write_task(&tx, task)?;
            write_audit(&tx, audit)?;
        }
        if let Some(agent) = linked_agent {
            tx.execute(
                "UPDATE agents SET task_id=?2,run_json=?3 WHERE id=?1",
                params![
                    agent.id.0.to_string(),
                    agent.task_id.map(|id| id.0.to_string()),
                    to_json(agent)?
                ],
            )?;
        }
        if let Some((request, response)) = receipt {
            tx.execute(
                "INSERT INTO task_requests(id,request_json,response_json) VALUES(?1,?2,?3)",
                params![
                    request.request_id.to_string(),
                    to_json(request)?,
                    to_json(response)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete_draft_task(
        &mut self,
        task: &Task,
        request: &TaskRequest,
    ) -> Result<(), StoreError> {
        let tx = self.connection.transaction()?;
        let references: i64 = tx.query_row("SELECT (SELECT COUNT(*) FROM agents WHERE task_id=?1) + (SELECT COUNT(*) FROM task_audit WHERE task_id=?1 AND json_extract(audit_json,'$.action')!='create')", params![task.id.0.to_string()], |r| r.get(0))?;
        if task.revision != 1 || task.assigned_agent_id.is_some() || references > 0 {
            return Err(StoreError::InvalidData("Only an untouched draft without references or history can be deleted. Cancel/archive this task instead.".into()));
        }
        tx.execute(
            "DELETE FROM task_audit WHERE task_id=?1",
            params![task.id.0.to_string()],
        )?;
        tx.execute(
            "DELETE FROM tasks WHERE id=?1",
            params![task.id.0.to_string()],
        )?;
        tx.execute(
            "INSERT INTO task_requests(id,request_json,response_json) VALUES(?1,?2,?3)",
            params![
                request.request_id.to_string(),
                to_json(request)?,
                to_json(&TaskResponse::Deleted(task.id))?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn reconcile_tasks(&mut self) -> Result<(), StoreError> {
        for mut task in self.load_tasks()? {
            if matches!(
                task.condition,
                TaskCondition::Running | TaskCondition::Queued | TaskCondition::AwaitingApproval
            ) {
                for attempt in task
                    .acceptance
                    .attempts
                    .iter_mut()
                    .filter(|a| a.result.active())
                {
                    attempt.result = ditch_core::AttemptResult::Interrupted;
                    attempt.ended_at = Some(Utc::now());
                    attempt.failure=Some(ditch_core::AttemptFailure{code:"daemon_restart".into(),message:"Child ownership is stale. Completed validators retained; explicit revalidation required.".into()});
                }
                task.condition = TaskCondition::Blocked;
                task.state = ditch_core::TaskState::Running;
                task.acceptance.current_submission = None;
                task.last_reason = Some("Runtime restarted. Prior child ownership is stale; inspect the agent and explicitly restart the task.".into());
                task.touch();
                let audit = task.audit(
                    TaskActor::Daemon,
                    "restart_recovery",
                    Some(task.column()),
                    task.last_reason.clone(),
                );
                self.save_task_changes(&[(task, audit)], None, None)?;
            }
        }
        Ok(())
    }

    pub fn reserve_writer(
        &mut self,
        id: ditch_core::AgentId,
        lease: &ditch_core::WorkspaceLease,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO writer_reservations(agent_id,lease_json) VALUES(?1,?2)",
            params![id.0.to_string(), to_json(lease)?],
        )?;
        Ok(())
    }
    pub fn writer_stopped(&mut self, id: ditch_core::AgentId) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE writer_reservations SET process_group_id=NULL,validator=0 WHERE agent_id=?1",
            params![id.0.to_string()],
        )?;
        Ok(())
    }
    pub fn validator_started(
        &mut self,
        id: ditch_core::AgentId,
        pid: i32,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE writer_reservations SET process_group_id=?2,validator=1 WHERE agent_id=?1",
            params![id.0.to_string(), pid],
        )?;
        Ok(())
    }
    pub fn validator_owners(
        &self,
    ) -> Result<std::collections::HashSet<ditch_core::AgentId>, StoreError> {
        let mut stmt = self
            .connection
            .prepare("SELECT agent_id FROM writer_reservations WHERE validator=1")?;
        Ok(stmt
            .query_map([], |r| Ok(ditch_core::AgentId(parse_uuid(r.get(0)?)?)))?
            .collect::<Result<_, _>>()?)
    }
    pub fn writer_started(&mut self, id: ditch_core::AgentId, pid: i32) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE writer_reservations SET process_group_id=?2,validator=0 WHERE agent_id=?1",
            params![id.0.to_string(), pid],
        )?;
        Ok(())
    }
    pub fn transfer_writer(
        &mut self,
        old: ditch_core::AgentId,
        new: ditch_core::AgentId,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE writer_reservations SET agent_id=?2 WHERE agent_id=?1",
            params![old.0.to_string(), new.0.to_string()],
        )?;
        Ok(())
    }
    pub fn release_writer(&mut self, id: ditch_core::AgentId) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM writer_reservations WHERE agent_id=?1",
            params![id.0.to_string()],
        )?;
        Ok(())
    }

    pub fn update_writer_lease(
        &mut self,
        id: ditch_core::AgentId,
        lease: &ditch_core::WorkspaceLease,
    ) -> Result<(), StoreError> {
        if self.connection.execute(
            "UPDATE writer_reservations SET lease_json=?2 WHERE agent_id=?1",
            params![id.0.to_string(), to_json(lease)?],
        )? != 1
        {
            return Err(StoreError::InvalidData(
                "Writer reservation is missing".into(),
            ));
        }
        Ok(())
    }
    pub fn writer_reservations(
        &self,
    ) -> Result<Vec<(ditch_core::AgentId, ditch_core::WorkspaceLease, Option<i32>)>, StoreError>
    {
        let mut stmt = self
            .connection
            .prepare("SELECT agent_id,lease_json,process_group_id FROM writer_reservations")?;
        Ok(stmt
            .query_map([], |r| {
                Ok((
                    ditch_core::AgentId(parse_uuid(r.get(0)?)?),
                    from_json(&r.get::<_, String>(1)?)?,
                    r.get(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

fn write_task(tx: &Transaction<'_>, task: &Task) -> Result<(), StoreError> {
    tx.execute("INSERT INTO tasks(id,project_id,title,description,state,acceptance_criteria_json,created_at,task_json,revision,order_key) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
        ON CONFLICT(id) DO UPDATE SET title=excluded.title,description=excluded.description,state=excluded.state,acceptance_criteria_json=excluded.acceptance_criteria_json,task_json=excluded.task_json,revision=excluded.revision,order_key=excluded.order_key",
        params![task.id.0.to_string(),task.project_id.0.to_string(),task.title,task.description,to_json(&task.state)?,to_json(&task.acceptance_criteria)?,task.created_at.to_rfc3339(),to_json(task)?,task.revision as i64,task.order_key])?;
    Ok(())
}
fn write_audit(tx: &Transaction<'_>, audit: &TaskAudit) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO task_audit(id,task_id,audit_json) VALUES(?1,?2,?3)",
        params![
            audit.id.to_string(),
            audit.task_id.0.to_string(),
            to_json(audit)?
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ditch_core::{TaskColumn, TaskDraft, TaskPriority, TaskState};
    #[test]
    fn legacy_rows_keep_serialized_states_and_unknown_commercial_tables() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        connection.execute_batch("CREATE TABLE private_extension(value TEXT); INSERT INTO private_extension VALUES('keep');").unwrap();
        let mut store = DitchStore { connection };
        let project = Project::new("Legacy", "/tmp/legacy");
        store.upsert_project(&project).unwrap();
        for state in [
            TaskState::Draft,
            TaskState::Ready,
            TaskState::Running,
            TaskState::Blocked,
            TaskState::InReview,
            TaskState::Accepted,
            TaskState::Rejected,
            TaskState::Cancelled,
        ] {
            store.connection.execute("INSERT INTO tasks(id,project_id,title,description,state,acceptance_criteria_json,created_at) VALUES(?1,?2,'Legacy','Description',?3,'[\"Review\"]',?4)",params![uuid::Uuid::new_v4().to_string(),project.id.0.to_string(),to_json(&state).unwrap(),Utc::now().to_rfc3339()]).unwrap();
        }
        store.initialize_tasks().unwrap();
        store.initialize_tasks().unwrap();
        let tasks = store.load_tasks().unwrap();
        assert_eq!(tasks.len(), 8);
        for task in &tasks {
            assert_eq!(store.task_detail(task).unwrap().history.len(), 1);
            assert_eq!(task.review_summary, None);
        }
        assert!(
            tasks
                .iter()
                .any(|t| t.state == TaskState::Accepted && t.column() == TaskColumn::Done)
        );
        assert!(tasks.iter().any(|t| t.state == TaskState::Cancelled
            && t.archived
            && t.condition == TaskCondition::Cancelled));
        let value: String = store
            .connection
            .query_row("SELECT value FROM private_extension", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "keep");
    }
    #[test]
    fn task_mutation_history_and_receipt_are_atomic_on_storage_failure() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA).unwrap();
        let mut store = DitchStore { connection };
        store.initialize_tasks().unwrap();
        let project = Project::new("Atomic", "/tmp/atomic");
        store.upsert_project(&project).unwrap();
        let task = Task::new(
            project.id,
            TaskDraft {
                skills: vec![],
                title: "Atomic".into(),
                description: String::new(),
                acceptance_criteria: vec![],
                priority: TaskPriority::Normal,
            },
            1024,
        )
        .unwrap();
        let audit = task.audit(TaskActor::User, "create", None, None);
        store
            .save_task_changes(&[(task.clone(), audit.clone())], None, None)
            .unwrap();
        let mut next = task.clone();
        next.title = "Must roll back".into();
        next.touch();
        // Duplicate immutable audit ID fails after the attempted projection write.
        assert!(
            store
                .save_task_changes(&[(next, audit)], None, None)
                .is_err()
        );
        assert_eq!(store.load_tasks().unwrap(), vec![task]);
    }
}

#[cfg(test)]
mod hardening_tests {
    use super::*;
    use ditch_core::{TaskDraft, TaskPriority};

    #[test]
    fn every_intermediate_schema_reopens_without_losing_rows_or_unknown_metadata() {
        for version in 0..=5 {
            let root =
                std::env::temp_dir().join(format!("ditch-migration-{}", uuid::Uuid::new_v4()));
            let paths = AppPaths {
                data_dir: root.clone(),
                database_path: root.join("db"),
                socket_path: root.join("sock"),
                logs_dir: root.join("logs"),
                scrollback_dir: root.join("scrollback"),
            };
            ensure_app_dirs(&paths).unwrap();
            let mut store = DitchStore::open(&paths).unwrap();
            let project = Project::new("Preserve", root.join("project"));
            store.upsert_project(&project).unwrap();
            let mut ids = Vec::new();
            for n in 0..1000 {
                let task = Task::new(
                    project.id,
                    TaskDraft {
                        title: format!("Task {n}"),
                        description: "Keep".into(),
                        acceptance_criteria: vec![],
                        priority: TaskPriority::Normal,
                        skills: vec![],
                    },
                    n * 1024,
                )
                .unwrap();
                ids.push(task.id);
                store
                    .save_task_changes(
                        &[(
                            task.clone(),
                            task.audit(TaskActor::User, "create", None, None),
                        )],
                        None,
                        None,
                    )
                    .unwrap();
            }
            store.connection.execute_batch("CREATE TABLE opaque_private_data(value TEXT); INSERT INTO opaque_private_data VALUES('do not interpret');").unwrap();
            // Optional, uninterpreted extension data must not prevent core recovery.
            store
                .set_setting("unknown.optional.metadata", "{broken JSON")
                .unwrap();
            if version < 4 {
                store.connection.execute_batch("UPDATE tasks SET task_json=NULL; DELETE FROM task_audit; DROP TABLE writer_reservations; DROP TABLE task_requests;").unwrap();
            } else if version == 4 {
                store
                    .connection
                    .execute_batch("ALTER TABLE writer_reservations DROP COLUMN validator;")
                    .unwrap();
            }
            store
                .connection
                .execute(
                    "DELETE FROM community_schema_migrations WHERE version>?1",
                    [version],
                )
                .unwrap();
            store
                .connection
                .pragma_update(None, "user_version", version.min(3))
                .unwrap();
            drop(store);
            for _ in 0..2 {
                let reopened = DitchStore::open(&paths).unwrap();
                let state = reopened.load().unwrap();
                assert_eq!(state.projects.len(), 1);
                assert_eq!(state.tasks.len(), ids.len());
                for id in &ids {
                    assert!(state.tasks.iter().any(|t| t.id == *id));
                }
                assert_eq!(
                    reopened
                        .setting("unknown.optional.metadata")
                        .unwrap()
                        .as_deref(),
                    Some("{broken JSON")
                );
                let private: String = reopened
                    .connection
                    .query_row("SELECT value FROM opaque_private_data", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(private, "do not interpret");
                let problems: i64 = reopened
                    .connection
                    .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(problems, 0);
                assert_eq!(
                    reopened.task_detail(&state.tasks[0]).unwrap().history.len(),
                    1
                );
                assert!(
                    state
                        .tasks
                        .windows(2)
                        .all(|w| w[0].order_key <= w[1].order_key)
                );
            }
            fs::remove_dir_all(root).unwrap();
        }
    }
}
