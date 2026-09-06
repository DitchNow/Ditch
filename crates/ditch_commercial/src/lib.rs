//! Proprietary Commercial capability composition.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ditch_identity::InstallationIdentity;
pub use ditch_identity::InstallationIdentitySummary;
use ditch_remote::MachineIdentity;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use url::Url;

pub const CONFIGURED_RELAY_ORIGIN: &str = match option_env!("DITCH_RELAY_ORIGIN") {
    Some(value) => value,
    None => "https://relay.ditchnow.nl",
};

/// Stable private capability identifier. New Commercial features register a
/// provider under their own identifier; the runtime itself never branches on
/// plans, billing products, or a global paid/free boolean.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommercialCapabilityId(&'static str);

impl CommercialCapabilityId {
    pub const REMOTE_CONTROL: Self = Self("remote_control");

    pub const fn new(value: &'static str) -> Self {
        Self(value)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommercialEntitlementStatus {
    Inactive,
    Active,
    OverLimit,
    Expired,
    Refunded,
}

/// Provider-neutral entitlement state issued by Ditch Relay. Stripe product,
/// Price, customer, and subscription identifiers never enter this model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommercialEntitlement {
    pub plan: String,
    pub status: CommercialEntitlementStatus,
    pub capabilities: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<DateTime<Utc>>,
    pub refreshed_at: DateTime<Utc>,
}

impl CommercialEntitlement {
    pub fn inactive(now: DateTime<Utc>) -> Self {
        Self {
            plan: "community".to_owned(),
            status: CommercialEntitlementStatus::Inactive,
            capabilities: BTreeSet::new(),
            valid_until: None,
            refreshed_at: now,
        }
    }

    pub fn active_at(&self, now: DateTime<Utc>) -> bool {
        matches!(
            self.status,
            CommercialEntitlementStatus::Active | CommercialEntitlementStatus::OverLimit
        ) && self.valid_until.is_none_or(|expires_at| expires_at > now)
    }

