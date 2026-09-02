//! Community-safe installation and host identity.
//!
//! This crate deliberately contains no mobile protocol, pairing, agreement-key
//! exchange, Relay transport, projection, or entitlement implementation.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::SecretKey;
use p256::ecdsa::{
    Signature, SigningKey, VerifyingKey,
    signature::{Signer, Verifier},
};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const IDENTITY_VERSION: u16 = 1;
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "ai.theditch.installation-identity.v1";
#[cfg(target_os = "macos")]
const KEYCHAIN_ACCOUNT: &str = "installation";

#[derive(Debug, Error)]
pub enum IdentityError {
    #[error("identity filesystem error: {0}")]
    Io(#[from] io::Error),
    #[error("identity data is invalid")]
    InvalidData,
    #[error("identity serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("identity Keychain operation failed: {0}")]
    Keychain(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallationIdentitySummary {
    pub installation_id: Uuid,
    pub signing_public_key: String,
    pub key_version: u16,
}

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    version: u16,
    installation_id: Uuid,
    signing_private_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agreement_private_key: Option<String>,
}

pub struct InstallationIdentity {
    installation_id: Uuid,
    signing: SigningKey,
    agreement: SecretKey,
}

impl InstallationIdentity {
    pub fn generate() -> Self {
        Self {
            installation_id: Uuid::new_v4(),
            signing: SigningKey::random(&mut OsRng),
            agreement: SecretKey::random(&mut OsRng),
        }
    }

    pub fn installation_id(&self) -> Uuid {
        self.installation_id
    }

    pub fn summary(&self) -> InstallationIdentitySummary {
        InstallationIdentitySummary {
            installation_id: self.installation_id,
            signing_public_key: URL_SAFE_NO_PAD.encode(
                VerifyingKey::from(&self.signing)
                    .to_encoded_point(false)
                    .as_bytes(),
            ),
            key_version: IDENTITY_VERSION,
        }
    }

    pub fn sign(&self, value: &[u8]) -> String {
        let signature: Signature = self.signing.sign(value);
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    }

    /// Public ECDH material required by Relay's generic machine registration.
    /// Community never derives mobile keys or implements the Remote Protocol.
    pub fn agreement_public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(
            self.agreement
                .public_key()
                .to_encoded_point(false)
                .as_bytes(),
        )
    }

    /// Private material export used only by the proprietary composition to
    /// reuse this exact installation identity after an edition replacement.
    pub fn private_identity_blob(&self) -> Result<Vec<u8>, IdentityError> {
        serde_json::to_vec(&(
            URL_SAFE_NO_PAD.encode(self.signing.to_bytes()),
            URL_SAFE_NO_PAD.encode(self.agreement.to_bytes()),
        ))
        .map_err(IdentityError::Serialization)
    }

    pub fn verify(summary: &InstallationIdentitySummary, value: &[u8], signature: &str) -> bool {
        let Ok(public) = URL_SAFE_NO_PAD.decode(&summary.signing_public_key) else {
            return false;
        };
        let Ok(verifying) = VerifyingKey::from_sec1_bytes(&public) else {
            return false;
        };
        let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
            return false;
        };
        let Ok(signature) = Signature::from_slice(&signature) else {
            return false;
        };
        verifying.verify(value, &signature).is_ok()
    }

    fn stored(&self) -> StoredIdentity {
        StoredIdentity {
            version: IDENTITY_VERSION,
            installation_id: self.installation_id,
            signing_private_key: URL_SAFE_NO_PAD.encode(self.signing.to_bytes()),
            agreement_private_key: Some(URL_SAFE_NO_PAD.encode(self.agreement.to_bytes())),
        }
    }

    fn from_stored(stored: StoredIdentity) -> Result<Self, IdentityError> {
        if stored.version != IDENTITY_VERSION {
            return Err(IdentityError::InvalidData);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(stored.signing_private_key)
            .map_err(|_| IdentityError::InvalidData)?;
        let signing = SigningKey::from_slice(&bytes).map_err(|_| IdentityError::InvalidData)?;
        let agreement = stored
            .agreement_private_key
            .as_deref()
            .ok_or(IdentityError::InvalidData)
            .and_then(|value| {
                URL_SAFE_NO_PAD
                    .decode(value)
                    .map_err(|_| IdentityError::InvalidData)
            })
            .and_then(|bytes| {
                SecretKey::from_slice(&bytes).map_err(|_| IdentityError::InvalidData)
            })?;
        Ok(Self {
            installation_id: stored.installation_id,
            signing,
            agreement,
        })
    }
}

pub trait IdentityStore {
    fn load_or_create(&self) -> Result<InstallationIdentity, IdentityError>;
}

/// Protected-file store used by Community SSH runtimes. macOS app composition
/// can wrap this primitive with Keychain-backed secret storage without changing
/// the public identity contract.
pub struct FileIdentityStore {
    path: PathBuf,
}

impl FileIdentityStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl IdentityStore for FileIdentityStore {
    fn load_or_create(&self) -> Result<InstallationIdentity, IdentityError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let mut stored: StoredIdentity = serde_json::from_slice(&bytes)?;
                if stored.agreement_private_key.is_none() {
                    stored.agreement_private_key =
                        Some(URL_SAFE_NO_PAD.encode(SecretKey::random(&mut OsRng).to_bytes()));
                    write_private_file(&self.path, &serde_json::to_vec(&stored)?)?;
                }
                InstallationIdentity::from_stored(stored)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let identity = InstallationIdentity::generate();
                write_private_file(&self.path, &serde_json::to_vec(&identity.stored())?)?;
                Ok(identity)
            }
            Err(error) => Err(error.into()),
        }
    }
}

