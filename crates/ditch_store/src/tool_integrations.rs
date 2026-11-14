use super::*;
use ditch_core::tool_integrations::ToolOperationReceipt;

impl DitchStore {
    pub(crate) fn initialize_tool_integrations(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS tool_operation_receipts (
                operation_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, agent_id TEXT NOT NULL,
                tool TEXT NOT NULL, state TEXT NOT NULL, updated_at TEXT NOT NULL);
             CREATE INDEX IF NOT EXISTS tool_receipt_age ON tool_operation_receipts(updated_at);
             INSERT OR IGNORE INTO community_schema_migrations(version,applied_at) VALUES(8,datetime('now'));"
        )?;
        Ok(())
    }

    /// Only the runtime startup path calls this. Opening another store handle
    /// must not declare another process's live operations interrupted.
    pub fn recover_tool_operations(&self) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE tool_operation_receipts SET state='outcome_unknown' WHERE state='dispatching'",
            [],
        )?;
        self.connection.execute(
            "DELETE FROM tool_operation_receipts WHERE updated_at < datetime('now','-7 days')",
            [],
        )?;
        Ok(())
    }

    /// Reserve durably before dispatch. False means the ID already exists and
    /// must never produce a second physical effect, even after a restart.
    pub fn reserve_tool_operation(
        &self,
        receipt: &ToolOperationReceipt,
    ) -> Result<bool, StoreError> {
        // Never evict a live run's deduplication records to make room. A model
        // issuing an unbounded stream of calls must start a fresh user turn.
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM tool_operation_receipts WHERE run_id=?1",
            [receipt.run_id.to_string()],
            |r| r.get(0),
        )?;
        if count >= 10000 {
            return Err(StoreError::InvalidData(
                "tool operation limit reached".into(),
            ));
        }
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO tool_operation_receipts(operation_id,run_id,agent_id,tool,state,updated_at)
             VALUES(?1,?2,?3,?4,'dispatching',datetime('now'))",
            params![receipt.operation_id.to_string(), receipt.run_id.to_string(), receipt.agent_id.0.to_string(), receipt.tool],
        )? == 1;
        Ok(inserted)
    }

    pub fn finish_tool_operation(
        &self,
        operation: uuid::Uuid,
        state: &str,
    ) -> Result<(), StoreError> {
        if !matches!(
            state,
            "completed" | "failed" | "cancelled" | "outcome_unknown"
        ) {
            return Err(StoreError::InvalidData("invalid tool outcome".into()));
        }
        self.connection.execute("UPDATE tool_operation_receipts SET state=?2,updated_at=datetime('now') WHERE operation_id=?1",
                                params![operation.to_string(), state])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interrupted_actions_are_not_replayed_and_receipts_contain_no_payload() {
        let store = DitchStore {
            connection: Connection::open_in_memory().unwrap(),
        };
        store.connection.execute_batch("CREATE TABLE community_schema_migrations(version INTEGER PRIMARY KEY,applied_at TEXT)").unwrap();
        store.initialize_tool_integrations().unwrap();
        let receipt = ToolOperationReceipt {
            operation_id: uuid::Uuid::new_v4(),
            run_id: uuid::Uuid::new_v4(),
            agent_id: ditch_core::AgentId::default(),
            tool: "desktop_act".into(),
            state: "dispatching".into(),
        };
        assert!(store.reserve_tool_operation(&receipt).unwrap());
        assert!(!store.reserve_tool_operation(&receipt).unwrap());
        store.recover_tool_operations().unwrap();
        let state: String = store
            .connection
            .query_row("SELECT state FROM tool_operation_receipts", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(state, "outcome_unknown");
        assert!(!store.reserve_tool_operation(&receipt).unwrap());
        assert!(
            store
                .finish_tool_operation(receipt.operation_id, "arbitrary observed contents")
                .is_err()
        );
    }
}
