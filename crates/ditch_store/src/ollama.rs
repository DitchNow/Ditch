use super::*;
use ditch_core::ollama::OllamaEndpoint;
use uuid::Uuid;

impl DitchStore {
    pub(crate) fn initialize_ollama(&self) -> Result<(), StoreError> {
        self.connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS ollama_endpoints (
            id TEXT PRIMARY KEY, endpoint_json TEXT NOT NULL);
            INSERT OR IGNORE INTO community_schema_migrations(version, applied_at)
            VALUES(7, datetime('now'));",
        )?;
        Ok(())
    }
    pub fn ollama_endpoints(&self) -> Result<Vec<OllamaEndpoint>, StoreError> {
        Ok(self
            .connection
            .prepare("SELECT endpoint_json FROM ollama_endpoints ORDER BY rowid")?
            .query_map([], |row| from_json(&row.get::<_, String>(0)?))?
            .collect::<Result<Vec<_>, _>>()?)
    }
    pub fn ollama_endpoint(&self, id: Uuid) -> Result<Option<OllamaEndpoint>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT endpoint_json FROM ollama_endpoints WHERE id=?1",
                [id.to_string()],
                |row| from_json(&row.get::<_, String>(0)?),
            )
            .optional()?)
    }
    pub fn insert_ollama_endpoint(&self, endpoint: &OllamaEndpoint) -> Result<(), StoreError> {
        let json =
            serde_json::to_string(endpoint).map_err(|e| StoreError::InvalidData(e.to_string()))?;
        self.connection.execute(
            "INSERT INTO ollama_endpoints(id,endpoint_json) VALUES(?1,?2)",
            params![endpoint.id.to_string(), json],
        )?;
        Ok(())
    }
    pub fn update_ollama_model_status(
        &self,
        id: Uuid,
        model: &str,
        status: ditch_core::ollama::OllamaModelStatus,
    ) -> Result<(), StoreError> {
        let mut endpoint = self
            .ollama_endpoint(id)?
            .ok_or_else(|| StoreError::InvalidData("Endpoint not found".into()))?;
        endpoint.model_status.insert(model.to_owned(), status);
        self.connection.execute(
            "UPDATE ollama_endpoints SET endpoint_json=?2 WHERE id=?1",
            params![
                id.to_string(),
                serde_json::to_string(&endpoint)
                    .map_err(|e| StoreError::InvalidData(e.to_string()))?
            ],
        )?;
        Ok(())
    }
    /// Destinations are immutable even before first use; replace by registering a new ID.
    pub fn update_ollama_status(
        &self,
        id: Uuid,
        models: Option<Vec<String>>,
        reachable: Option<bool>,
        error: Option<String>,
        disable: bool,
    ) -> Result<OllamaEndpoint, StoreError> {
        let mut endpoint = self
            .ollama_endpoint(id)?
            .ok_or_else(|| StoreError::InvalidData("Endpoint not found".into()))?;
        if let Some(models) = models {
            endpoint.models = models;
        }
        endpoint.reachable = reachable;
        endpoint.error = error;
        if disable {
            endpoint.enabled = false;
        }
        let json =
            serde_json::to_string(&endpoint).map_err(|e| StoreError::InvalidData(e.to_string()))?;
        self.connection.execute(
            "UPDATE ollama_endpoints SET endpoint_json=?2 WHERE id=?1",
            params![id.to_string(), json],
        )?;
        Ok(endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_keep_identity_and_cached_models_on_failure() {
        let root = std::env::temp_dir().join(format!("ditch-ollama-store-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let database = root.join("endpoints.sqlite");
        let store = DitchStore {
            connection: Connection::open(&database).unwrap(),
        };
        store.connection.execute_batch("CREATE TABLE community_schema_migrations(version INTEGER PRIMARY KEY, applied_at TEXT)").unwrap();
        store.initialize_ollama().unwrap();
        let endpoint = OllamaEndpoint {
            id: Uuid::new_v4(),
            display_name: "GPU".into(),
            api_root_url: "https://example.com".into(),
            auth_mode: "none".into(),
            has_credentials: false,
            allow_private_http: false,
            enabled: true,
            created_at: Utc::now(),
            model_status: Default::default(),
            models: vec!["model:tag".into()],
            reachable: Some(true),
            error: None,
        };
        store.insert_ollama_endpoint(&endpoint).unwrap();
        let mut duplicate = endpoint.clone();
        duplicate.id = Uuid::new_v4();
        store.insert_ollama_endpoint(&duplicate).unwrap();
        assert_eq!(store.ollama_endpoints().unwrap().len(), 2);
        assert!(store.insert_ollama_endpoint(&endpoint).is_err());
        let status = ditch_core::ollama::OllamaModelStatus {
            tools_declared: true,
            vision_declared: false,
            context_window: 65536,
            model_fingerprint: "model-hash".into(),
            server_version: "test".into(),
            verified_at: Utc::now(),
            developer_instructions: true,
            conversation_recall: true,
            rejects_overflow: true,
        };
        store
            .update_ollama_model_status(endpoint.id, "model:tag", status.clone())
            .unwrap();
        assert_eq!(
            store
                .ollama_endpoint(endpoint.id)
                .unwrap()
                .unwrap()
                .model_status["model:tag"],
            status
        );
        assert!(
            store
                .ollama_endpoint(duplicate.id)
                .unwrap()
                .unwrap()
                .model_status
                .is_empty()
        );
        let mut old_json = serde_json::to_value(&endpoint).unwrap();
        old_json.as_object_mut().unwrap().remove("model_status");
        assert!(
            serde_json::from_value::<OllamaEndpoint>(old_json)
                .unwrap()
                .model_status
                .is_empty()
        );
        let disabled = store
            .update_ollama_status(
                endpoint.id,
                None,
                Some(false),
                Some("Unavailable".into()),
                true,
            )
            .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.api_root_url, endpoint.api_root_url);
        assert_eq!(disabled.models, endpoint.models);
        drop(store);
        let store = DitchStore {
            connection: Connection::open(&database).unwrap(),
        };
        store.initialize_ollama().unwrap();
        assert_eq!(
            store.ollama_endpoint(endpoint.id).unwrap().unwrap(),
            disabled
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
