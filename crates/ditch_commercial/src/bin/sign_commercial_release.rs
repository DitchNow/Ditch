use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Utc};
use p256::ecdsa::{
    Signature, SigningKey,
    signature::{Signer, Verifier},
};
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;
use uuid::Uuid;

#[derive(Serialize)]
struct CommercialReleaseManifest {
    release_id: Uuid,
    edition: &'static str,
    channel: String,
    version: String,
    build: String,
    release_sequence: u64,
    minimum_community_version: String,
    minimum_community_build: u64,
    community_revision: String,
    artifact_sha256: String,
    artifact_size: u64,
    bundle_id: String,
    team_id: String,
    appcast_url: String,
    artifact_url: String,
    published_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct SignedCommercialRelease {
    manifest: CommercialReleaseManifest,
    signature: String,
}

fn required(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("required environment variable {name} is missing").into())
}

fn parse_u64(name: &str) -> Result<u64, Box<dyn Error>> {
    Ok(required(name)?.parse()?)
}

fn valid_revision(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn select_descriptor_revision(
    actual_revision: String,
    legacy_revision: Option<String>,
) -> Result<String, Box<dyn Error>> {
    let revision = legacy_revision.unwrap_or(actual_revision);
    if !valid_revision(&revision) {
        return Err(
            "release descriptor Community revision must be one lowercase full Git commit".into(),
        );
    }
    Ok(revision)
}

fn descriptor_revision(actual_revision: String) -> Result<String, Box<dyn Error>> {
    let legacy_revision = env::var("DITCH_LEGACY_SOURCE_COMMUNITY_REVISION")
        .ok()
        .filter(|value| !value.is_empty());
    select_descriptor_revision(actual_revision, legacy_revision)
}

fn main() -> Result<(), Box<dyn Error>> {
    let artifact = PathBuf::from(required("DITCH_COMMERCIAL_ARTIFACT")?);
    let output = PathBuf::from(required("DITCH_COMMERCIAL_MANIFEST_OUTPUT")?);
    let artifact_bytes = fs::read(&artifact)?;
    let actual_revision = fs::read_to_string("COMMUNITY_REVISION")?.trim().to_owned();
    if !valid_revision(&actual_revision) {
        return Err("COMMUNITY_REVISION must contain one lowercase full Git commit".into());
    }
    let revision = descriptor_revision(actual_revision)?;
    let published_at =
        DateTime::parse_from_rfc3339(&required("DITCH_RELEASE_PUBLISHED_AT")?)?.with_timezone(&Utc);
    if published_at > Utc::now() + chrono::Duration::minutes(5) {
        return Err("release publication time is unreasonably far in the future".into());
    }
    let environment = required("DITCH_DEPLOYMENT_ENVIRONMENT")?;
    let channel = required("DITCH_RELEASE_CHANNEL")?;
    let expected_channel = match environment.as_str() {
        "staging" => "beta",
        "production" => "stable",
        _ => return Err("deployment environment must be staging or production".into()),
    };
    if channel != expected_channel {
        return Err(format!("{environment} releases must use channel {expected_channel}").into());
    }
    let version = required("DITCH_RELEASE_VERSION")?;
    let minimum_community_version = required("DITCH_MINIMUM_COMMUNITY_VERSION")?;
    Version::parse(&version)?;
    Version::parse(&minimum_community_version)?;
    let build = required("DITCH_RELEASE_BUILD")?;
    if build.parse::<u64>()? == 0 {
        return Err("release build must be a positive integer".into());
    }
    let release_sequence = parse_u64("DITCH_RELEASE_SEQUENCE")?;
    if release_sequence == 0 || artifact_bytes.is_empty() {
        return Err("release sequence and artifact size must be positive".into());
    }
    let bundle_id = required("DITCH_BUNDLE_ID")?;
    if bundle_id != "ai.theditch.app" {
        return Err("Commercial release has the wrong application bundle ID".into());
    }
    let team_id = required("DITCH_APPLE_TEAM_ID")?;
    if team_id.len() != 10
        || !team_id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return Err("Apple Team ID must contain 10 uppercase letters or digits".into());
    }
    let manifest = CommercialReleaseManifest {
        release_id: Uuid::parse_str(&required("DITCH_RELEASE_ID")?)?,
        edition: "commercial",
        channel,
        version,
        build,
        release_sequence,
        minimum_community_version,
        minimum_community_build: parse_u64("DITCH_MINIMUM_COMMUNITY_BUILD")?,
        community_revision: revision,
        artifact_sha256: format!("{:x}", Sha256::digest(&artifact_bytes)),
        artifact_size: artifact_bytes.len() as u64,
        bundle_id,
        team_id,
        appcast_url: required("DITCH_AUTHORIZED_APPCAST_URL")?,
        artifact_url: required("DITCH_AUTHORIZED_ARTIFACT_URL")?,
        published_at,
    };
    let relay = Url::parse(&required("DITCH_RELAY_ORIGIN")?)?;
    if relay.scheme() != "https"
        || relay.host_str().is_none()
        || !relay.username().is_empty()
        || relay.password().is_some()
        || relay.port().is_some()
        || relay.path() != "/"
        || relay.query().is_some()
        || relay.fragment().is_some()
    {
        return Err("DITCH_RELAY_ORIGIN must be a bare HTTPS origin".into());
    }
    for candidate in [&manifest.appcast_url, &manifest.artifact_url] {
        let candidate = Url::parse(candidate)?;
        if candidate.scheme() != "https"
            || candidate.origin() != relay.origin()
            || !candidate.username().is_empty()
            || candidate.password().is_some()
            || candidate.fragment().is_some()
        {
            return Err("Commercial release URLs must use the configured Relay origin".into());
        }
    }
    let key_bytes = URL_SAFE_NO_PAD.decode(required("DITCH_RELEASE_MANIFEST_SIGNING_KEY_B64")?)?;
    let signing = SigningKey::from_slice(&key_bytes)?;
    let configured_public =
        STANDARD.decode(required("DITCH_RELEASE_MANIFEST_PUBLIC_KEY_SEC1_B64")?)?;
    let derived_public = signing.verifying_key().to_encoded_point(false);
    if derived_public.as_bytes() != configured_public {
        return Err(
            "release-manifest private key does not match the public key embedded in the app".into(),
        );
    }
    let canonical = serde_json::to_vec(&manifest)?;
    let signature: Signature = signing.sign(&canonical);
    signing.verifying_key().verify(&canonical, &signature)?;
    let release = SignedCommercialRelease {
        manifest,
        signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    };
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = output.with_extension("json.partial");
    fs::write(&temporary, serde_json::to_vec_pretty(&release)?)?;
    fs::rename(temporary, output)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{select_descriptor_revision, valid_revision};

    const ACTUAL: &str = "9b8b2a8444c053d672ccdf087bcfbd65667db7f5";
    const LEGACY: &str = "714a2b044355604d8d22cb966052eea9d10522e9";

    #[test]
    fn uses_actual_revision_for_normal_releases() {
        assert_eq!(
            select_descriptor_revision(ACTUAL.to_owned(), None).unwrap(),
            ACTUAL
        );
    }

    #[test]
    fn uses_explicit_legacy_revision_only_for_descriptor() {
        assert_eq!(
            select_descriptor_revision(ACTUAL.to_owned(), Some(LEGACY.to_owned())).unwrap(),
            LEGACY
        );
    }

    #[test]
    fn rejects_noncanonical_revisions() {
        assert!(!valid_revision(&LEGACY.to_uppercase()));
        assert!(select_descriptor_revision(ACTUAL.to_owned(), Some("abc".to_owned())).is_err());
    }
}
