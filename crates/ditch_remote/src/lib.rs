//! Daemon-owned domain boundary for Ditch Remote Control v1.
//!
//! This crate intentionally contains no Cloudflare runtime dependencies and no
//! generic execution mechanism. Network adapters deliver only the exhaustive
//! [`RemoteCommandType`] set to `ditchd`, which remains execution authority.

use aes_gcm::{
    AeadCore, Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration, Utc};
use ditch_core::{AgentRun, AgentState, AttentionKind, Project};
use ditch_protocol::RuntimeAttention;
use hkdf::Hkdf;
use p256::{
    PublicKey, SecretKey,
    ecdh::diffie_hellman,
    ecdsa::{
        Signature, SigningKey, VerifyingKey,
        signature::{Signer, Verifier},
    },
    elliptic_curve::sec1::ToEncodedPoint,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

pub const REMOTE_PROTOCOL_VERSION: u16 = 1;
pub const MAX_COMMAND_AGE: Duration = Duration::minutes(5);
const KEYCHAIN_SERVICE: &str = "dev.theditch.remote-control.v1";

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("unsupported_protocol")]
    UnsupportedProtocol,
    #[error("command_expired")]
    CommandExpired,
    #[error("command_duplicate")]
    CommandDuplicate,
    #[error("action_not_allowed")]
    ActionNotAllowed,
    #[error("invalid_ciphertext")]
    InvalidCiphertext,
    #[error("invalid_key")]
    InvalidKey,
    #[error("keychain_unavailable: {0}")]
    Keychain(String),
    #[error("serialization_failed: {0}")]
    Serialization(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationClass {
    None,
    ContextConfirmation,
    DeviceOwnerAuthentication,
    DesktopOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RemoteCommandType {
    #[serde(rename = "session.start")]
    SessionStart,
    #[serde(rename = "session.prompt")]
    SessionPrompt,
    #[serde(rename = "session.stop")]
    SessionStop,
    #[serde(rename = "session.force_kill")]
    SessionForceKill,
    #[serde(rename = "approval.respond")]
    ApprovalRespond,
    #[serde(rename = "attention.execute")]
    AttentionExecute,
    #[serde(rename = "attention.acknowledge")]
    AttentionAcknowledge,
    #[serde(rename = "attention.snooze")]
    AttentionSnooze,
    #[serde(rename = "integration.apply")]
    IntegrationApply,
    #[serde(rename = "query.session_transcript")]
    QuerySessionTranscript,
    #[serde(rename = "query.approval")]
    QueryApproval,
    #[serde(rename = "query.session_review")]
    QuerySessionReview,
}

impl RemoteCommandType {
    pub fn required_confirmation(self) -> ConfirmationClass {
        match self {
            Self::SessionStart
            | Self::SessionPrompt
            | Self::QuerySessionTranscript
            | Self::QuerySessionReview
            | Self::QueryApproval => ConfirmationClass::None,
            Self::SessionStop
            | Self::AttentionExecute
            | Self::AttentionAcknowledge
            | Self::AttentionSnooze => ConfirmationClass::ContextConfirmation,
            Self::SessionForceKill | Self::ApprovalRespond | Self::IntegrationApply => {
                ConfirmationClass::DeviceOwnerAuthentication
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EncryptedPayload {
    pub key_version: u32,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteCommand {
    pub protocol_version: u16,
    pub command_id: Uuid,
    pub idempotency_key: String,
    pub owner_id: Uuid,
    pub machine_id: Uuid,
    pub device_id: Uuid,
    #[serde(rename = "type")]
    pub command_type: RemoteCommandType,
    pub created_at: i64,
    pub expires_at: i64,
    pub confirmation_class: ConfirmationClass,
    pub payload: EncryptedPayload,
}

impl RemoteCommand {
    pub fn validate(&self, now: DateTime<Utc>) -> Result<ConfirmationClass, RemoteError> {
        if self.protocol_version != REMOTE_PROTOCOL_VERSION {
            return Err(RemoteError::UnsupportedProtocol);
        }
        let now = now.timestamp_millis();
        if self.expires_at <= now
            || self.created_at > now + MAX_COMMAND_AGE.num_milliseconds()
            || self.expires_at - self.created_at > MAX_COMMAND_AGE.num_milliseconds()
        {
            return Err(RemoteError::CommandExpired);
        }
        if self.idempotency_key.len() < 16 || self.idempotency_key.len() > 128 {
            return Err(RemoteError::ActionNotAllowed);
        }
        Ok(self
            .confirmation_class
            .max(self.command_type.required_confirmation()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionStartPayload {
    pub project_id: Uuid,
    pub task_title: String,
    pub initial_prompt: String,
    pub provider: Option<String>,
    pub permission_profile: Option<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionPromptPayload {
    pub session_id: Uuid,
    pub text: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionTargetPayload {
    pub session_id: Uuid,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TranscriptQueryPayload {
    pub session_id: Uuid,
    pub before_sequence: Option<u64>,
    pub limit: u16,
    pub query_id: Uuid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Aad {
    pub command_or_event_type: String,
    pub created_at: i64,
    pub device_id: Uuid,
    pub expires_at: i64,
    pub machine_id: Uuid,
    pub message_id: Uuid,
    pub owner_id: Uuid,
    pub protocol_version: u16,
}

impl Aad {
    pub fn canonical_json(&self) -> Result<Vec<u8>, RemoteError> {
        serde_json::to_vec(self).map_err(|error| RemoteError::Serialization(error.to_string()))
    }
}

#[derive(Clone)]
pub struct MachineIdentity {
    pub machine_id: Uuid,
    signing: SigningKey,
    agreement: SecretKey,
}

impl MachineIdentity {
    pub fn generate(machine_id: Uuid) -> Self {
        Self {
            machine_id,
            signing: SigningKey::random(&mut OsRng),
            agreement: SecretKey::random(&mut OsRng),
        }
    }
    pub fn from_private_scalars(
        machine_id: Uuid,
        signing: &[u8],
        agreement: &[u8],
    ) -> Result<Self, RemoteError> {
        Ok(Self {
            machine_id,
            signing: SigningKey::from_slice(signing).map_err(|_| RemoteError::InvalidKey)?,
            agreement: SecretKey::from_slice(agreement).map_err(|_| RemoteError::InvalidKey)?,
        })
    }
    pub fn signing_public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(
            VerifyingKey::from(&self.signing)
                .to_encoded_point(false)
                .as_bytes(),
        )
    }
    pub fn agreement_public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(
            self.agreement
                .public_key()
                .to_encoded_point(false)
                .as_bytes(),
        )
    }
    pub fn sign(&self, value: &[u8]) -> String {
        let signature: Signature = self.signing.sign(value);
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    }
    pub fn verify(public_key: &str, value: &[u8], signature: &str) -> bool {
        let Ok(public) = URL_SAFE_NO_PAD.decode(public_key) else {
            return false;
        };
        let Ok(verifying) = VerifyingKey::from_sec1_bytes(&public) else {
            return false;
        };
        let Ok(bytes) = URL_SAFE_NO_PAD.decode(signature) else {
            return false;
        };
        let Ok(signature) = Signature::from_slice(&bytes) else {
            return false;
        };
        verifying.verify(value, &signature).is_ok()
    }
    pub fn private_blob(&self) -> Result<Vec<u8>, RemoteError> {
        let signing = self.signing.to_bytes();
        let agreement = self
            .agreement
            .to_pkcs8_der()
            .map_err(|_| RemoteError::InvalidKey)?;
        serde_json::to_vec(&(
            URL_SAFE_NO_PAD.encode(signing),
            URL_SAFE_NO_PAD.encode(agreement.as_bytes()),
        ))
        .map_err(|error| RemoteError::Serialization(error.to_string()))
    }
    pub fn from_private_blob(machine_id: Uuid, bytes: &[u8]) -> Result<Self, RemoteError> {
        let (signing, agreement): (String, String) = serde_json::from_slice(bytes)
            .map_err(|error| RemoteError::Serialization(error.to_string()))?;
        let signing_bytes = URL_SAFE_NO_PAD
            .decode(signing)
            .map_err(|_| RemoteError::InvalidKey)?;
        let agreement_bytes = URL_SAFE_NO_PAD
            .decode(agreement)
            .map_err(|_| RemoteError::InvalidKey)?;
        Ok(Self {
            machine_id,
            signing: SigningKey::from_slice(&signing_bytes).map_err(|_| RemoteError::InvalidKey)?,
            agreement: SecretKey::from_pkcs8_der(&agreement_bytes)
                .map_err(|_| RemoteError::InvalidKey)?,
        })
    }
    pub fn derive_pairwise(
        &self,
        device_public_key: &str,
        context: &PairwiseContext,
    ) -> Result<[u8; 32], RemoteError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(device_public_key)
            .map_err(|_| RemoteError::InvalidKey)?;
        let public = PublicKey::from_sec1_bytes(&bytes).map_err(|_| RemoteError::InvalidKey)?;
        let shared = diffie_hellman(self.agreement.to_nonzero_scalar(), public.as_affine());
        let salt = Sha256::digest(b"Ditch Remote v1 pairwise salt");
        let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared.raw_secret_bytes());
        let mut key = [0_u8; 32];
        hkdf.expand(context.info().as_bytes(), &mut key)
            .map_err(|_| RemoteError::InvalidKey)?;
        Ok(key)
    }
}

/// Validates the canonical uncompressed SEC1 encoding used for both Remote
/// Protocol v1 P-256 identity and agreement public keys.
pub fn validate_p256_public_key(value: &str) -> Result<(), RemoteError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| RemoteError::InvalidKey)?;
    PublicKey::from_sec1_bytes(&bytes).map_err(|_| RemoteError::InvalidKey)?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairwiseContext {
    pub owner_id: Uuid,
    pub machine_id: Uuid,
    pub device_id: Uuid,
    pub key_version: u32,
}
impl PairwiseContext {
    pub fn info(&self) -> String {
        format!(
            "DITCH-REMOTE-PAIRWISE\n{}\n{}\n{}\n{}\n{}",
            REMOTE_PROTOCOL_VERSION,
            self.owner_id,
            self.machine_id,
            self.device_id,
            self.key_version
        )
    }
}

pub fn encrypt(
    key: &[u8; 32],
    aad: &Aad,
    plaintext: &[u8],
) -> Result<EncryptedPayload, RemoteError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| RemoteError::InvalidKey)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &aad.canonical_json()?,
            },
        )
        .map_err(|_| RemoteError::InvalidCiphertext)?;
    Ok(EncryptedPayload {
        key_version: 1,
        nonce: URL_SAFE_NO_PAD.encode(nonce),
        ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
    })
}

pub fn decrypt(
    key: &[u8; 32],
    aad: &Aad,
    payload: &EncryptedPayload,
) -> Result<Vec<u8>, RemoteError> {
    let nonce = URL_SAFE_NO_PAD
        .decode(&payload.nonce)
        .map_err(|_| RemoteError::InvalidCiphertext)?;
    let ciphertext = URL_SAFE_NO_PAD
        .decode(&payload.ciphertext)
        .map_err(|_| RemoteError::InvalidCiphertext)?;
    if nonce.len() != 12 {
        return Err(RemoteError::InvalidCiphertext);
    }
    Aes256Gcm::new_from_slice(key)
        .map_err(|_| RemoteError::InvalidKey)?
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: &aad.canonical_json()?,
            },
        )
        .map_err(|_| RemoteError::InvalidCiphertext)
}

pub fn canonical_request(
    method: &str,
    path_and_query: &str,
    body: &[u8],
    timestamp_ms: i64,
    nonce: &str,
    principal_id: Uuid,
) -> String {
    format!(
        "DITCH1\n{}\n{}\n{:x}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path_and_query,
        Sha256::digest(body),
        timestamp_ms,
        nonce,
        principal_id
    )
}

pub trait IdentityStore {
    fn load(&self, machine_id: Uuid) -> Result<Option<MachineIdentity>, RemoteError>;
    fn save(&self, identity: &MachineIdentity) -> Result<(), RemoteError>;
    fn delete(&self, machine_id: Uuid) -> Result<(), RemoteError>;
}

/// Identity storage for headless hosts without a reliable OS keyring. The
/// caller supplies `~/.ditch/identity`; private material never leaves it.
pub struct FileIdentityStore {
    directory: PathBuf,
}

impl FileIdentityStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    fn path(&self, machine_id: Uuid) -> PathBuf {
        self.directory.join(format!("{machine_id}.identity"))
    }
}

impl IdentityStore for FileIdentityStore {
    fn load(&self, machine_id: Uuid) -> Result<Option<MachineIdentity>, RemoteError> {
        match fs::read(self.path(machine_id)) {
            Ok(bytes) => MachineIdentity::from_private_blob(machine_id, &bytes).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RemoteError::Keychain(error.to_string())),
        }
    }

    fn save(&self, identity: &MachineIdentity) -> Result<(), RemoteError> {
        fs::create_dir_all(&self.directory)
            .map_err(|error| RemoteError::Keychain(error.to_string()))?;
        set_private_permissions(&self.directory, true)?;
        let path = self.path(identity.machine_id);
        let temporary = self.directory.join(format!(".{}.tmp", Uuid::new_v4()));
        fs::write(&temporary, identity.private_blob()?)
            .map_err(|error| RemoteError::Keychain(error.to_string()))?;
        set_private_permissions(&temporary, false)?;
        fs::rename(&temporary, &path).map_err(|error| RemoteError::Keychain(error.to_string()))?;
        Ok(())
    }

    fn delete(&self, machine_id: Uuid) -> Result<(), RemoteError> {
        match fs::remove_file(self.path(machine_id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(RemoteError::Keychain(error.to_string())),
        }
    }
}

fn set_private_permissions(path: &Path, directory: bool) -> Result<(), RemoteError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )
        .map_err(|error| RemoteError::Keychain(error.to_string()))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub struct KeychainIdentityStore;
#[cfg(target_os = "macos")]
impl IdentityStore for KeychainIdentityStore {
    fn load(&self, machine_id: Uuid) -> Result<Option<MachineIdentity>, RemoteError> {
        match security_framework::passwords::get_generic_password(
            KEYCHAIN_SERVICE,
            &machine_id.to_string(),
        ) {
            Ok(bytes) => MachineIdentity::from_private_blob(machine_id, &bytes).map(Some),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(error) => Err(RemoteError::Keychain(error.to_string())),
        }
    }
    fn save(&self, identity: &MachineIdentity) -> Result<(), RemoteError> {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            &identity.machine_id.to_string(),
            &identity.private_blob()?,
        )
        .map_err(|error| RemoteError::Keychain(error.to_string()))
    }
    fn delete(&self, machine_id: Uuid) -> Result<(), RemoteError> {
        security_framework::passwords::delete_generic_password(
            KEYCHAIN_SERVICE,
            &machine_id.to_string(),
        )
        .map_err(|error| RemoteError::Keychain(error.to_string()))
    }
}

#[derive(Default)]
pub struct MemoryIdentityStore(std::sync::Mutex<HashMap<Uuid, Vec<u8>>>);
impl IdentityStore for MemoryIdentityStore {
    fn load(&self, machine_id: Uuid) -> Result<Option<MachineIdentity>, RemoteError> {
        self.0
            .lock()
            .unwrap()
            .get(&machine_id)
            .map(|bytes| MachineIdentity::from_private_blob(machine_id, bytes))
            .transpose()
    }
    fn save(&self, identity: &MachineIdentity) -> Result<(), RemoteError> {
        self.0
            .lock()
            .unwrap()
            .insert(identity.machine_id, identity.private_blob()?);
        Ok(())
    }
    fn delete(&self, machine_id: Uuid) -> Result<(), RemoteError> {
        self.0.lock().unwrap().remove(&machine_id);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_status: Option<String>,
    pub project_id: Uuid,
    pub name: String,
    pub status: String,
    pub running_count: u32,
    pub waiting_count: u32,
    pub ready_count: u32,
    pub failed_count: u32,
    pub attention_count: u32,
    /// Remote Protocol v1 Unix epoch milliseconds.
    pub last_activity_at: i64,
    pub projection_version: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionProjection {
    pub session_id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub provider_name: String,
    pub status: String,
    pub semantic_phase: Option<String>,
    pub needs_user: bool,
    pub attention_count: u32,
    pub changed_file_count: Option<u32>,
    pub validation_summary: Option<String>,
    pub integration_readiness: Option<String>,
    /// Remote Protocol v1 Unix epoch milliseconds.
    pub last_activity_at: i64,
    pub projection_version: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttentionProjection {
    pub attention_id: Uuid,
    pub project_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
    #[serde(rename = "type")]
    pub kind: String,
    pub severity: String,
    pub summary: String,
    pub state: String,
    pub remote_actions: Vec<RemoteAction>,
    pub projection_version: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RemoteAction {
    pub action_id: Uuid,
    #[serde(rename = "type")]
    pub action_type: TrustedActionType,
    pub label: String,
    pub confirmation_class: ConfirmationClass,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustedActionType {
    Acknowledge,
    Snooze,
    RespondApproval,
    Deny,
    Answer,
    ApplyIntegration,
}

pub struct RemoteProjector;
impl RemoteProjector {
    pub fn project(
        project: &Project,
        agents: &[AgentRun],
        attention: &[RuntimeAttention],
        version: u64,
    ) -> ProjectProjection {
        let relevant: Vec<_> = agents
            .iter()
            .filter(|run| run.project_id == project.id)
            .collect();
        let running = relevant
            .iter()
            .filter(|run| {
                matches!(
                    run.state,
                    AgentState::Starting | AgentState::Working | AgentState::Stopping
                )
            })
            .count() as u32;
        let waiting = relevant
            .iter()
            .filter(|run| {
                matches!(
                    run.state,
                    AgentState::AwaitingApproval | AgentState::Blocked
                )
            })
            .count() as u32;
        let failed = relevant
            .iter()
            .filter(|run| run.state == AgentState::Failed)
            .count() as u32;
        let last = relevant
            .iter()
            .map(|run| run.updated_at)
            .max()
            .unwrap_or(project.created_at);
        ProjectProjection {
            target_host: None,
            target_status: None,
            project_id: project.id.0,
            name: project.name.clone(),
            status: if running > 0 {
                "running"
            } else if failed > 0 {
                "failed"
            } else {
                "idle"
            }
            .into(),
            running_count: running,
            waiting_count: waiting,
            ready_count: 0,
            failed_count: failed,
            attention_count: attention
                .iter()
                .filter(|item| item.project_id == Some(project.id))
                .count() as u32,
            last_activity_at: last.timestamp_millis(),
            projection_version: version,
        }
    }
    pub fn session(
        run: &AgentRun,
        attention: &[RuntimeAttention],
        version: u64,
    ) -> SessionProjection {
        SessionProjection {
            session_id: run.id.0,
            project_id: run.project_id.0,
            title: run
                .user_title
                .clone()
                .or_else(|| run.codex_title.clone())
                .unwrap_or_else(|| "Agent session".into()),
            provider_name: format!("{:?}", run.provider),
            status: format!("{:?}", run.state).to_ascii_lowercase(),
            semantic_phase: run
                .last_visible_action
                .clone()
                .map(|value| truncate(&value, 120)),
            needs_user: matches!(
                run.state,
                AgentState::AwaitingApproval | AgentState::Blocked
            ),
            attention_count: attention
                .iter()
                .filter(|item| item.agent_id == Some(run.id))
                .count() as u32,
            changed_file_count: None,
            validation_summary: None,
            integration_readiness: None,
            last_activity_at: run.updated_at.timestamp_millis(),
            projection_version: version,
        }
    }
    pub fn attention(item: &RuntimeAttention, version: u64) -> AttentionProjection {
        AttentionProjection {
            attention_id: item.id,
            project_id: item.project_id.map(|id| id.0),
            session_id: item.agent_id.map(|id| id.0),
            kind: format!("{:?}", item.kind).to_ascii_lowercase(),
            severity: if item.kind == AttentionKind::Failed {
                "error"
            } else {
                "info"
            }
            .into(),
            summary: truncate(&item.title, 160),
            state: "open".into(),
            remote_actions: {
                let mut actions = vec![RemoteAction {
                    // Action descriptors must be stable trusted domain data. A fresh
                    // random ID on every reconciliation would make harmless retries
                    // appear to be distinct executable actions to a phone.
                    action_id: item.id,
                    action_type: if item.kind == AttentionKind::ApprovalRequired {
                        TrustedActionType::RespondApproval
                    } else {
                        TrustedActionType::Acknowledge
                    },
                    label: if item.kind == AttentionKind::ApprovalRequired {
                        "Review approval"
                    } else {
                        "Acknowledge"
                    }
                    .into(),
                    confirmation_class: if item.kind == AttentionKind::ApprovalRequired {
                        ConfirmationClass::DeviceOwnerAuthentication
                    } else {
                        ConfirmationClass::ContextConfirmation
                    },
                }];
                if item.kind == AttentionKind::ApprovalRequired {
                    actions.push(RemoteAction {
                        action_id: item.id,
                        action_type: TrustedActionType::Deny,
                        label: "Deny".into(),
                        confirmation_class: ConfirmationClass::DeviceOwnerAuthentication,
                    });
                }
                actions
            },
            projection_version: version,
        }
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn file_identity_store_uses_private_atomic_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ditch-identity-{}", Uuid::new_v4()));
        let store = FileIdentityStore::new(&root);
        let identity = MachineIdentity::generate(Uuid::new_v4());
        store.save(&identity).unwrap();
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = root.join(format!("{}.identity", identity.machine_id));
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            store.load(identity.machine_id).unwrap().unwrap().machine_id,
            identity.machine_id
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn canonical_request_matches_contract_fixture() {
        let body = br#"{"machine_id":"22222222-2222-4222-8222-222222222222"}"#;
        let principal = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        assert_eq!(
            canonical_request(
                "post",
                "/v1/auth/socket-ticket",
                body,
                1_787_596_800_000,
                "AAECAwQFBgcICQoLDA0ODw",
                principal
            ),
            "DITCH1\nPOST\n/v1/auth/socket-ticket\nacd1e31f7f0cfe00afdf5ac287050fec8f5af7f4af3afe5512a92607e77f1a11\n1787596800000\nAAECAwQFBgcICQoLDA0ODw\n11111111-1111-4111-8111-111111111111"
        );
    }

    #[test]
    fn pairwise_keys_and_ciphertext_interoperate_and_bind_aad() {
        let machine = MachineIdentity::generate(Uuid::new_v4());
        let device = MachineIdentity::generate(Uuid::new_v4());
        let context = PairwiseContext {
            owner_id: Uuid::new_v4(),
            machine_id: machine.machine_id,
            device_id: device.machine_id,
            key_version: 1,
        };
        let machine_key = machine
            .derive_pairwise(&device.agreement_public_key(), &context)
            .unwrap();
        let device_key = device
            .derive_pairwise(&machine.agreement_public_key(), &context)
            .unwrap();
        assert_eq!(machine_key, device_key);
        let aad = Aad {
            command_or_event_type: "session.prompt".into(),
            created_at: 1,
            device_id: context.device_id,
            expires_at: 2,
            machine_id: context.machine_id,
            message_id: Uuid::new_v4(),
            owner_id: context.owner_id,
            protocol_version: 1,
        };
        let encrypted = encrypt(&machine_key, &aad, b"private prompt").unwrap();
        assert_eq!(
            decrypt(&device_key, &aad, &encrypted).unwrap(),
            b"private prompt"
        );
        let mut wrong = aad.clone();
        wrong.expires_at = 3;
        assert!(matches!(
            decrypt(&device_key, &wrong, &encrypted),
            Err(RemoteError::InvalidCiphertext)
        ));
    }

    #[test]
    fn unknown_command_is_rejected_by_serde() {
        let input = r#"{"protocol_version":1,"command_id":"44444444-4444-4444-8444-444444444444","idempotency_key":"fixture-idempotency-0001","owner_id":"55555555-5555-4555-8555-555555555555","machine_id":"22222222-2222-4222-8222-222222222222","device_id":"11111111-1111-4111-8111-111111111111","type":"shell.execute","created_at":1787596800000,"expires_at":1787596860000,"confirmation_class":"none","payload":{"key_version":1,"nonce":"AAECAwQFBgcICQoL","ciphertext":"abc"}}"#;
        assert!(serde_json::from_str::<RemoteCommand>(input).is_err());
    }

    #[test]
    fn project_projection_cannot_serialize_root_or_prompt() {
        let project = Project::new("Safe name", "/secret/repository");
        let projection = RemoteProjector::project(&project, &[], &[], 1);
        let json = serde_json::to_value(&projection).unwrap();
        let encoded = serde_json::to_string(&json).unwrap();
        assert!(!encoded.contains("/secret/repository"));
        assert!(!encoded.contains("current_prompt"));
        assert_eq!(
            json["last_activity_at"].as_i64(),
            Some(project.created_at.timestamp_millis())
        );
        assert!(json["last_activity_at"].is_number());
    }

    #[test]
    fn projection_timestamps_are_always_json_epoch_milliseconds() {
        let session = SessionProjection {
            session_id: Uuid::new_v4(),
            project_id: Uuid::new_v4(),
            title: "Fixture session".into(),
            provider_name: "Codex".into(),
            status: "working".into(),
            semantic_phase: None,
            needs_user: false,
            attention_count: 0,
            changed_file_count: None,
            validation_summary: None,
            integration_readiness: None,
            last_activity_at: -1,
            projection_version: 1,
        };
        let json = serde_json::to_value(session).unwrap();
        assert_eq!(json["last_activity_at"], serde_json::json!(-1));
        assert!(!json["last_activity_at"].is_string());
    }

    #[test]
    fn public_key_validation_rejects_invalid_curve_points() {
        let identity = MachineIdentity::generate(Uuid::new_v4());
        assert!(validate_p256_public_key(&identity.signing_public_key()).is_ok());
        assert!(validate_p256_public_key(&identity.agreement_public_key()).is_ok());
        assert!(validate_p256_public_key("not-base64url").is_err());
        assert!(validate_p256_public_key(
            "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        )
        .is_err());
    }
}
