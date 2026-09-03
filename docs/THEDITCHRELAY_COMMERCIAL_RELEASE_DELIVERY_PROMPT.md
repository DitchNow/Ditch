# Codex Implementation Prompt — Complete TheDitchRelay Commercial Release Delivery

> Private Commercial implementation material. Keep this document in the Commercial repository.

## Mission

Work only in this repository:

```text
/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay
```

Implement the missing Relay side of Ditch's private, entitlement-authorized Commercial macOS release system. The result must support clean staging and production Commercial-to-Commercial upgrades through Sparkle 2 and private Cloudflare R2 storage.

You may inspect the macOS repository to honor its contract:

```text
/Users/mtn/Documents/Personal/The Ditch v2/commercial
```

Do not edit the macOS repository. Do not redesign billing, Remote Control, Community/Commercial composition, or the single-`ditchd` runtime.

Implement and test the Relay changes. Do not deploy staging or production, publish a release, upload artifacts, mutate Stripe, or change live Cloudflare state unless the user explicitly authorizes that after reviewing the code.

## Why this work is required

The macOS publisher now fails closed unless Relay advertises and implements this authenticated contract:

```http
GET /v1/internal/commercial/releases/capabilities
Authorization: Bearer <internal publisher token>
```

```json
{
  "protocol_version": 1,
  "client_release_ids": true,
  "idempotent_registration": true,
  "authenticated_sparkle_sessions": true
}
```

The current Relay does not implement that endpoint. Its release code also has incompatible and security-relevant gaps:

1. `publishCommercialRelease` discards the publisher's signed `release_id` and generates a different UUID.
2. It ignores `appcast_key`, `descriptor_key`, and `minimum_build`.
3. Registration is not idempotent and cannot safely resume after a partial publication attempt.
4. `GET /v1/commercial/releases/current` returns no authenticated `update_session`.
5. Relay has no private Sparkle appcast or artifact routes matching the URLs embedded in the signed descriptor.
6. Release selection orders by publication time before build, so a later inserted older build can shadow a newer build.
7. Compatibility checks ignore `community_build`.
8. The existing one-use DMG grant cannot serve Sparkle, which needs repeated `HEAD`, `GET`, retry, and byte-range requests during one short-lived session.

The publisher creates the immutable release ID before signing the descriptor. Relay must preserve that exact ID. Otherwise the database row, signed URLs, appcast enclosure, and signed descriptor refer to different releases.

## Inspect first

Read the actual Relay implementation before editing, especially:

```text
src/releases.ts
src/worker.ts
src/types.ts
src/auth.ts
src/crypto.ts
src/billing/entitlements.ts
test/commercial.test.ts
test/support/harness.ts
migrations/0005_commercial_entitlements.sql
migrations/0006_stripe_billing.sql
wrangler.toml
wrangler.deploy.template.toml
scripts/deploy-cloudflare.mjs
docs/COMMERCIAL_BILLING.md
package.json
```

Read these macOS files as the producer/consumer contract, without editing them:

```text
/Users/mtn/Documents/Personal/The Ditch v2/commercial/scripts/macos-release
/Users/mtn/Documents/Personal/The Ditch v2/commercial/docs/MACOS_RELEASES.md
/Users/mtn/Documents/Personal/The Ditch v2/commercial/crates/ditch_commercial/src/bin/sign_commercial_release.rs
/Users/mtn/Documents/Personal/The Ditch v2/commercial/community/crates/ditch_upgrade/src/lib.rs
/Users/mtn/Documents/Personal/The Ditch v2/commercial/apps/macos/macos/Runner/AppDelegate.swift
```

Base implementation on current source, including uncommitted work. Do not revert or overwrite unrelated work.

## Architecture to preserve