    pub fn allows(&self, capability: CommercialCapabilityId, now: DateTime<Utc>) -> bool {
        self.active_at(now) && self.capabilities.contains(capability.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommercialCapabilityDescriptor {
    pub id: CommercialCapabilityId,
    pub runtime_capability: &'static str,
}

pub const REMOTE_CONTROL_CAPABILITY: CommercialCapabilityDescriptor =
    CommercialCapabilityDescriptor {
        id: CommercialCapabilityId::REMOTE_CONTROL,
        runtime_capability: "remote_control_v1",
    };

/// Compile-time Commercial composition. Adding a future paid feature adds a
/// provider/descriptor here without changing the Community runtime authority
/// or introducing another daemon.
#[derive(Clone, Debug)]
pub struct CommercialCapabilityRegistry {
    descriptors: Vec<CommercialCapabilityDescriptor>,
}

impl CommercialCapabilityRegistry {
    pub fn new(
        descriptors: impl IntoIterator<Item = CommercialCapabilityDescriptor>,
    ) -> Result<Self, String> {
        let descriptors = descriptors.into_iter().collect::<Vec<_>>();
        let mut ids = BTreeSet::new();
        let mut runtime_capabilities = BTreeSet::new();
        for descriptor in &descriptors {
            if descriptor.id.as_str().is_empty()
                || !descriptor
                    .id
                    .as_str()
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(format!(
                    "invalid Commercial capability identifier: {}",
                    descriptor.id.as_str()
                ));
            }
            if !ids.insert(descriptor.id.as_str()) {
                return Err(format!(
                    "duplicate Commercial capability identifier: {}",
                    descriptor.id.as_str()
                ));
            }
            if !runtime_capabilities.insert(descriptor.runtime_capability) {
                return Err(format!(
                    "duplicate runtime capability: {}",
                    descriptor.runtime_capability
                ));
            }
        }
        Ok(Self { descriptors })
    }

    pub fn standard() -> Self {
        Self::new([REMOTE_CONTROL_CAPABILITY])
            .expect("built-in Commercial capability registry must be valid")
    }

    pub fn allows(
        &self,
        entitlement: &CommercialEntitlement,
        capability: CommercialCapabilityId,
        now: DateTime<Utc>,
    ) -> bool {
        self.descriptors.iter().any(|entry| entry.id == capability)
            && entitlement.allows(capability, now)
    }

    pub fn enabled_runtime_capabilities(
        &self,
        entitlement: &CommercialEntitlement,
        now: DateTime<Utc>,
    ) -> Vec<String> {
        self.descriptors
            .iter()
            .filter(|entry| entitlement.allows(entry.id, now))
            .map(|entry| entry.runtime_capability.to_owned())
            .collect()
    }
}

#[derive(Deserialize)]
struct EntitlementResponse {
    #[serde(default = "community_plan")]
    plan: String,
    status: String,
    #[serde(default)]
    valid_until: Option<DateTime<Utc>>,
    capabilities: serde_json::Map<String, serde_json::Value>,
}

fn community_plan() -> String {
    "community".to_owned()
}

#[derive(Deserialize)]
struct CapabilityState {
    enabled: bool,
}

#[derive(Deserialize)]
struct BillingManagementResponse {
    billing_management_url: String,
}

fn register(identity: &InstallationIdentity) -> Result<(), String> {
    let summary = identity.summary();
    let signing_public_key = summary.signing_public_key;
    let agreement_public_key = identity.agreement_public_key();
    let name = "The Ditch on macOS";
    let canonical = format!(
        "DITCH-MACHINE-REGISTER-1\n{}\n{}\n{}\n{}",
        summary.installation_id, name, signing_public_key, agreement_public_key
    );
    let response = ureq::post(&format!(
        "{}/v1/machines/register",
        configured_relay_origin()?
    ))
    .set("content-type", "application/json")
    .send_json(serde_json::json!({
        "protocol_version": 1,
        "machine_id": summary.installation_id,
        "name": name,
        "signing_public_key": signing_public_key,
        "agreement_public_key": agreement_public_key,
        "proof": identity.sign(canonical.as_bytes()),
    }))
    .map_err(|error| error.to_string())?;
    if response.status() != 201 {
        return Err(format!("Relay registration returned {}", response.status()));
    }
    Ok(())
}

fn signed_request(
    identity: &InstallationIdentity,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<ureq::Response, String> {
    let official_build_bearer = ditch_upgrade::HttpUpgradeBackend::official()
        .and_then(|backend| backend.official_build_authorization(identity))
        .map_err(|error| error.to_string())?;
    let installation_id = identity.installation_id();
    let timestamp = Utc::now().timestamp_millis();
    let mut nonce = [0_u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let nonce = URL_SAFE_NO_PAD.encode(nonce);
    let canonical = format!(
        "DITCH1\n{}\n{}\n{:x}\n{}\n{}\n{}",
        method.to_ascii_uppercase(),
        path,
        Sha256::digest(body),
        timestamp,
        nonce,
        installation_id,
    );
    let mut request = ureq::request(method, &format!("{}{path}", configured_relay_origin()?))
        .set("X-Ditch-Protocol", "1")
        .set("X-Ditch-Principal-Type", "machine")
        .set("X-Ditch-Principal-Id", &installation_id.to_string())
        .set("X-Ditch-Timestamp", &timestamp.to_string())
        .set("X-Ditch-Nonce", &nonce)
        .set("X-Ditch-Signature", &identity.sign(canonical.as_bytes()));
    if let Some(bearer) = official_build_bearer.as_deref() {
        request = request.set("X-Ditch-Official-Build", bearer);
    }
    if !body.is_empty() {
        request = request.set("content-type", "application/json");
    }
    if method == "GET" && body.is_empty() {
        request.call().map_err(|error| error.to_string())
    } else {
        request.send_bytes(body).map_err(|error| error.to_string())
    }
}

/// Refreshes only sanitized entitlement state. It reuses the Community
/// installation identity and never creates a second machine/license owner.
pub fn refresh_entitlement(
    identity: &InstallationIdentity,
) -> Result<CommercialEntitlement, String> {
    register(identity)?;
    let _: serde_json::Value = signed_request(identity, "POST", "/v1/commercial/bootstrap", b"")?
        .into_json()
        .map_err(|error| error.to_string())?;
    let response: EntitlementResponse =
        signed_request(identity, "GET", "/v1/commercial/entitlement", b"")?
            .into_json()
            .map_err(|error| error.to_string())?;
    let status = match response.status.as_str() {
        "active" => CommercialEntitlementStatus::Active,
        "over_limit" => CommercialEntitlementStatus::OverLimit,
        "expired" => CommercialEntitlementStatus::Expired,
        "refunded" => CommercialEntitlementStatus::Refunded,
        _ => CommercialEntitlementStatus::Inactive,
    };
    let capabilities = response
        .capabilities
        .into_iter()
        .filter_map(|(id, value)| {
            serde_json::from_value::<CapabilityState>(value)
                .ok()
                .filter(|state| state.enabled)
                .map(|_| id)
        })
        .collect();
    Ok(CommercialEntitlement {
        plan: response.plan,
        status,
        capabilities,
        valid_until: response.valid_until,
        refreshed_at: Utc::now(),
    })
}

pub fn configured_relay_origin() -> Result<&'static str, String> {
    let parsed = Url::parse(CONFIGURED_RELAY_ORIGIN).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(
            "DITCH_RELAY_ORIGIN must be an HTTPS origin without credentials or a path".to_owned(),
        );
    }
    Ok(CONFIGURED_RELAY_ORIGIN.trim_end_matches('/'))
}

/// Requests an opaque hosted billing destination from Ditch Relay. No billing
/// provider identifiers or SDK contracts cross this boundary.
pub fn billing_management_url(identity: &InstallationIdentity) -> Result<String, String> {
    let response: BillingManagementResponse =
        signed_request(identity, "POST", "/v1/commercial/customer-portal", b"")?
            .into_json()
            .map_err(|error| error.to_string())?;
    validate_billing_management_url(response.billing_management_url)
}

fn validate_billing_management_url(value: String) -> Result<String, String> {
    let parsed = Url::parse(&value).map_err(|error| error.to_string())?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err("Ditch Relay returned a non-HTTPS billing URL".to_owned());
    }
    Ok(value)
}

pub fn remote_machine_identity(identity: &InstallationIdentity) -> Result<MachineIdentity, String> {
    let private_blob = identity
        .private_identity_blob()
        .map_err(|error| error.to_string())?;
    let (signing, agreement): (String, String) =
        serde_json::from_slice(&private_blob).map_err(|error| error.to_string())?;
    let signing = URL_SAFE_NO_PAD
        .decode(signing)
        .map_err(|_| "installation signing key is invalid".to_owned())?;
    let agreement = URL_SAFE_NO_PAD
        .decode(agreement)
        .map_err(|_| "installation agreement key is invalid".to_owned())?;
    MachineIdentity::from_private_scalars(identity.installation_id(), &signing, &agreement)
        .map_err(|error| error.to_string())
}

/// Commercial composition supplies this provider to the one authoritative
/// runtime. Community services never consult it and therefore remain available
/// when a subscription expires.
pub trait CommercialCapabilityProvider: Send + Sync {
    fn descriptor(&self) -> CommercialCapabilityDescriptor;

