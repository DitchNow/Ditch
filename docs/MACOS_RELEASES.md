# Commercial macOS release runbook

Commercial releases use Sparkle 2 for verified replacement, Cloudflare R2 for
private immutable storage, and Ditch Relay for entitlement-aware release
authorization. GitHub Releases are not involved.

## One-time setup

1. Create separate staging and production Sparkle Ed25519 signing keys in the
   release Mac's Keychain (`generate_keys`). Put only the matching public key
   in each environment's official app.
2. Create and protect separate staging and production P-256
   release-descriptor signing keys. Put only the matching SEC1 public key in
   each environment's official app.
3. Configure a Developer ID Application identity and a `notarytool` Keychain
   profile.
4. Create separate staging and production R2 buckets (or strictly separated
   prefixes and credentials).
5. Configure Relay's private publisher endpoint to accept the generated
   registration contract, read the R2 objects, verify the signed descriptor,
   and issue short-lived appcast/artifact sessions only after entitlement
   checks.
6. Generate a unique per-build official-build credential with at least 32
   random bytes, register only its SHA-256 hash and immutable build metadata
   with the matching Relay environment, and store the raw value in a mode-0600
   file outside Git.

No Stripe key, webhook secret, Cloudflare token, Apple private key, or release
signing private key is embedded in Ditch.

## Relay release contract

The publisher preflight requires this authenticated endpoint before any build
or upload begins:

```text
GET /v1/internal/commercial/releases/capabilities

{
  "protocol_version": 1,
  "client_release_ids": true,
  "idempotent_registration": true,
  "authenticated_sparkle_sessions": true
}
```

Registration must preserve and echo the publisher's immutable `release_id`;
it must never replace it with a Relay-generated ID. Repeating an identical
registration is successful and repeating the same ID with different metadata
is rejected. Relay stores the artifact, appcast and descriptor object keys.

An entitled `current` request returns the signed descriptor plus a short-lived
update session. That bearer authorizes the descriptor's exact appcast and
artifact URLs. Appcast and artifact routes are not public, the artifact route
supports Sparkle's `HEAD`, `GET` and range/retry behavior for the session TTL,
and Relay remains authoritative for entitlement at session creation.

The macOS client binds the Relay-authorized release ID, appcast URL, artifact
URL, byte length, version, build and channel to Sparkle's selected item. The
bearer is sent only to that appcast and exact artifact request; external
release-note downloads are disabled. Relay validates the appcast against the
signed descriptor, and Sparkle performs the final Ed25519 verification of the
downloaded archive before installation.

## Release environment

The repository root is the only application and release workspace. Create
`.env.release-secrets.staging` and `.env.release-secrets.production` there.
These ignored files must be owned by the current user and have mode `0600`;
`scripts/macos-release` validates both properties before it loads them. Keep
public app and release verification values only in
`apps/macos/config/.env.<environment>` and
`apps/macos/config/.env.release.<environment>`. Start from the examples in that
directory.

Private signing keys and per-build official credentials belong outside every
checkout under:

```text
~/Library/Application Support/DitchNow/ReleaseKeys/staging/
~/Library/Application Support/DitchNow/ReleaseKeys/production/
```

Release outputs, immutable publication inputs, and receipts are written only
under `dist/commercial/`. The obsolete root `staging.sh` and `production.sh`
wrappers are unsupported; invoke `scripts/macos-release` directly.

Staging and production must use different Sparkle Keychain accounts and
different Sparkle/P-256 public keys. The release machine's public trust files
remain local even though their values are not secrets. `scripts/macos-app` and
`scripts/macos-release` fail closed when the trust roots are reused across
environments.

```text
DITCH_CODESIGN_IDENTITY
DITCH_NOTARY_PROFILE
DITCH_NOTARY_KEYCHAIN
DITCH_RELEASE_MANIFEST_SIGNING_KEY_FILE
DITCH_R2_BUCKET
DITCH_RELEASE_REGISTER_URL
DITCH_RELEASE_REGISTER_TOKEN_FILE
DITCH_CLOUDFLARE_API_TOKEN_FILE
DITCH_OFFICIAL_BUILD_CREDENTIAL_FILE
```

The token variables may contain values directly, but file references are
preferred so secrets are not duplicated. The release preflight loads the
Cloudflare token only from the protected file, verifies Wrangler's account and
the exact environment-specific R2 bucket, and checks Relay's release-contract
capabilities before building. `DITCH_FLUTTER` can point to a Flutter executable
when it is not on PATH.

The official-build credential is injected only into the signed in-process
macOS runtime host and exchanged for short-lived, machine-bound Relay sessions.
The standalone CLI and SSH runtime artifacts are built without it. It keeps
ordinary Community source builds out of hosted services, but it is not
hardware-backed remote attestation and can be extracted by a determined
reverse engineer. Rotate and revoke it per build; never reuse it across staging
and production.

## Staging

Use a positive, globally increasing build number. Both staging and production
require a clean Commercial worktree and a clean, exact Community pin. There is
no dirty-release override. Staging accepts the beta Sparkle channel; `stable`
remains the default channel.

```sh
scripts/macos-release staging all 1.2.0 120 1.0.0 1 beta
```

Run the non-mutating release preflight before the full build:

```sh
scripts/macos-release staging preflight 1.2.0 120 1.0.0 1 beta
```

The command tests both Community and Commercial, builds the single-runtime
app, signs/notarizes/staples a DMG, generates an Ed25519-signed appcast, signs
the durable release descriptor, registers the official build, uploads and
downloads immutable objects to verify their hashes, and finally registers the
Commercial release. The receipt records both Relay registrations without
recording the raw credential.

### One-time build 108 bridge

Installed build 108 compares the signed descriptor's Community revision for
exact equality. Build 109 therefore needs one explicit compatibility bridge:

```sh
scripts/macos-release staging all \
  0.1.0 109 0.1.0 108 beta \
  --legacy-source-revision 714a2b044355604d8d22cb966052eea9d10522e9
```

This flag changes only the signed v1 descriptor field consumed by the source
client. The rebuilt app embeds the actual `COMMUNITY_REVISION`, and the
official-build registration sends that same actual target revision. Normal
releases omit the flag. Production additionally requires
`--confirm-production-legacy-bridge` so an inherited bridge cannot be
published accidentally.

Test on a disposable Mac/user profile: purchase with the Stripe test context,
wait for Relay entitlement, upgrade Community, create local and SSH sessions,
and confirm the bell offers the next Commercial update without interrupting an
active agent.

## Production

Production enforces the same clean-tree rule and uses the production Relay
configuration.

```sh
scripts/macos-release production all 1.2.0 120 1.0.0 1 stable
```

Do not copy staging Stripe, Relay, R2, signing, or registration credentials into
production. Publication is complete only when Relay returns HTTP 201; uploaded
but unregistered immutable objects are harmless orphans and are never treated
as a release.

## Updating paid installations

Monthly and lifetime installations request sanitized entitlement and a fresh
authorized release from Relay. A newer compatible release appears in Ditch's
notification bell. Clicking **Install** requests a fresh short-lived download
session and starts Sparkle. Ditch defers while agents are active. Expired
subscriptions keep all Community functionality and data but cannot obtain paid
artifact authorization until renewed.
