//! Public bootstrap for purchasing and installing an authorized Commercial build.
//!
//! This crate is intentionally unaware of the mobile Remote Protocol, pairing,
//! Relay WebSockets, device encryption, projections, and mobile commands.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use ditch_identity::{InstallationIdentity, InstallationIdentitySummary};
use ditch_product::Edition;
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use rand_core::{OsRng, RngCore};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use thiserror::Error;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const OFFICIAL_UPGRADE_API_ORIGIN: &str = "https://relay.ditchnow.nl";
pub const DEPLOYMENT_ENVIRONMENT: &str = match option_env!("DITCH_DEPLOYMENT_ENVIRONMENT") {
    Some(value) => value,
    None => "production",
};
pub const CONFIGURED_UPGRADE_API_ORIGIN: &str = match option_env!("DITCH_RELAY_ORIGIN") {
    Some(value) => value,
    None => OFFICIAL_UPGRADE_API_ORIGIN,
};
pub const CANONICAL_BUNDLE_ID: &str = "ai.theditch.app";

const OFFICIAL_BUILD_SESSION_SAFETY_MARGIN_SECONDS: i64 = 60;

#[derive(Clone)]
struct CachedOfficialBuildSession {
    bearer: String,
    expires_at: DateTime<Utc>,
}

static OFFICIAL_BUILD_SESSION: OnceLock<Mutex<Option<CachedOfficialBuildSession>>> =
    OnceLock::new();
static OFFICIAL_BUILD_CREDENTIAL: OnceLock<Zeroizing<String>> = OnceLock::new();

/// Configures the release credential inside the signed local macOS runtime.
/// Standalone daemons and CLI binaries never call this function and therefore
/// remain unable to establish an official-build session.
pub fn configure_official_build_credential(credential: &[u8]) -> bool {
    let Ok(credential) = std::str::from_utf8(credential) else {
        return false;
    };
    if credential.len() < 43
        || credential.len() > 128
        || !credential
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return false;
    }
    OFFICIAL_BUILD_CREDENTIAL
        .set(Zeroizing::new(credential.to_owned()))
        .is_ok()
}

/// Returns the current short-lived official-build bearer for Commercial
/// transports that share this process. The bearer never crosses IPC.
pub fn cached_official_build_bearer() -> Option<String> {
    OFFICIAL_BUILD_SESSION
        .get()?
        .lock()
        .ok()?
        .as_ref()
        .filter(|session| session.expires_at > Utc::now())
        .map(|session| session.bearer.clone())
}

fn expected_release_channel(environment: &str) -> Result<&'static str, UpgradeError> {
    match environment {
        "staging" => Ok("beta"),
        "production" => Ok("stable"),
        _ => Err(UpgradeError::InvalidResponse(
            "deployment environment must be staging or production".to_owned(),
        )),
    }
}

