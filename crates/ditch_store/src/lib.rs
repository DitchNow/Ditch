//! Proprietary persistence extensions for the shared Community store.
//!
//! This crate owns only Commercial tables and records. Every operation runs
//! through `ditch_store::DitchStore`, so there is one SQLite connection,
//! migration authority, and transaction owner in the single `ditchd`.

use chrono::{DateTime, Utc};
use ditch_remote::{AttentionProjection, ProjectProjection, SessionProjection};
use ditch_store::{DitchStore, StoreError};
use rusqlite::{Transaction, params, types::Type};
use uuid::Uuid;

const NAMESPACE: &str = "commercial_remote_control";

#[derive(Clone, Debug)]
pub struct RemoteMachineRecord {
    pub machine_id: Uuid,
    pub owner_id: Option<Uuid>,
    pub name: String,
    pub signing_public_key: String,
    pub agreement_public_key: String,
    pub key_version: u32,
    pub enabled: bool,
    pub projection_epoch: Uuid,
    pub projection_sequence: u64,
}

#[derive(Clone, Debug)]
pub struct RemoteDeviceRecord {
    pub device_id: Uuid,
    pub name: String,
    pub signing_public_key: String,
    pub agreement_public_key: String,
    pub key_version: u32,
    pub state: String,
    pub last_seen_at: Option<DateTime<Utc>>,
}

pub trait CommercialStoreExt {
    fn initialize_commercial_schema(&mut self) -> Result<(), StoreError>;
    fn remote_machine(&self) -> Result<Option<RemoteMachineRecord>, StoreError>;
    fn upsert_remote_machine(&mut self, machine: &RemoteMachineRecord) -> Result<(), StoreError>;
    fn set_remote_enabled(&mut self, enabled: bool) -> Result<(), StoreError>;
    fn upsert_remote_device(&mut self, device: &RemoteDeviceRecord) -> Result<(), StoreError>;
    fn remote_devices(&self) -> Result<Vec<RemoteDeviceRecord>, StoreError>;
    fn delete_remote_device(&mut self, device_id: Uuid) -> Result<(), StoreError>;
    fn replace_remote_devices(&mut self, devices: &[RemoteDeviceRecord]) -> Result<(), StoreError>;
    fn begin_remote_command(
        &mut self,
        command_id: Uuid,
        idempotency_key: &str,
        command_type: &str,
        device_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, StoreError>;
    fn finish_remote_command(
        &mut self,
        command_id: Uuid,
        status: &str,
        error_code: Option<&str>,
        result_json: Option<&str>,
    ) -> Result<(), StoreError>;
    fn replace_remote_projection_cache(
        &mut self,
        epoch: Uuid,
        sequence: u64,
        projects: &[ProjectProjection],
        sessions: &[SessionProjection],
        attention: &[AttentionProjection],
    ) -> Result<(), StoreError>;
}

impl CommercialStoreExt for DitchStore {
    fn initialize_commercial_schema(&mut self) -> Result<(), StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            connection.execute_batch(COMMERCIAL_SCHEMA)?;
            connection.execute(
                "INSERT OR IGNORE INTO commercial_schema_migrations(capability,version,applied_at) VALUES(?1,?2,?3)",
                params![NAMESPACE, 1_i64, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn remote_machine(&self) -> Result<Option<RemoteMachineRecord>, StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            match connection.query_row(
                "SELECT machine_id,owner_id,name,signing_public_key,agreement_public_key,key_version,enabled,projection_epoch,projection_sequence FROM remote_machine WHERE singleton=1",
                [],
                |row| {
                    Ok(RemoteMachineRecord {
                        machine_id: parse_uuid(row.get(0)?)?,
                        owner_id: row.get::<_, Option<String>>(1)?.map(parse_uuid).transpose()?,
                        name: row.get(2)?,
                        signing_public_key: row.get(3)?,
                        agreement_public_key: row.get(4)?,
                        key_version: row.get(5)?,
                        enabled: row.get::<_, i64>(6)? != 0,
                        projection_epoch: parse_uuid(row.get(7)?)?,
                        projection_sequence: u64::try_from(row.get::<_, i64>(8)?).unwrap_or_default(),
                    })
                },
            ) {
                Ok(value) => Ok(Some(value)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(error) => Err(error.into()),
            }
        })
    }

