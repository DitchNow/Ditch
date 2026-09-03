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

Create `.env.release-secrets.staging` and `.env.release-secrets.production` on
the release Mac. These ignored files must be owned by the current user and
have mode `0600`; `scripts/macos-release` validates both properties before it
loads them. Keep app verification values in the ignored local
`apps/macos/config/.env.release.<environment>` files instead. Start from
`apps/macos/config/release.example`; no `.env` file is committed.

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
```

The token variables may contain values directly, but file references are
preferred so secrets are not duplicated. The release preflight loads the
Cloudflare token only from the protected file, verifies Wrangler's account and
the exact environment-specific R2 bucket, and checks Relay's release-contract
capabilities before building. `DITCH_FLUTTER` can point to a Flutter executable
when it is not on PATH.

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
the durable release descriptor, uploads immutable objects, downloads them back
to verify hashes, and registers the release with Relay last.

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