#[derive(Debug, Error)]
pub enum UpgradeError {
    #[error("upgrade service rejected the request")]
    Rejected,
    #[error("upgrade service response is invalid: {0}")]
    InvalidResponse(String),
    #[error("upgrade network request failed: {0}")]
    Network(String),
    #[error("Ditch Relay rejected the request ({code}): {message}")]
    Relay { code: String, message: String },
    #[error("commercial release authorization has expired")]
    Expired,
    #[error("commercial release would downgrade this installation")]
    Downgrade,
    #[error("commercial release is incompatible with this Community build")]
    Incompatible,
    #[error("commercial release manifest signature is invalid")]
    ManifestSignature,
    #[error("commercial artifact digest does not match its manifest")]
    Digest,
    #[error("commercial artifact size does not match its manifest")]
    Size,
    #[error("commercial release metadata does not match this application")]
    ApplicationIdentity,
    #[error("upgrade filesystem operation failed: {0}")]
    Io(#[from] io::Error),
}

/// The wrapper prevents accidental logging and zeroes the caller-owned buffer
/// when dropped. Production requests serialize it directly and never persist it.
pub struct LicenseKey(Zeroizing<String>);

impl LicenseKey {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for LicenseKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LicenseKey([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingOffer {
    pub offer_id: String,
    pub kind: CommercialOfferKind,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub currency: String,
    pub base_amount_minor: String,
    pub minor_unit_exponent: u8,
    pub billing_type: CommercialBillingType,
    #[serde(default)]
    pub recurring_interval: Option<CommercialBillingInterval>,
    #[serde(default)]
    pub recurring_interval_count: Option<u16>,
    #[serde(default)]
    pub introductory_price: Option<CommercialIntroductoryPrice>,
    pub purchase_action: CommercialPurchaseAction,
    pub eligible: bool,
    #[serde(default)]
    pub ineligible_reason: Option<String>,
    pub entitlement: CommercialOfferEntitlement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommercialOfferKind {
    CommercialMonthly,
    CommercialLifetime,
    LifetimeExtraPair,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommercialBillingType {
    Recurring,
    OneTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommercialBillingInterval {
    Day,
    Week,
    Month,
    Year,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommercialIntroductoryPrice {
    pub amount_minor: String,
    pub duration_count: u16,
    pub duration_unit: CommercialBillingInterval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommercialPurchaseAction {
    Acquire,
    AddCapacity,
    Upgrade,
    Renew,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommercialOfferEntitlement {
    pub mac_slots: u16,
    pub iphone_slots: u16,
    pub ssh_hosts_unlimited: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommercialOfferCatalog {
    pub protocol_version: u16,
    pub offers: Vec<PricingOffer>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default, deserialize_with = "deserialize_optional_utc_timestamp")]
    pub refreshed_at: Option<DateTime<Utc>>,
}

fn deserialize_optional_utc_timestamp<'de, D>(
    deserializer: D,
) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|value| {
            if !value.ends_with('Z') {
                return Err(serde::de::Error::custom(
                    "Commercial catalog refreshed_at must be an RFC 3339 UTC string",
                ));
            }
            DateTime::parse_from_rfc3339(&value)
                .map(|timestamp| timestamp.with_timezone(&Utc))
                .map_err(serde::de::Error::custom)
        })
        .transpose()
}

impl CommercialOfferCatalog {
    fn validate(self) -> Result<Self, UpgradeError> {
        if self.protocol_version != 1 {
            return Err(UpgradeError::InvalidResponse(
                "unsupported Commercial catalog protocol".to_owned(),
            ));
        }
        if self.offers.len() > 100 {
            return Err(UpgradeError::InvalidResponse(
                "Commercial catalog contains too many offers".to_owned(),
            ));
        }
        let mut offer_ids = HashSet::with_capacity(self.offers.len());
        for offer in &self.offers {
            offer.validate()?;
            if !offer_ids.insert(offer.offer_id.as_str()) {
                return Err(UpgradeError::InvalidResponse(
                    "Commercial catalog contains duplicate offer identifiers".to_owned(),
                ));
            }
        }
        Ok(self)
    }
}

impl PricingOffer {
    fn validate(&self) -> Result<(), UpgradeError> {
        if self.offer_id.trim() != self.offer_id
            || self.offer_id.is_empty()
            || self.offer_id.len() > 512
            || self.title.trim().is_empty()
            || self.title.len() > 200
            || self
                .description
                .as_ref()
                .is_some_and(|value| value.len() > 1_000)
            || self.currency.len() != 3
            || !self
                .currency
                .bytes()
                .all(|value| value.is_ascii_uppercase())
            || self.minor_unit_exponent > 6
            || self
                .ineligible_reason
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.len() > 200)
            || (self.eligible && self.ineligible_reason.is_some())
            || (!self.eligible && self.ineligible_reason.is_none())
            || !self.entitlement.ssh_hosts_unlimited
        {
            return Err(UpgradeError::InvalidResponse(
                "Commercial offer metadata is invalid".to_owned(),
            ));
        }
        let base_amount = parse_minor_amount(&self.base_amount_minor)?;
        match self.billing_type {
            CommercialBillingType::Recurring => {
                if self.recurring_interval.is_none()
                    || self.recurring_interval_count.is_none_or(|count| count == 0)
                {
                    return Err(UpgradeError::InvalidResponse(
                        "recurring Commercial offer is missing its interval".to_owned(),
                    ));
                }
            }
            CommercialBillingType::OneTime => {
                if self.recurring_interval.is_some() || self.recurring_interval_count.is_some() {
                    return Err(UpgradeError::InvalidResponse(
                        "one-time Commercial offer contains a recurring interval".to_owned(),
                    ));
                }
            }
        }
        if let Some(introductory) = &self.introductory_price {
            let introductory_amount = parse_minor_amount(&introductory.amount_minor)?;
            if self.billing_type != CommercialBillingType::Recurring
                || introductory.duration_count == 0
                || introductory_amount >= base_amount
            {
                return Err(UpgradeError::InvalidResponse(
                    "Commercial introductory price is invalid".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

fn parse_minor_amount(value: &str) -> Result<u128, UpgradeError> {
    if value.is_empty() || value.len() > 38 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(UpgradeError::InvalidResponse(
            "Commercial amount is not an exact non-negative minor-unit value".to_owned(),
        ));
    }
    value
        .parse()
        .map_err(|_| UpgradeError::InvalidResponse("Commercial amount is too large".to_owned()))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EntitlementSummary {
    pub active: bool,
    pub plan: Option<String>,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub mac_slots: u16,
    pub iphone_slots: u16,
    pub billing_management_available: bool,
    pub renewal_available: bool,
    pub current_license: CurrentLicenseSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CurrentLicenseSummary {
    pub edition: String,
    pub status: String,
    pub display_name: String,
    #[serde(default)]
    pub plans: Vec<CurrentLicensePlan>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CurrentLicensePlan {
    pub kind: String,
    pub display_name: String,
    pub billing_type: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckoutSession {
    pub id: Uuid,
    pub hosted_url: String,
    pub expires_at: DateTime<Utc>,
}

/// A short-lived, Relay-authorized destination for managing an existing Ditch
/// Commercial purchase. The client deliberately treats the destination as an
/// opaque HTTPS URL so the billing provider can change without a macOS update.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BillingManagementSession {
    pub billing_management_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommercialReleaseManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_id: Option<Uuid>,
    pub edition: Edition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub version: String,
    pub build: String,
    pub release_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_community_version: Option<String>,
    pub minimum_community_build: u64,
    pub community_revision: String,
    pub artifact_sha256: String,
    pub artifact_size: u64,
    pub bundle_id: String,
    pub team_id: String,
    pub appcast_url: String,
    pub artifact_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    /// Legacy manifests combined durable release metadata and short-lived
    /// authorization. New manifests omit this; authorization belongs to the
    /// update session below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommercialUpdateSession {
    pub bearer: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignedCommercialRelease {
    pub manifest: CommercialReleaseManifest,
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_session: Option<CommercialUpdateSession>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallationAuthorization {
    pub installation: InstallationIdentitySummary,
    pub issued_at: DateTime<Utc>,
    pub nonce: Uuid,
    pub action: String,
    pub body_sha256: String,
    pub signature: String,
}

impl InstallationAuthorization {
    pub fn new(
        identity: &InstallationIdentity,
        action: impl Into<String>,
        body: &[u8],
    ) -> Result<Self, UpgradeError> {
        let mut authorization = Self {
            installation: identity.summary(),
            issued_at: Utc::now(),
            nonce: Uuid::new_v4(),
            action: action.into(),
            body_sha256: format!("{:x}", Sha256::digest(body)),
            signature: String::new(),
        };
        authorization.signature = identity.sign(&authorization.canonical_bytes()?);
        Ok(authorization)
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, UpgradeError> {
        #[derive(Serialize)]
        struct Proof<'a> {
            installation: &'a InstallationIdentitySummary,
            issued_at: DateTime<Utc>,
            nonce: Uuid,
            action: &'a str,
            body_sha256: &'a str,
        }
        serde_json::to_vec(&Proof {
            installation: &self.installation,
            issued_at: self.issued_at,
            nonce: self.nonce,
            action: &self.action,
            body_sha256: &self.body_sha256,
        })
        .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))
    }

    pub fn verify(&self) -> bool {
        self.canonical_bytes().is_ok_and(|canonical| {
            InstallationIdentity::verify(&self.installation, &canonical, &self.signature)
        })
    }
}

impl CommercialReleaseManifest {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, UpgradeError> {
        serde_json::to_vec(self).map_err(|error| UpgradeError::InvalidResponse(error.to_string()))
    }
}

pub trait UpgradeBackend {
    fn commercial_offers(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<CommercialOfferCatalog, UpgradeError>;

    fn create_checkout(
        &self,
        installation: &InstallationIdentity,
        offer_id: &str,
    ) -> Result<CheckoutSession, UpgradeError>;

    fn entitlement(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<EntitlementSummary, UpgradeError>;

    fn redeem(
        &self,
        installation: &InstallationIdentity,
        license_key: &LicenseKey,
    ) -> Result<EntitlementSummary, UpgradeError>;

    fn billing_management(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<BillingManagementSession, UpgradeError>;

    fn current_release(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<SignedCommercialRelease, UpgradeError>;
}

#[derive(Deserialize)]
struct OfficialBuildSessionEnvelope {
    official_build_session: OfficialBuildSessionResponse,
}

#[derive(Deserialize)]
struct OfficialBuildSessionResponse {
    bearer: String,
    expires_at: DateTime<Utc>,
    build: u64,
    version: String,
}

pub struct HttpUpgradeBackend {
    origin: Url,
}

impl HttpUpgradeBackend {
    pub fn official() -> Result<Self, UpgradeError> {
        Self::for_deployment(DEPLOYMENT_ENVIRONMENT, CONFIGURED_UPGRADE_API_ORIGIN)
    }

    fn for_deployment(environment: &str, origin: &str) -> Result<Self, UpgradeError> {
        match environment {
            "production" if origin != OFFICIAL_UPGRADE_API_ORIGIN => {
                return Err(UpgradeError::InvalidResponse(
                    "production builds must use the official Ditch Relay".to_owned(),
                ));
            }
            "staging" if origin == OFFICIAL_UPGRADE_API_ORIGIN => {
                return Err(UpgradeError::InvalidResponse(
                    "staging builds must not use the production Ditch Relay".to_owned(),
                ));
            }
            "production" | "staging" => {}
            _ => {
                return Err(UpgradeError::InvalidResponse(
                    "deployment environment must be staging or production".to_owned(),
                ));
            }
        }
        Self::new(origin)
    }

    pub fn new(origin: &str) -> Result<Self, UpgradeError> {
        let origin =
            Url::parse(origin).map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.port().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(UpgradeError::InvalidResponse(
                "upgrade API must be an HTTPS origin without credentials, port, path, or query"
                    .to_owned(),
            ));
        }
        Ok(Self { origin })
    }

    /// Exchanges the release-injected build credential for a short-lived,
    /// machine-bound session. Source builds contain no credential and return
    /// `None`, allowing the Relay's rollout mode to decide whether to reject.
    pub fn official_build_authorization(
        &self,
        identity: &InstallationIdentity,
    ) -> Result<Option<String>, UpgradeError> {
        self.official_build_bearer(identity)
    }

    fn endpoint(&self, path: &str) -> Result<Url, UpgradeError> {
        self.origin
            .join(path)
            .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))
    }

    fn relay_error(error: ureq::Error) -> UpgradeError {
        #[derive(Deserialize)]
        struct ErrorEnvelope {
            error: ErrorBody,
        }
        #[derive(Deserialize)]
        struct ErrorBody {
            code: String,
            message: String,
        }
        match error {
            ureq::Error::Status(_, response) => response
                .into_json::<ErrorEnvelope>()
                .map(|body| UpgradeError::Relay {
                    code: body.error.code,
                    message: body.error.message,
                })
                .unwrap_or_else(|error| UpgradeError::Network(error.to_string())),
            error => UpgradeError::Network(error.to_string()),
        }
    }

    fn official_build_number() -> Result<u64, UpgradeError> {
        option_env!("DITCH_BUILD_NUMBER")
            .unwrap_or("0")
            .parse()
            .map_err(|_| UpgradeError::InvalidResponse("invalid official build number".to_owned()))
    }

    fn create_official_build_session(
        &self,
        identity: &InstallationIdentity,
        credential: &str,
    ) -> Result<CachedOfficialBuildSession, UpgradeError> {
        #[derive(Serialize)]
        struct Request {
            protocol_version: u16,
            build: u64,
            version: &'static str,
            edition: &'static str,
            deployment_environment: &'static str,
        }

        let version = option_env!("DITCH_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
        let body = serde_json::to_vec(&Request {
            protocol_version: 1,
            build: Self::official_build_number()?,
            version,
            edition: option_env!("DITCH_EDITION").unwrap_or("community"),
            deployment_environment: DEPLOYMENT_ENVIRONMENT,
        })
        .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        let response: OfficialBuildSessionEnvelope = self.machine_signed_json(
            identity,
            "POST",
            "/v1/official-build/session",
            &body,
            Some(credential),
        )?;
        let session = response.official_build_session;
        if session.expires_at <= Utc::now()
            || session.expires_at > Utc::now() + chrono::Duration::hours(1)
            || session.build != Self::official_build_number()?
            || session.version != version
            || session.bearer.len() < 32
            || session.bearer.len() > 512
            || !session
                .bearer
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(UpgradeError::InvalidResponse(
                "Relay returned an invalid official-build session".to_owned(),
            ));
        }
        Ok(CachedOfficialBuildSession {
            bearer: session.bearer,
            expires_at: session.expires_at,
        })
    }

    fn official_build_bearer(
        &self,
        identity: &InstallationIdentity,
    ) -> Result<Option<String>, UpgradeError> {
        let cache = OFFICIAL_BUILD_SESSION.get_or_init(|| Mutex::new(None));
        if let Some(session) = cache
            .lock()
            .map_err(|_| UpgradeError::Rejected)?
            .as_ref()
            .filter(|session| {
                session.expires_at
                    > Utc::now()
                        + chrono::Duration::seconds(OFFICIAL_BUILD_SESSION_SAFETY_MARGIN_SECONDS)
            })
            .cloned()
        {
            return Ok(Some(session.bearer));
        }

        let Some(credential) = OFFICIAL_BUILD_CREDENTIAL.get() else {
            return Ok(None);
        };
        self.register(identity)?;
        let session = self.create_official_build_session(identity, credential.as_str())?;
        let bearer = session.bearer.clone();
        *cache.lock().map_err(|_| UpgradeError::Rejected)? = Some(session);
        Ok(Some(bearer))
    }

    fn register(&self, identity: &InstallationIdentity) -> Result<(), UpgradeError> {
        #[derive(Serialize)]
        struct Registration<'a> {
            protocol_version: u16,
            machine_id: Uuid,
            name: &'a str,
            signing_public_key: &'a str,
            agreement_public_key: &'a str,
            proof: String,
        }
        let summary = identity.summary();
        let agreement = identity.agreement_public_key();
        let name = "The Ditch on macOS";
        let canonical = format!(
            "DITCH-MACHINE-REGISTER-1\n{}\n{}\n{}\n{}",
            summary.installation_id, name, summary.signing_public_key, agreement
        );
        ureq::post(self.endpoint("/v1/machines/register")?.as_str())
            .set("content-type", "application/json")
            .send_json(Registration {
                protocol_version: 1,
                machine_id: summary.installation_id,
                name,
                signing_public_key: &summary.signing_public_key,
                agreement_public_key: &agreement,
                proof: identity.sign(canonical.as_bytes()),
            })
            .map_err(Self::relay_error)?;
        Ok(())
    }

    fn machine_signed_response(
        &self,
        identity: &InstallationIdentity,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        official_build_bearer: Option<&str>,
    ) -> Result<ureq::Response, UpgradeError> {
        let timestamp = Utc::now().timestamp_millis();
        let mut nonce = [0_u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let nonce = URL_SAFE_NO_PAD.encode(nonce);
        let principal = identity.installation_id();
        let canonical = format!(
            "DITCH1\n{}\n{}\n{:x}\n{}\n{}\n{}",
            method.to_ascii_uppercase(),
            path_and_query,
            Sha256::digest(body),
            timestamp,
            nonce,
            principal
        );
        let mut request = ureq::request(method, self.endpoint(path_and_query)?.as_str())
            .set("X-Ditch-Protocol", "1")
            .set("X-Ditch-Principal-Type", "machine")
            .set("X-Ditch-Principal-Id", &principal.to_string())
            .set("X-Ditch-Timestamp", &timestamp.to_string())
            .set("X-Ditch-Nonce", &nonce)
            .set("X-Ditch-Signature", &identity.sign(canonical.as_bytes()));
        if let Some(bearer) = official_build_bearer {
            request = request.set("X-Ditch-Official-Build", bearer);
        }
        if !body.is_empty() {
            request = request.set("content-type", "application/json");
        }
        if method == "GET" && body.is_empty() {
            request.call().map_err(Self::relay_error)
        } else {
            request.send_bytes(body).map_err(Self::relay_error)
        }
    }

    fn signed_response(
        &self,
        identity: &InstallationIdentity,
        method: &str,
        path_and_query: &str,
        body: &[u8],
    ) -> Result<ureq::Response, UpgradeError> {
        let bearer = self.official_build_bearer(identity)?;
        self.machine_signed_response(identity, method, path_and_query, body, bearer.as_deref())
    }

    fn machine_signed_json<R: for<'de> Deserialize<'de>>(
        &self,
        identity: &InstallationIdentity,
        method: &str,
        path_and_query: &str,
        body: &[u8],
        official_build_bearer: Option<&str>,
    ) -> Result<R, UpgradeError> {
        self.machine_signed_response(
            identity,
            method,
            path_and_query,
            body,
            official_build_bearer,
        )?
        .into_json()
        .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))
    }

    fn signed_json<R: for<'de> Deserialize<'de>>(
        &self,
        identity: &InstallationIdentity,
        method: &str,
        path_and_query: &str,
        body: &[u8],
    ) -> Result<R, UpgradeError> {
        self.signed_response(identity, method, path_and_query, body)?
            .into_json()
            .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))
    }

    fn ensure_owner(&self, identity: &InstallationIdentity) -> Result<(), UpgradeError> {
        self.register(identity)?;
        let _: serde_json::Value =
            self.signed_json(identity, "POST", "/v1/commercial/bootstrap", b"")?;
        Ok(())
    }

    fn validate_external_https_url(value: &str) -> Result<(), UpgradeError> {
        let url =
            Url::parse(value).map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        if url.scheme() != "https" || url.host_str().is_none() {
            return Err(UpgradeError::InvalidResponse(
                "Relay returned a non-HTTPS external URL".to_owned(),
            ));
        }
        Ok(())
    }

    fn commercial_offer_checkout_path(offer_id: &str) -> Result<String, UpgradeError> {
        if offer_id.trim() != offer_id || offer_id.is_empty() || offer_id.len() > 512 {
            return Err(UpgradeError::Rejected);
        }
        let mut checkout_url = Url::parse("https://relay.invalid/v1/commercial/offers/")
            .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        checkout_url
            .path_segments_mut()
            .map_err(|_| UpgradeError::InvalidResponse("invalid checkout endpoint".to_owned()))?
            .pop_if_empty()
            .push(offer_id)
            .push("checkout");
        Ok(checkout_url.path().to_owned())
    }
}

#[derive(Deserialize)]
struct RelayEntitlement {
    plan: String,
    status: String,
    #[serde(default)]
    valid_until: Option<DateTime<Utc>>,
    #[serde(default)]
    billing_management_available: Option<bool>,
    #[serde(default)]
    renewal_available: bool,
    #[serde(default)]
    current_license: Option<CurrentLicenseSummary>,
    capabilities: RelayCapabilities,
}

#[derive(Deserialize)]
struct RelayCapabilities {
    remote_control: RelayRemoteControl,
}

#[derive(Deserialize)]
struct RelayRemoteControl {
    enabled: bool,
    macs: RelaySlots,
    ios: RelaySlots,
}

#[derive(Deserialize)]
struct RelaySlots {
    allowed: u16,
}

impl From<RelayEntitlement> for EntitlementSummary {
    fn from(value: RelayEntitlement) -> Self {
        let active = value.capabilities.remote_control.enabled
            && matches!(value.status.as_str(), "active" | "over_limit");
        let billing_management_available = value.billing_management_available.unwrap_or(false);
        let current_license = value.current_license.unwrap_or_else(|| {
            let commercial = value.plan != "community";
            CurrentLicenseSummary {
                edition: if commercial {
                    "commercial"
                } else {
                    "community"
                }
                .to_owned(),
                status: if commercial {
                    value.status.clone()
                } else {
                    "active".to_owned()
                },
                display_name: if commercial {
                    "Ditch Commercial".to_owned()
                } else {
                    "Ditch Community".to_owned()
                },
                plans: Vec::new(),
            }
        });
        Self {
            active,
            plan: (value.plan != "community").then_some(value.plan),
            status: value.status,
            expires_at: value.valid_until,
            mac_slots: value.capabilities.remote_control.macs.allowed,
            iphone_slots: value.capabilities.remote_control.ios.allowed,
            billing_management_available,
            renewal_available: value.renewal_available,
            current_license,
        }
    }
}

impl UpgradeBackend for HttpUpgradeBackend {
    fn commercial_offers(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<CommercialOfferCatalog, UpgradeError> {
        self.ensure_owner(installation)?;
        let catalog: CommercialOfferCatalog =
            self.signed_json(installation, "GET", "/v1/commercial/offers", b"")?;
        catalog.validate()
    }

    fn create_checkout(
        &self,
        installation: &InstallationIdentity,
        offer_id: &str,
    ) -> Result<CheckoutSession, UpgradeError> {
        #[derive(Serialize)]
        struct Request {
            quantity: u16,
        }
        #[derive(Deserialize)]
        struct Response {
            checkout_intent_id: Uuid,
            checkout_url: String,
            expires_at: i64,
        }
        let path = Self::commercial_offer_checkout_path(offer_id)?;
        self.ensure_owner(installation)?;
        let body = serde_json::to_vec(&Request { quantity: 1 })
            .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        let response: Response = self.signed_json(installation, "POST", &path, &body)?;
        let expires_at = DateTime::from_timestamp_millis(response.expires_at).ok_or_else(|| {
            UpgradeError::InvalidResponse("Commercial checkout expiry is invalid".to_owned())
        })?;
        let now = Utc::now();
        if expires_at <= now || expires_at > now + chrono::Duration::hours(24) {
            return Err(UpgradeError::InvalidResponse(
                "Commercial checkout expiry is outside the allowed window".to_owned(),
            ));
        }
        let checkout = CheckoutSession {
            id: response.checkout_intent_id,
            hosted_url: response.checkout_url,
            expires_at,
        };
        Self::validate_external_https_url(&checkout.hosted_url)?;
        Ok(checkout)
    }

    fn entitlement(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<EntitlementSummary, UpgradeError> {
        let response: RelayEntitlement =
            self.signed_json(installation, "GET", "/v1/commercial/entitlement", b"")?;
        Ok(response.into())
    }

    fn redeem(
        &self,
        installation: &InstallationIdentity,
        license_key: &LicenseKey,
    ) -> Result<EntitlementSummary, UpgradeError> {
        #[derive(Serialize)]
        struct Request<'a> {
            license_key: &'a str,
        }
        #[derive(Deserialize)]
        struct Response {
            entitlement: RelayEntitlement,
        }
        self.ensure_owner(installation)?;
        let body = serde_json::to_vec(&Request {
            license_key: license_key.expose(),
        })
        .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        let response: Response =
            self.signed_json(installation, "POST", "/v1/commercial/license/redeem", &body)?;
        Ok(response.entitlement.into())
    }

    fn billing_management(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<BillingManagementSession, UpgradeError> {
        let session: BillingManagementSession =
            self.signed_json(installation, "POST", "/v1/commercial/customer-portal", b"")?;
        Self::validate_external_https_url(&session.billing_management_url)?;
        Ok(session)
    }

    fn current_release(
        &self,
        installation: &InstallationIdentity,
    ) -> Result<SignedCommercialRelease, UpgradeError> {
        #[derive(Deserialize)]
        struct CurrentResponse {
            release: RelayRelease,
            #[serde(default)]
            update_session: Option<CommercialUpdateSession>,
        }
        #[derive(Deserialize)]
        struct RelayRelease {
            #[serde(default)]
            release_id: Option<Uuid>,
            signature_metadata: serde_json::Value,
        }
        let activate_path = format!(
            "/v1/commercial/devices/{}/activate",
            installation.installation_id()
        );
        let _: serde_json::Value = self.signed_json(installation, "POST", &activate_path, b"")?;
        let channel = expected_release_channel(DEPLOYMENT_ENVIRONMENT)?;
        let path = format!(
            "/v1/commercial/releases/current?channel={channel}&community_version={}&community_build={}",
            option_env!("DITCH_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
            option_env!("DITCH_BUILD_NUMBER").unwrap_or("0")
        );
        let response: CurrentResponse = self.signed_json(installation, "GET", &path, b"")?;
        let release_id = response.release.release_id;
        let metadata = response.release.signature_metadata;
        let mut release: SignedCommercialRelease = serde_json::from_value(metadata.clone())
            .or_else(|_| {
                metadata
                    .get("signed_release")
                    .cloned()
                    .ok_or(())
                    .and_then(|value| serde_json::from_value(value).map_err(|_| ()))
            })
            .map_err(|_| {
                UpgradeError::InvalidResponse(
                    "Relay release metadata does not contain a signed Commercial manifest"
                        .to_owned(),
                )
            })?;
        match (release.manifest.release_id, release_id) {
            (Some(manifest_id), Some(relay_id)) if manifest_id == relay_id => {}
            (Some(_), Some(_)) => {
                return Err(UpgradeError::InvalidResponse(
                    "Relay release ID does not match the signed Commercial manifest".to_owned(),
                ));
            }
            _ => {
                return Err(UpgradeError::InvalidResponse(
                    "Relay release and signed manifest must contain the immutable release ID"
                        .to_owned(),
                ));
            }
        }
        let session = response.update_session.ok_or_else(|| {
            UpgradeError::InvalidResponse(
                "Relay did not provide an authenticated Commercial update session".to_owned(),
            )
        })?;
        if session.expires_at <= Utc::now()
            || session.expires_at > Utc::now() + chrono::Duration::hours(24)
            || session.bearer.len() < 32
            || session.bearer.len() > 512
            || !session
                .bearer
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(UpgradeError::InvalidResponse(
                "Relay returned an invalid Commercial update session".to_owned(),
            ));
        }
        release.update_session = Some(session);
        Ok(release)
    }
}

pub struct ReleaseVerifier {
    manifest_public_key: VerifyingKey,
    expected_team_id: String,
    expected_channel: &'static str,
    expected_origin: Url,
    community_version: Version,
    community_revision: Option<String>,
}

impl ReleaseVerifier {
    pub fn new(
        manifest_public_key_sec1: &[u8],
        expected_team_id: impl Into<String>,
    ) -> Result<Self, UpgradeError> {
        let expected_channel = expected_release_channel(DEPLOYMENT_ENVIRONMENT)?;
        let expected_origin = HttpUpgradeBackend::official()?.origin;
        let community_version =
            Version::parse(option_env!("DITCH_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")))
                .map_err(|_| {
                UpgradeError::InvalidResponse("installed Community version is invalid".to_owned())
            })?;
        let community_revision = option_env!("DITCH_COMMUNITY_REVISION").map(str::to_owned);
        Ok(Self {
            manifest_public_key: VerifyingKey::from_sec1_bytes(manifest_public_key_sec1)
                .map_err(|_| UpgradeError::ManifestSignature)?,
            expected_team_id: expected_team_id.into(),
            expected_channel,
            expected_origin,
            community_version,
            community_revision,
        })
    }

    pub fn verify_manifest(
        &self,
        release: &SignedCommercialRelease,
        now: DateTime<Utc>,
        community_build: u64,
        installed_release_sequence: u64,
    ) -> Result<(), UpgradeError> {
        let signature = URL_SAFE_NO_PAD
            .decode(&release.signature)
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .ok_or(UpgradeError::ManifestSignature)?;
        self.manifest_public_key
            .verify(&release.manifest.canonical_bytes()?, &signature)
            .map_err(|_| UpgradeError::ManifestSignature)?;
        let manifest = &release.manifest;
        if manifest.edition != Edition::Commercial {
            return Err(UpgradeError::InvalidResponse(
                "authorized artifact is not Commercial".to_owned(),
            ));
        }
        if manifest.release_id.is_none() {
            return Err(UpgradeError::InvalidResponse(
                "Commercial release is missing its immutable release ID".to_owned(),
            ));
        }
        if manifest.channel.as_deref() != Some(self.expected_channel) {
            return Err(UpgradeError::InvalidResponse(
                "Commercial release channel does not match this environment".to_owned(),
            ));
        }
        let published_at = manifest.published_at.ok_or_else(|| {
            UpgradeError::InvalidResponse(
                "Commercial release is missing its publication time".to_owned(),
            )
        })?;
        if published_at > now + chrono::Duration::minutes(5) {
            return Err(UpgradeError::InvalidResponse(
                "Commercial release publication time is in the future".to_owned(),
            ));
        }
        if manifest
            .expires_at
            .is_some_and(|expires_at| expires_at <= now)
        {
            return Err(UpgradeError::Expired);
        }
        if manifest.minimum_community_build > community_build {
            return Err(UpgradeError::Incompatible);
        }
        let minimum_version = manifest
            .minimum_community_version
            .as_deref()
            .ok_or_else(|| {
                UpgradeError::InvalidResponse(
                    "Commercial release is missing its minimum Community version".to_owned(),
                )
            })
            .and_then(|value| {
                Version::parse(value).map_err(|_| {
                    UpgradeError::InvalidResponse(
                        "Commercial release minimum Community version is invalid".to_owned(),
                    )
                })
            })?;
        if minimum_version > self.community_version {
            return Err(UpgradeError::Incompatible);
        }
        if manifest.release_sequence < installed_release_sequence {
            return Err(UpgradeError::Downgrade);
        }
        if manifest.bundle_id != CANONICAL_BUNDLE_ID || manifest.team_id != self.expected_team_id {
            return Err(UpgradeError::ApplicationIdentity);
        }
        if manifest.version.trim() != manifest.version
            || Version::parse(&manifest.version).is_err()
            || manifest.build.parse::<u64>().is_err()
            || manifest.release_sequence == 0
            || manifest.artifact_size == 0
            || manifest.artifact_sha256.len() != 64
            || !manifest
                .artifact_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            || manifest.community_revision.len() != 40
            || !manifest
                .community_revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(UpgradeError::InvalidResponse(
                "Commercial release metadata has an invalid format".to_owned(),
            ));
        }
        if self
            .community_revision
            .as_deref()
            .is_some_and(|revision| revision != manifest.community_revision)
        {
            return Err(UpgradeError::Incompatible);
        }
        let session = release.update_session.as_ref().ok_or_else(|| {
            UpgradeError::InvalidResponse(
                "Commercial release is missing its authenticated update session".to_owned(),
            )
        })?;
        if session.expires_at <= now
            || session.expires_at > now + chrono::Duration::hours(24)
            || session.bearer.len() < 32
            || session.bearer.len() > 512
            || !session
                .bearer
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(UpgradeError::InvalidResponse(
                "Commercial update session is invalid or expired".to_owned(),
            ));
        }
        let url = Url::parse(&manifest.artifact_url)
            .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        if url.scheme() != "https"
            || url.origin() != self.expected_origin.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(UpgradeError::InvalidResponse(
                "artifact URL must use the configured Ditch Relay origin".to_owned(),
            ));
        }
        let appcast = Url::parse(&manifest.appcast_url)
            .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?;
        if appcast.scheme() != "https"
            || appcast.origin() != self.expected_origin.origin()
            || !appcast.username().is_empty()
            || appcast.password().is_some()
            || appcast.fragment().is_some()
        {
            return Err(UpgradeError::InvalidResponse(
                "authorized appcast URL must use the configured Ditch Relay origin".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn verify_artifact(
        &self,
        manifest: &CommercialReleaseManifest,
        path: &Path,
    ) -> Result<(), UpgradeError> {
        let metadata = fs::metadata(path)?;
        if metadata.len() != manifest.artifact_size {
            return Err(UpgradeError::Size);
        }
        let mut file = File::open(path)?;
        let mut digest = Sha256::new();
        io::copy(&mut file, &mut digest)?;
        let actual = format!("{:x}", digest.finalize());
        if actual != manifest.artifact_sha256.to_ascii_lowercase() {
            return Err(UpgradeError::Digest);
        }
        Ok(())
    }
}

/// Streams a short-lived authorized URL into a private staging directory. The
/// partial file is never exposed as an installable artifact.
pub fn download_to_staging(
    manifest: &CommercialReleaseManifest,
    staging_root: &Path,
) -> Result<PathBuf, UpgradeError> {
    let response = ureq::get(&manifest.artifact_url)
        .call()
        .map_err(|error| UpgradeError::Network(error.to_string()))?;
    stream_to_staging(manifest, staging_root, response.into_reader())
}

fn stream_to_staging(
    manifest: &CommercialReleaseManifest,
    staging_root: &Path,
    reader: impl Read,
) -> Result<PathBuf, UpgradeError> {
    fs::create_dir_all(staging_root)?;
    let final_path = staging_root.join(format!(
        "commercial-{}-{}.dmg",
        manifest.version, manifest.build
    ));
    let partial_path = staging_root.join(format!(".download-{}.partial", Uuid::new_v4()));
    let result = (|| {
        let mut reader = reader.take(manifest.artifact_size.saturating_add(1));
        let mut output = File::create(&partial_path)?;
        let copied = io::copy(&mut reader, &mut output)?;
        output.flush()?;
        if copied != manifest.artifact_size {
            return Err(UpgradeError::Size);
        }
        fs::rename(&partial_path, &final_path)?;
        Ok(final_path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use ditch_identity::InstallationIdentity;
    use p256::ecdsa::{SigningKey, signature::Signer};
    use rand_core::OsRng;
    use std::sync::Mutex;

    struct FakeBackend {
        redeemed: Mutex<bool>,
    }

    impl UpgradeBackend for FakeBackend {
        fn commercial_offers(
            &self,
            _installation: &InstallationIdentity,
        ) -> Result<CommercialOfferCatalog, UpgradeError> {
            serde_json::from_str(TEST_CATALOG)
                .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))
                .and_then(CommercialOfferCatalog::validate)
        }

        fn create_checkout(
            &self,
            _installation: &InstallationIdentity,
            offer_id: &str,
        ) -> Result<CheckoutSession, UpgradeError> {
            if offer_id.is_empty() {
                return Err(UpgradeError::Rejected);
            }
            Ok(CheckoutSession {
                id: Uuid::new_v4(),
                hosted_url: format!("https://checkout.example.test/ditch/{offer_id}"),
                expires_at: Utc::now() + chrono::Duration::minutes(35),
            })
        }

        fn entitlement(
            &self,
            _installation: &InstallationIdentity,
        ) -> Result<EntitlementSummary, UpgradeError> {
            let active = *self.redeemed.lock().unwrap();
            Ok(EntitlementSummary {
                active,
                plan: active.then(|| "commercial-monthly".to_owned()),
                status: if active { "active" } else { "inactive" }.to_owned(),
                expires_at: None,
                mac_slots: u16::from(active),
                iphone_slots: u16::from(active),
                billing_management_available: active,
                renewal_available: !active,
                current_license: CurrentLicenseSummary {
                    edition: if active { "commercial" } else { "community" }.to_owned(),
                    status: "active".to_owned(),
                    display_name: if active {
                        "Ditch Commercial Test".to_owned()
                    } else {
                        "Ditch Community".to_owned()
                    },
                    plans: Vec::new(),
                },
            })
        }

        fn redeem(
            &self,
            _installation: &InstallationIdentity,
            license_key: &LicenseKey,
        ) -> Result<EntitlementSummary, UpgradeError> {
            if license_key.expose() != "valid-test-license" {
                return Err(UpgradeError::Rejected);
            }
            *self.redeemed.lock().unwrap() = true;
            self.entitlement(_installation)
        }

        fn current_release(
            &self,
            _installation: &InstallationIdentity,
        ) -> Result<SignedCommercialRelease, UpgradeError> {
            Err(UpgradeError::Rejected)
        }

        fn billing_management(
            &self,
            _installation: &InstallationIdentity,
        ) -> Result<BillingManagementSession, UpgradeError> {
            Ok(BillingManagementSession {
                billing_management_url: "https://billing.example.test/ditch/session".to_owned(),
            })
        }
    }

    const TEST_CATALOG: &str = include_str!("../../../docs/contracts/commercial-offers-v1.json");
    const TEST_CATALOG_SCHEMA: &str =
        include_str!("../../../docs/contracts/commercial-offers-v1.schema.json");

    fn signed_release(
        signing: &SigningKey,
        bytes: &[u8],
        sequence: u64,
    ) -> SignedCommercialRelease {
        let manifest = CommercialReleaseManifest {
            release_id: Some(Uuid::new_v4()),
            edition: Edition::Commercial,
            channel: Some(
                expected_release_channel(DEPLOYMENT_ENVIRONMENT)
                    .unwrap()
                    .to_owned(),
            ),
            version: "1.2.3".to_owned(),
            build: "123".to_owned(),
            release_sequence: sequence,
            minimum_community_version: Some("0.1.0".to_owned()),
            minimum_community_build: 10,
            community_revision: "0123456789012345678901234567890123456789".to_owned(),
            artifact_sha256: format!("{:x}", Sha256::digest(bytes)),
            artifact_size: bytes.len() as u64,
            bundle_id: CANONICAL_BUNDLE_ID.to_owned(),
            team_id: "DITCHNOW1".to_owned(),
            appcast_url: format!("{CONFIGURED_UPGRADE_API_ORIGIN}/v1/commercial/appcast/token"),
            artifact_url: format!("{CONFIGURED_UPGRADE_API_ORIGIN}/authorized/example.dmg"),
            published_at: Some(Utc::now()),
            expires_at: Some(Utc::now() + Duration::minutes(5)),
        };
        let signature: Signature = signing.sign(&manifest.canonical_bytes().unwrap());
        SignedCommercialRelease {
            manifest,
            signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
            update_session: Some(CommercialUpdateSession {
                bearer: "a-secure-test-bearer-with-at-least-32-characters".to_owned(),
                expires_at: Utc::now() + Duration::minutes(5),
            }),
        }
    }

    fn resign(signing: &SigningKey, release: &mut SignedCommercialRelease) {
        let signature: Signature = signing.sign(&release.manifest.canonical_bytes().unwrap());
        release.signature = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    }

    fn parse_test_catalog(input: &str) -> Result<CommercialOfferCatalog, UpgradeError> {
        serde_json::from_str::<CommercialOfferCatalog>(input)
            .map_err(|error| UpgradeError::InvalidResponse(error.to_string()))?
            .validate()
    }

    #[test]
    fn commercial_offer_contract_fixture_preserves_exact_money_and_introductory_terms() {
        let catalog = parse_test_catalog(TEST_CATALOG).unwrap();
        assert!(!catalog.stale);
        assert_eq!(
            catalog.refreshed_at,
            Some("2026-08-30T08:00:00Z".parse().unwrap())
        );
        assert_eq!(catalog.offers[1].base_amount_minor, "1500");
        assert_eq!(
            catalog.offers[1]
                .introductory_price
                .as_ref()
                .unwrap()
                .amount_minor,
            "750"
        );
        assert_eq!(catalog.offers[2].entitlement.mac_slots, 2);
        assert!(
            catalog
                .offers
                .iter()
                .all(|offer| offer.entitlement.ssh_hosts_unlimited)
        );
    }

    #[test]
    fn commercial_offer_contract_fixture_matches_its_published_json_schema() {
        let schema = serde_json::from_str(TEST_CATALOG_SCHEMA).unwrap();
        let fixture = serde_json::from_str(TEST_CATALOG).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let errors = validator
            .iter_errors(&fixture)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
        assert!(errors.is_empty(), "schema errors: {errors:?}");
    }

    #[test]
    fn commercial_offer_contract_rejects_noncanonical_refresh_timestamps() {
        let mut catalog = serde_json::from_str::<serde_json::Value>(TEST_CATALOG).unwrap();
        catalog["refreshed_at"] = serde_json::json!(1_788_085_651_552_u64);
        let error = parse_test_catalog(&catalog.to_string()).unwrap_err();
        assert!(error.to_string().contains("expected a string"));

        catalog["refreshed_at"] = serde_json::json!("not-a-timestamp");
        let error = parse_test_catalog(&catalog.to_string()).unwrap_err();
        assert!(matches!(error, UpgradeError::InvalidResponse(_)));

        catalog["refreshed_at"] = serde_json::json!("2026-08-30T10:00:00+02:00");
        let error = parse_test_catalog(&catalog.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("refreshed_at must be an RFC 3339 UTC string")
        );
    }

    #[test]
    fn commercial_offer_contract_rejects_protocol_drift_and_broken_relay_fields() {
        let mut catalog = serde_json::from_str::<serde_json::Value>(TEST_CATALOG).unwrap();
        catalog["protocol_version"] = serde_json::json!(2);
        assert!(matches!(
            parse_test_catalog(&catalog.to_string()),
            Err(UpgradeError::InvalidResponse(message))
                if message == "unsupported Commercial catalog protocol"
        ));

        let mut catalog = serde_json::from_str::<serde_json::Value>(TEST_CATALOG).unwrap();
        let first = catalog["offers"][0].as_object_mut().unwrap();
        let amount = first.remove("base_amount_minor").unwrap();
        first.insert("normal_unit_amount".to_owned(), amount);
        let error = parse_test_catalog(&catalog.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown field `normal_unit_amount`")
        );
    }

    #[test]
    fn commercial_offer_contract_rejects_invalid_money_intro_and_entitlement() {
        let fixture = serde_json::from_str::<serde_json::Value>(TEST_CATALOG).unwrap();

        let mut invalid_amount = fixture.clone();
        invalid_amount["offers"][0]["base_amount_minor"] = serde_json::json!("15.00");
        assert!(parse_test_catalog(&invalid_amount.to_string()).is_err());

        let mut invalid_exponent = fixture.clone();
        invalid_exponent["offers"][0]["minor_unit_exponent"] = serde_json::json!(7);
        assert!(parse_test_catalog(&invalid_exponent.to_string()).is_err());

        let mut invalid_intro = fixture.clone();
        invalid_intro["offers"][1]["introductory_price"]["amount_minor"] =
            serde_json::json!("1500");
        assert!(parse_test_catalog(&invalid_intro.to_string()).is_err());

        let mut metered_ssh = fixture;
        metered_ssh["offers"][0]["entitlement"]["ssh_hosts_unlimited"] = serde_json::json!(false);
        assert!(parse_test_catalog(&metered_ssh.to_string()).is_err());
    }

    #[test]
    fn commercial_offer_contract_rejects_duplicate_opaque_offer_ids() {
        let mut catalog = serde_json::from_str::<serde_json::Value>(TEST_CATALOG).unwrap();
        catalog["offers"][1]["offer_id"] = catalog["offers"][0]["offer_id"].clone();
        assert!(matches!(
            parse_test_catalog(&catalog.to_string()),
            Err(UpgradeError::InvalidResponse(message))
                if message == "Commercial catalog contains duplicate offer identifiers"
        ));
    }

    #[test]
    fn fake_backend_checkout_redemption_and_billing_are_provider_neutral() {
        let backend = FakeBackend {
            redeemed: Mutex::new(false),
        };
        let installation = InstallationIdentity::generate();
        let catalog = backend.commercial_offers(&installation).unwrap();
        assert_eq!(catalog.offers.len(), 3);
        let checkout = backend
            .create_checkout(&installation, "opaque-monthly-offer")
            .unwrap();
        assert!(checkout.hosted_url.starts_with("https://"));
        for offer_id in ["opaque-lifetime-offer", "opaque-extra-pair-offer"] {
            assert!(
                backend
                    .create_checkout(&installation, offer_id)
                    .unwrap()
                    .hosted_url
                    .ends_with(offer_id)
            );
        }
        assert!(backend.create_checkout(&installation, "").is_err());
        assert!(!backend.entitlement(&installation).unwrap().active);
        assert!(
            backend
                .redeem(&installation, &LicenseKey::new("wrong".to_owned()))
                .is_err()
        );
        let active = backend
            .redeem(
                &installation,
                &LicenseKey::new("valid-test-license".to_owned()),
            )
            .unwrap();
        assert!(active.active);
        assert_eq!((active.mac_slots, active.iphone_slots), (1, 1));
        let billing = backend.billing_management(&installation).unwrap();
        assert_eq!(
            billing.billing_management_url,
            "https://billing.example.test/ditch/session"
        );
    }

    #[test]
    fn installation_authorization_proves_possession_and_binds_action_and_body() {
        let identity = InstallationIdentity::generate();
        let mut authorization =
            InstallationAuthorization::new(&identity, "commercial.checkout", b"commercial-monthly")
                .unwrap();
        assert!(authorization.verify());
        authorization.action = "commercial.release.current".to_owned();
        assert!(!authorization.verify());
    }

    #[test]
    fn hosted_destinations_must_be_https_but_are_not_provider_hardcoded() {
        assert!(
            HttpUpgradeBackend::validate_external_https_url(
                "https://payments.example.test/hosted/session"
            )
            .is_ok()
        );
        assert!(
            HttpUpgradeBackend::validate_external_https_url(
                "http://payments.example.test/hosted/session"
            )
            .is_err()
        );
        assert!(HttpUpgradeBackend::validate_external_https_url("not a URL").is_err());
    }

    #[test]
    fn deployment_origin_cannot_cross_staging_and_production() {
        assert!(
            HttpUpgradeBackend::for_deployment("production", OFFICIAL_UPGRADE_API_ORIGIN).is_ok()
        );
        assert!(
            HttpUpgradeBackend::for_deployment(
                "staging",
                "https://ditch-remote-relay-staging.example.workers.dev",
            )
            .is_ok()
        );
        assert!(
            HttpUpgradeBackend::for_deployment("staging", OFFICIAL_UPGRADE_API_ORIGIN).is_err()
        );
        assert!(
            HttpUpgradeBackend::for_deployment("production", "https://relay.example.test").is_err()
        );
        assert!(
            HttpUpgradeBackend::for_deployment("development", "https://relay.example.test")
                .is_err()
        );
    }

    #[test]
    fn compiled_deployment_configuration_is_valid() {
        assert!(HttpUpgradeBackend::official().is_ok());
    }

    #[test]
    fn upgrade_backend_requires_a_bare_https_origin() {
        assert!(HttpUpgradeBackend::new("https://relay.example.test").is_ok());
        assert!(HttpUpgradeBackend::new("http://relay.example.test").is_err());
        assert!(HttpUpgradeBackend::new("https://relay.example.test/api").is_err());
        assert!(HttpUpgradeBackend::new("https://user@relay.example.test").is_err());
        assert!(HttpUpgradeBackend::new("https://relay.example.test:8443").is_err());
    }

    #[test]
    fn opaque_offer_id_is_one_encoded_checkout_path_segment() {
        assert_eq!(
            HttpUpgradeBackend::commercial_offer_checkout_path("opaque/offer?revision=2").unwrap(),
            "/v1/commercial/offers/opaque%2Foffer%3Frevision=2/checkout"
        );
        assert!(HttpUpgradeBackend::commercial_offer_checkout_path(" padded ").is_err());
    }

    #[test]
    fn release_rejects_tampering_expiry_and_downgrade() {
        let signing = SigningKey::random(&mut OsRng);
        let verifier = ReleaseVerifier::new(
            VerifyingKey::from(&signing)
                .to_encoded_point(false)
                .as_bytes(),
            "DITCHNOW1",
        )
        .unwrap();
        let mut release = signed_release(&signing, b"artifact", 20);
        verifier
            .verify_manifest(&release, Utc::now(), 10, 20)
            .unwrap();
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 21),
            Err(UpgradeError::Downgrade)
        ));
        release.manifest.expires_at = Some(Utc::now() - Duration::seconds(1));
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::ManifestSignature)
        ));
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::Expired)
        ));
    }

    #[test]
    fn release_rejects_wrong_application_and_team_identity() {
        let signing = SigningKey::random(&mut OsRng);
        let verifier = ReleaseVerifier::new(
            VerifyingKey::from(&signing)
                .to_encoded_point(false)
                .as_bytes(),
            "DITCHNOW1",
        )
        .unwrap();
        let mut release = signed_release(&signing, b"artifact", 20);
        release.manifest.bundle_id = "example.attacker.app".to_owned();
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::ApplicationIdentity)
        ));
        release.manifest.bundle_id = CANONICAL_BUNDLE_ID.to_owned();
        release.manifest.team_id = "OTHERTEAM".to_owned();
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::ApplicationIdentity)
        ));
    }

    #[test]
    fn release_requires_matching_channel_origin_and_authenticated_session() {
        let signing = SigningKey::random(&mut OsRng);
        let verifier = ReleaseVerifier::new(
            VerifyingKey::from(&signing)
                .to_encoded_point(false)
                .as_bytes(),
            "DITCHNOW1",
        )
        .unwrap();
        let mut release = signed_release(&signing, b"artifact", 20);

        release.manifest.channel = Some("wrong-channel".to_owned());
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::InvalidResponse(_))
        ));

        release = signed_release(&signing, b"artifact", 20);
        release.manifest.artifact_url = "https://attacker.example/artifact.dmg".to_owned();
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::InvalidResponse(_))
        ));

        release = signed_release(&signing, b"artifact", 20);
        release.update_session = None;
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::InvalidResponse(_))
        ));
    }

    #[test]
    fn release_requires_compatible_version_revision_and_well_formed_artifact_metadata() {
        let signing = SigningKey::random(&mut OsRng);
        let verifier = ReleaseVerifier::new(
            VerifyingKey::from(&signing)
                .to_encoded_point(false)
                .as_bytes(),
            "DITCHNOW1",
        )
        .unwrap();
        let mut release = signed_release(&signing, b"artifact", 20);

        release.manifest.minimum_community_version = Some("999.0.0".to_owned());
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::Incompatible)
        ));

        release = signed_release(&signing, b"artifact", 20);
        release.manifest.community_revision = "not-a-commit".to_owned();
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::InvalidResponse(_))
        ));

        release = signed_release(&signing, b"artifact", 20);
        release.manifest.artifact_sha256 = "A0".repeat(32);
        resign(&signing, &mut release);
        assert!(matches!(
            verifier.verify_manifest(&release, Utc::now(), 10, 20),
            Err(UpgradeError::InvalidResponse(_))
        ));
    }

    #[test]
    fn artifact_rejects_wrong_hash_and_size() {
        let signing = SigningKey::random(&mut OsRng);
        let verifier = ReleaseVerifier::new(
            VerifyingKey::from(&signing)
                .to_encoded_point(false)
                .as_bytes(),
            "DITCHNOW1",
        )
        .unwrap();
        let root = std::env::temp_dir().join(format!("ditch-upgrade-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("artifact.dmg");
        fs::write(&path, b"artifact").unwrap();
        let mut release = signed_release(&signing, b"artifact", 20);
        verifier.verify_artifact(&release.manifest, &path).unwrap();
        release.manifest.artifact_sha256 = "00".repeat(32);
        assert!(matches!(
            verifier.verify_artifact(&release.manifest, &path),
            Err(UpgradeError::Digest)
        ));
        release.manifest.artifact_size = 1;
        assert!(matches!(
            verifier.verify_artifact(&release.manifest, &path),
            Err(UpgradeError::Size)
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn partial_download_is_never_promoted_or_left_behind() {
        let signing = SigningKey::random(&mut OsRng);
        let release = signed_release(&signing, b"complete-artifact", 20);
        let root = std::env::temp_dir().join(format!("ditch-partial-{}", Uuid::new_v4()));
        let result = stream_to_staging(&release.manifest, &root, std::io::Cursor::new(b"partial"));
        assert!(matches!(result, Err(UpgradeError::Size)));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn license_debug_is_always_redacted() {
        let key = LicenseKey::new("secret-value".to_owned());
        assert_eq!(format!("{key:?}"), "LicenseKey([REDACTED])");
    }
}