/// Keychain-backed store for the local macOS application. On first use it
/// migrates the legacy protected file atomically enough to preserve the exact
/// installation identity: Keychain is written and read back before the file
/// containing private material is removed. SSH runtimes continue to use the
/// file store under `~/.ditch` because a login Keychain is not dependable on a
/// headless host.
#[cfg(target_os = "macos")]
pub struct MacOsIdentityStore {
    legacy_path: PathBuf,
}

#[cfg(target_os = "macos")]
impl MacOsIdentityStore {
    pub fn new(legacy_path: impl Into<PathBuf>) -> Self {
        Self {
            legacy_path: legacy_path.into(),
        }
    }

    fn load_keychain(&self) -> Result<Option<InstallationIdentity>, IdentityError> {
        match security_framework::passwords::get_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
        ) {
            Ok(bytes) => serde_json::from_slice::<StoredIdentity>(&bytes)
                .map_err(IdentityError::Serialization)
                .and_then(InstallationIdentity::from_stored)
                .map(Some),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(error) => Err(IdentityError::Keychain(error.to_string())),
        }
    }

    fn save_keychain(&self, identity: &InstallationIdentity) -> Result<(), IdentityError> {
        let bytes = serde_json::to_vec(&identity.stored())?;
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            KEYCHAIN_ACCOUNT,
            &bytes,
        )
        .map_err(|error| IdentityError::Keychain(error.to_string()))?;
        let persisted = self.load_keychain()?.ok_or_else(|| {
            IdentityError::Keychain("saved identity could not be read back".to_owned())
        })?;
        if persisted.summary() != identity.summary()
            || persisted.agreement_public_key() != identity.agreement_public_key()
        {
            return Err(IdentityError::Keychain(
                "saved identity did not match the installation identity".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl IdentityStore for MacOsIdentityStore {
    fn load_or_create(&self) -> Result<InstallationIdentity, IdentityError> {
        if let Some(identity) = self.load_keychain()? {
            return Ok(identity);
        }
        let identity = if self.legacy_path.exists() {
            FileIdentityStore::new(&self.legacy_path).load_or_create()?
        } else {
            InstallationIdentity::generate()
        };
        self.save_keychain(&identity)?;
        match fs::remove_file(&self.legacy_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(identity)
    }
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), IdentityError> {
    let parent = path.parent().ok_or(IdentityError::InvalidData)?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".identity-{}.tmp", Uuid::new_v4()));
    fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_store_reopen_and_proves_possession() {
        let root = std::env::temp_dir().join(format!("ditch-identity-{}", Uuid::new_v4()));
        let store = FileIdentityStore::new(root.join("identity.json"));
        let first = store.load_or_create().unwrap();
        let summary = first.summary();
        let proof = first.sign(b"purchase challenge");
        let second = store.load_or_create().unwrap();
        assert_eq!(summary, second.summary());
        assert_eq!(first.agreement_public_key(), second.agreement_public_key());
        assert!(InstallationIdentity::verify(
            &summary,
            b"purchase challenge",
            &proof
        ));
        assert!(!InstallationIdentity::verify(&summary, b"other", &proof));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_signing_identity_gains_stable_agreement_material() {
        let root = std::env::temp_dir().join(format!("ditch-identity-legacy-{}", Uuid::new_v4()));
        let path = root.join("identity.json");
        let legacy = InstallationIdentity::generate();
        let value = serde_json::json!({
            "version": 1,
            "installation_id": legacy.installation_id(),
            "signing_private_key": URL_SAFE_NO_PAD.encode(legacy.signing.to_bytes()),
        });
        write_private_file(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
        let store = FileIdentityStore::new(&path);
        let first = store.load_or_create().unwrap();
        let second = store.load_or_create().unwrap();
        assert_eq!(first.summary(), legacy.summary());
        assert_eq!(first.agreement_public_key(), second.agreement_public_key());
        let _ = fs::remove_dir_all(root);
    }
}