    fn upsert_remote_machine(&mut self, machine: &RemoteMachineRecord) -> Result<(), StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            connection.execute(
                "INSERT INTO remote_machine(singleton,machine_id,owner_id,name,signing_public_key,agreement_public_key,key_version,enabled,projection_epoch,projection_sequence,updated_at) VALUES(1,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(singleton) DO UPDATE SET machine_id=excluded.machine_id,owner_id=excluded.owner_id,name=excluded.name,signing_public_key=excluded.signing_public_key,agreement_public_key=excluded.agreement_public_key,key_version=excluded.key_version,enabled=excluded.enabled,projection_epoch=excluded.projection_epoch,projection_sequence=excluded.projection_sequence,updated_at=excluded.updated_at",
                params![machine.machine_id.to_string(), machine.owner_id.map(|value| value.to_string()), machine.name, machine.signing_public_key, machine.agreement_public_key, machine.key_version, i64::from(machine.enabled), machine.projection_epoch.to_string(), i64::try_from(machine.projection_sequence).unwrap_or(i64::MAX), Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn set_remote_enabled(&mut self, enabled: bool) -> Result<(), StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            connection.execute(
                "UPDATE remote_machine SET enabled=?,updated_at=? WHERE singleton=1",
                params![i64::from(enabled), Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn upsert_remote_device(&mut self, device: &RemoteDeviceRecord) -> Result<(), StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            insert_device(connection, device)?;
            Ok(())
        })
    }

    fn remote_devices(&self) -> Result<Vec<RemoteDeviceRecord>, StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            let mut statement = connection.prepare(
                "SELECT device_id,name,signing_public_key,agreement_public_key,key_version,state,last_seen_at FROM remote_devices ORDER BY name",
            )?;
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
        })
    }

    fn delete_remote_device(&mut self, device_id: Uuid) -> Result<(), StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            connection.execute(
                "DELETE FROM remote_devices WHERE device_id=?",
                params![device_id.to_string()],
            )?;
            Ok(())
        })
    }

    fn replace_remote_devices(&mut self, devices: &[RemoteDeviceRecord]) -> Result<(), StoreError> {
        self.with_extension_transaction(NAMESPACE, |transaction| {
            transaction.execute("DELETE FROM remote_devices", [])?;
            for device in devices {
                insert_device(transaction, device)?;
            }
            Ok(())
        })
    }

    fn begin_remote_command(
        &mut self,
        command_id: Uuid,
        idempotency_key: &str,
        command_type: &str,
        device_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            let changed = connection.execute(
                "INSERT OR IGNORE INTO remote_command_outcomes(command_id,idempotency_key,command_type,device_id,status,received_at,expires_at) VALUES(?,?,?,?,'received',?,?)",
                params![command_id.to_string(), idempotency_key, command_type, device_id.to_string(), Utc::now().to_rfc3339(), expires_at.to_rfc3339()],
            )?;
            Ok(changed == 1)
        })
    }

    fn finish_remote_command(
        &mut self,
        command_id: Uuid,
        status: &str,
        error_code: Option<&str>,
        result_json: Option<&str>,
    ) -> Result<(), StoreError> {
        self.with_extension_connection(NAMESPACE, |connection| {
            connection.execute(
                "UPDATE remote_command_outcomes SET status=?,error_code=?,result_json=?,finished_at=? WHERE command_id=?",
                params![status, error_code, result_json, Utc::now().to_rfc3339(), command_id.to_string()],
            )?;
            Ok(())
        })
    }

    fn replace_remote_projection_cache(
        &mut self,
        epoch: Uuid,
        sequence: u64,
        projects: &[ProjectProjection],
        sessions: &[SessionProjection],
        attention: &[AttentionProjection],
    ) -> Result<(), StoreError> {
        self.with_extension_transaction(NAMESPACE, |transaction| {
            transaction.execute("DELETE FROM remote_projection_cache", [])?;
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
                transaction.execute(
                    "INSERT INTO remote_projection_cache(kind,record_id,projection_json) VALUES(?,?,?)",
                    params![kind, id.to_string(), value.map_err(|error| StoreError::InvalidData(error.to_string()))?],
                )?;
            }
            transaction.execute(
                "UPDATE remote_machine SET projection_epoch=?,projection_sequence=?,updated_at=? WHERE singleton=1",
                params![epoch.to_string(), i64::try_from(sequence).unwrap_or(i64::MAX), Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }
}

trait DeviceConnection {
    fn insert_device(&self, device: &RemoteDeviceRecord) -> rusqlite::Result<usize>;
}

impl DeviceConnection for rusqlite::Connection {
    fn insert_device(&self, device: &RemoteDeviceRecord) -> rusqlite::Result<usize> {
        self.execute(
            "INSERT INTO remote_devices(device_id,name,signing_public_key,agreement_public_key,key_version,state,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?)
             ON CONFLICT(device_id) DO UPDATE SET name=excluded.name,signing_public_key=excluded.signing_public_key,agreement_public_key=excluded.agreement_public_key,key_version=excluded.key_version,state=excluded.state,last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at",
            params![device.device_id.to_string(), device.name, device.signing_public_key, device.agreement_public_key, device.key_version, device.state, device.last_seen_at.map(|value| value.to_rfc3339()), Utc::now().to_rfc3339()],
        )
    }
}

impl DeviceConnection for Transaction<'_> {
    fn insert_device(&self, device: &RemoteDeviceRecord) -> rusqlite::Result<usize> {
        insert_device_sql(self, device)
    }
}

fn insert_device(
    connection: &impl DeviceConnection,
    device: &RemoteDeviceRecord,
) -> rusqlite::Result<usize> {
    connection.insert_device(device)
}

fn insert_device_sql(
    connection: &impl std::ops::Deref<Target = rusqlite::Connection>,
    device: &RemoteDeviceRecord,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO remote_devices(device_id,name,signing_public_key,agreement_public_key,key_version,state,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?)
         ON CONFLICT(device_id) DO UPDATE SET name=excluded.name,signing_public_key=excluded.signing_public_key,agreement_public_key=excluded.agreement_public_key,key_version=excluded.key_version,state=excluded.state,last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at",
        params![device.device_id.to_string(), device.name, device.signing_public_key, device.agreement_public_key, device.key_version, device.state, device.last_seen_at.map(|value| value.to_rfc3339()), Utc::now().to_rfc3339()],
    )
}