    fn entitlement_changed(&self, _entitlement: &CommercialEntitlement) {}

    fn shutdown(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use ditch_identity::{FileIdentityStore, IdentityStore};
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn expiry_disables_only_commercial_capabilities() {
        let now = Utc::now();
        let active = CommercialEntitlement {
            plan: "commercial_monthly".to_owned(),
            status: CommercialEntitlementStatus::Active,
            capabilities: BTreeSet::from(["remote_control".to_owned()]),
            valid_until: Some(now + Duration::minutes(1)),
            refreshed_at: now,
        };
        let expired = CommercialEntitlement {
            valid_until: Some(now - Duration::seconds(1)),
            ..active.clone()
        };
        assert!(active.active_at(now));
        assert!(!expired.active_at(now));
        assert!(active.allows(CommercialCapabilityId::REMOTE_CONTROL, now));
        assert!(!CommercialEntitlement::inactive(now).active_at(now));
    }

    #[test]
    fn registry_gates_capabilities_independently() {
        const TEST_CAPABILITY: CommercialCapabilityId =
            CommercialCapabilityId::new("test_capability");
        let registry = CommercialCapabilityRegistry::new([
            REMOTE_CONTROL_CAPABILITY,
            CommercialCapabilityDescriptor {
                id: TEST_CAPABILITY,
                runtime_capability: "test_capability_v1",
            },
        ])
        .unwrap();
        let now = Utc::now();
        let entitlement = CommercialEntitlement {
            plan: "test".to_owned(),
            status: CommercialEntitlementStatus::Active,
            capabilities: BTreeSet::from(["test_capability".to_owned()]),
            valid_until: None,
            refreshed_at: now,
        };
        assert!(!registry.allows(&entitlement, CommercialCapabilityId::REMOTE_CONTROL, now));
        assert!(registry.allows(&entitlement, TEST_CAPABILITY, now));
        assert_eq!(
            registry.enabled_runtime_capabilities(&entitlement, now),
            vec!["test_capability_v1"]
        );
    }

    #[test]
    fn commercial_reuses_the_persisted_community_installation_identity() {
        let root =
            std::env::temp_dir().join(format!("ditch-commercial-identity-{}", Uuid::new_v4()));
        let path = root.join("identity/installation-v1.json");
        let first = FileIdentityStore::new(&path).load_or_create().unwrap();
        let second = FileIdentityStore::new(&path).load_or_create().unwrap();
        assert_eq!(first.summary(), second.summary());
        assert_eq!(first.agreement_public_key(), second.agreement_public_key());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn billing_destination_is_https_and_provider_neutral() {
        assert_eq!(
            validate_billing_management_url(
                "https://payments.example.test/ditch/session".to_owned()
            )
            .unwrap(),
            "https://payments.example.test/ditch/session"
        );
        assert!(
            validate_billing_management_url(
                "http://payments.example.test/ditch/session".to_owned()
            )
            .is_err()
        );
    }
}