```text
Signed/notarized Commercial build
  -> publisher uploads immutable DMG, appcast, descriptor to private R2
  -> publisher registers exact keys and signed release ID with Relay
  -> entitled Ditch asks Relay for the current compatible release
  -> Relay returns signed descriptor plus a short-lived update session
  -> Sparkle requests private appcast and DMG through Relay
  -> Relay authorizes and streams the exact R2 objects
  -> Sparkle verifies its Ed25519 enclosure signature
  -> Ditch verifies descriptor signature, digest, bundle ID, Team ID,
     compatibility, and rollback sequence
```

Relay is the only public download surface. R2 remains private. GitHub Releases are not involved. Stripe controls payment and entitlement only; these release routes must not call Stripe.

## 1. Add a forward-only D1 migration

Do not edit migration `0005` in place. Add the next numbered migration.

Extend `commercial_releases` to retain at least:

```text
appcast_key
descriptor_key
minimum_build
```

Retain artifact key, SHA-256, signed descriptor metadata, minimum version, channel, build, state, and timestamps. Store R2 object identity such as ETag and size where useful to detect replacement after registration.

Add a dedicated short-lived Commercial update-session table. It stores only a hash of the bearer token, never the raw token, and is scoped to:

```text
owner_id
machine_id
release_id
expires_at
created_at
```

Add indexes for token lookup and expiry cleanup. Sessions are intentionally multi-request within their TTL; they are not one-use download grants.

Keep existing download-grant routes compatible unless there is a proved safe migration path. Update the test database reset helper for the new table.

## 2. Add the authenticated capabilities endpoint

Add:

```http
GET /v1/internal/commercial/releases/capabilities
Authorization: Bearer <INTERNAL_ADMIN_SECRET>
```

Use the existing timing-safe internal authorization check and return exactly:

```json
{
  "protocol_version": 1,
  "client_release_ids": true,
  "idempotent_registration": true,
  "authenticated_sparkle_sessions": true
}
```

Missing or incorrect authorization must fail without revealing configuration. The publisher constructs this URL by appending `/capabilities` to `/v1/internal/commercial/releases`.

## 3. Repair release registration

The endpoint remains:

```http
POST /v1/internal/commercial/releases
Authorization: Bearer <INTERNAL_ADMIN_SECRET>
Content-Type: application/json
```

Accept the exact publisher body:

```json
{
  "protocol_version": 1,
  "release_id": "publisher-created-uuid",
  "version": "1.0.2",
  "build": 102,
  "channel": "beta",
  "artifact_key": "commercial/staging/beta/<release-id>/The-Ditch-Commercial-1.0.2-102.dmg",
  "appcast_key": "commercial/staging/beta/<release-id>/appcast.xml",
  "descriptor_key": "commercial/staging/beta/<release-id>/release.json",
  "sha256": "64-lowercase-hex-characters",
  "minimum_version": "1.0.0",
  "minimum_build": 100,
  "signature_metadata": {
    "manifest": {},
    "signature": "base64url-p256-signature"
  }
}
```

Requirements:

- Require `protocol_version == 1`.
- Validate and store the publisher UUID unchanged. Never generate a replacement release ID.
- Validate SemVer, positive build, non-negative minimum build, channel, digest, and safe R2 keys.
- Require all keys to use the exact environment/channel/release-ID prefix. Reject absolute paths, traversal, wrong environment, wrong channel, wrong ID, and unexpected names.
- Confirm artifact, appcast, and descriptor exist in R2 before publishing the row.
- Parse the R2 descriptor and require it to match `signature_metadata`.
- Validate descriptor fields against registration: release ID, edition, channel, version, build, release sequence, minimum version/build, artifact digest/size, canonical Relay URLs, and timestamp.
- Validate the P-256 signature over the manifest using a public environment trust value. The private signing key must never enter Relay. Use the same compact JSON serialization as the Rust signer and prove it with a cross-language fixture/test.
- Require descriptor artifact size to match R2 size. Do not buffer an arbitrarily large DMG merely to hash it; the signed macOS client verifies SHA-256 before installation.
- Capture object ETags/sizes and reject serving an object whose identity changes after registration.
- Enforce monotonically increasing build/release sequence per environment and channel after exact idempotent replay is handled.
- Insert only after all validation succeeds.