fn parse_uuid(value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value)
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error)))
}

fn parse_time(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| rusqlite::Error::FromSqlConversionFailure(0, Type::Text, Box::new(error)))
}

pub const COMMERCIAL_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS commercial_schema_migrations (
  capability TEXT NOT NULL,
  version INTEGER NOT NULL,
  applied_at TEXT NOT NULL,
  PRIMARY KEY(capability,version)
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
CREATE INDEX IF NOT EXISTS remote_command_retention
  ON remote_command_outcomes(received_at);

CREATE TABLE IF NOT EXISTS remote_projection_cache (
  kind TEXT NOT NULL CHECK(kind IN ('project','session','attention')),
  record_id TEXT NOT NULL,
  projection_json TEXT NOT NULL,
  PRIMARY KEY(kind,record_id)
);
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use ditch_core::{AppPaths, Project};
    use ditch_store::ensure_app_dirs;
    use std::fs;

    #[test]
    fn community_and_commercial_reopen_the_same_database_without_state_loss() {
        let root = std::env::temp_dir().join(format!("ditch-edition-store-{}", Uuid::new_v4()));
        let paths = AppPaths {
            data_dir: root.clone(),
            database_path: root.join("ditch.sqlite3"),
            socket_path: root.join("ditchd.sock"),
            logs_dir: root.join("logs"),
            scrollback_dir: root.join("scrollback"),
        };
        ensure_app_dirs(&paths).unwrap();
        let project = Project::new("Existing Community project", root.join("project"));
        let machine = RemoteMachineRecord {
            machine_id: Uuid::new_v4(),
            owner_id: Some(Uuid::new_v4()),
            name: "Existing Mac".to_owned(),
            signing_public_key: "signing".to_owned(),
            agreement_public_key: "agreement".to_owned(),
            key_version: 1,
            enabled: true,
            projection_epoch: Uuid::new_v4(),
            projection_sequence: 7,
        };

        {
            let mut commercial = DitchStore::open(&paths).unwrap();
            commercial.initialize_commercial_schema().unwrap();
            commercial.upsert_project(&project).unwrap();
            commercial.upsert_remote_machine(&machine).unwrap();
        }
        {
            let community = DitchStore::open(&paths).unwrap();
            let durable = community.load().unwrap();
            assert!(durable.projects.iter().any(|value| value.id == project.id));
        }
        {
            let mut commercial = DitchStore::open(&paths).unwrap();
            commercial.initialize_commercial_schema().unwrap();
            let restored = commercial.remote_machine().unwrap().unwrap();
            assert_eq!(restored.machine_id, machine.machine_id);
            assert_eq!(restored.projection_sequence, 7);
        }
        let _ = fs::remove_dir_all(root);
    }
}