Add only the public P-256 verification value to Relay environment configuration. Staging and production may have distinct values. Make deployment validation fail closed if it is missing or malformed. Never add a release signing key to Relay.

### Idempotency

- First valid registration returns HTTP `201`.
- Repeating the exact same registration returns HTTP `201` with the same response and no duplicate rows or audit events.
- Reusing the ID with different immutable metadata or object identity returns HTTP `409` with a provider-neutral conflict error.
- Concurrent duplicate requests must preserve the same invariant; avoid an unsafe read-then-unconditional-insert race.

Success must echo the publisher ID:

```json
{
  "release_id": "publisher-created-uuid",
  "edition": "commercial",
  "version": "1.0.2",
  "build": 102,
  "channel": "beta",
  "state": "published"
}
```

## 4. Return a compatible release with a fresh session

Preserve:

```http
GET /v1/commercial/releases/current
  ?channel=stable|beta
  &community_version=<semver>
  &community_build=<non-negative-integer>
```

Requirements:

- Only an authenticated, owned, active, licensed desktop Mac may obtain a release/session.
- Validate both compatibility parameters.
- Require `minimum_version <= community_version` and `minimum_build <= community_build`.
- Select the highest compatible published build/sequence, not the most recently inserted row.
- Keep stable and beta isolated with no implicit fallback.
- Create a cryptographically random short-lived bearer for the selected exact release.
- Store only its SHA-256 hash.
- Return `update_session.expires_at` as an RFC 3339 UTC string, not epoch milliseconds.
- Rate-limit issuance and add privacy-safe audit events without logging the bearer.

Return:

```json
{
  "protocol_version": 1,
  "release": {
    "release_id": "same-publisher-created-uuid",
    "edition": "commercial",
    "version": "1.0.2",
    "build": 102,
    "channel": "beta",
    "sha256": "...",
    "minimum_version": "1.0.0",
    "minimum_build": 100,
    "published_at": 1788280000000,
    "signature_metadata": {
      "manifest": {},
      "signature": "..."
    }
  },
  "update_session": {
    "bearer": "opaque-random-token",
    "expires_at": "2026-09-01T14:30:00.000Z"
  }
}
```

`signature_metadata` remains the unmodified signed descriptor. Issue a fresh session per request: the Mac checks for notification first, then requests `current` again when Install is clicked.

## 5. Implement private Sparkle routes

Implement the exact signed URLs:

```http
GET|HEAD /v1/commercial/releases/{release_id}/appcast
GET|HEAD /v1/commercial/releases/{release_id}/artifact/{exact_filename}
Authorization: Bearer <update-session-bearer>
```

These routes run before ordinary signed-principal authentication because Sparkle sends the short-lived Authorization header.

Authorization requirements:

- Hash and compare the bearer; never query by or store raw tokens.
- Require a non-expired session for the exact release.
- Require the release to remain published.
- Confirm owner, Mac, and paid desktop activation remain valid. Expiry, revocation, or deactivation after issuance fails closed.
- Appcast authorization grants no other release/object.
- Artifact authorization requires the exact registered filename.
- Never put bearer values in query strings, redirects, logs, errors, or analytics.
- Use `Cache-Control: private, no-store`.

Sparkle transport requirements:

- Support repeated `HEAD` and `GET` within the TTL.
- Support standard single byte ranges through R2 streaming, including correct `206`, `Content-Range`, `Content-Length`, and `Accept-Ranges`.
- Preserve safe R2 metadata such as content type and ETag.
- Reject malformed/unsupported ranges safely and never load the full DMG into memory.
- Stream appcast as XML and DMG as `application/x-apple-diskimage` with a fixed safe filename.
- Require current object identity to match registration.
- Wrong, expired, or revoked authorization must not disclose object existence.

Sparkle's `httpHeaders` apply to appcast and download requests. Do not replace this with permanent public URLs, query tokens, public R2, or redirects.

## 6. Preserve staging/production isolation

The same code must work with separate bindings:

```text
staging Relay    -> ditch-commercial-artifacts-staging    -> staging D1
production Relay -> ditch-commercial-artifacts-production -> production D1
```

Never accept cross-environment descriptors, prefixes, trust keys, sessions, or release IDs. Use existing deployment generation instead of a second manual Wrangler path. Update examples, validation, generated configuration, and docs without committing secrets.

## 7. Tests

Use real D1 test behavior and fake R2, without direct Stripe access. At minimum cover:

### Capabilities/registration

1. Correct publisher bearer returns exact capabilities; missing/wrong bearer fails.
2. Valid signed descriptor and three objects register under the publisher ID.
3. Exact replay is idempotent.
4. Same ID with changed metadata/object identity conflicts.
5. Invalid protocol, UUID, SemVer, builds, channel, digest, key/prefix/traversal fails.
6. Missing artifact, appcast, or descriptor fails.
7. Descriptor mismatch/signature failure/artifact-size mismatch fails.
8. A lower build cannot supersede or shadow a higher build.

### Release/session authorization

9. Highest compatible stable and beta releases are selected independently.
10. Minimum version and build are both enforced.
11. Unlicensed, expired, revoked, wrong-owner, SSH-node, and iPhone principals cannot obtain desktop sessions.
12. Response has unchanged signed metadata, matching ID, and RFC 3339 expiry.
13. Only token hash is stored; raw bearer is absent from D1/logs.

### Sparkle transport

14. Appcast/artifact fail without a valid exact session.
15. Valid session supports repeated requests.
16. `HEAD` has correct metadata and no body.
17. Full `GET` streams exact bytes.
18. Range `GET` streams exact partial bytes and headers.
19. Wrong filename/release, expired session, or later entitlement revocation fails.
20. Changed R2 object identity fails.
21. Session for release A cannot fetch release B.

Run:

```sh
npm run check
```

Also run the focused release tests separately and report exact commands/results.

## 8. Documentation

Document publisher-created IDs, idempotent registration, private R2, signed descriptors/Sparkle enclosures, short-lived sessions, range behavior, staging/production isolation, withdrawal, and layered client verification. Include required public configuration but no real secrets or private keys.

## Known macOS integration dependency — do not work around it in Relay

At the time of this prompt, the macOS Rust client constructs `current` with:

```text
channel=stable
```

even in staging, while staging Sparkle accepts beta. This must be corrected separately in macOS before a beta staging update can complete.

Do not make Relay infer/substitute channel based on hostname or environment. Keep strict channel semantics and report this external blocker if still present.

## Non-goals

Do not:

- change Stripe, checkout, webhook, pricing, or catalog logic;
- expose permanent/unauthenticated artifact URLs or public R2;
- use one-use grants for Sparkle;
- put bearer tokens in URLs;
- generate a new release ID in Relay;
- weaken ownership or entitlement checks;
- change Ditch daemons, local/SSH behavior, Remote Control protocol, or working crypto;
- deploy or publish without explicit authorization.

## Definition of done

1. macOS preflight receives the authenticated capability response.
2. Registration preserves/echoes the signed ID and is safely idempotent.
3. Objects, metadata, compatibility, signature, and environment are validated.
4. Entitled desktop Macs receive signed release plus fresh short-lived session.
5. Sparkle can privately perform repeated HEAD/GET/retry/range requests.
6. Expired/revoked entitlement cannot authorize delivery.
7. Older builds cannot shadow newer builds.
8. Staging and production remain isolated.
9. `npm run check` passes without direct Stripe/network dependency.

## Final report

Report verified facts only:

1. Root causes fixed.
2. Files/migration changed.
3. Registration/idempotency behavior.
4. Selection/compatibility behavior.
5. Session authorization and lifetime.
6. Streaming/range behavior.
7. Environment configuration changes.
8. Tests and actual results.
9. Whether anything was deployed (expected: no unless separately authorized).
10. Remaining integration blockers, including the macOS staging-channel issue.

Do not claim production readiness, deployment, or end-to-end success without direct evidence.
