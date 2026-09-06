# TheDitchRelay: official-build sessions and dynamic license identity

Implement the Relay side of Ditch's official-build authorization and provider-neutral license metadata. Work in the current, possibly dirty, `/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay` worktree. Preserve all existing changes and integrate with the current billing, entitlement, release, machine-identity, and error-envelope architecture. Do not reset, discard, or overwrite unrelated work.

## Important macOS constraint and intended security boundary

Native macOS apps currently cannot use `DCAppAttestService`: Apple's current `isSupported` documentation and macOS SDK header explicitly say it returns `false` on a Mac. Do **not** implement Apple App Attest, DeviceCheck, `aclBlob`, AAGUID, or an App Attest certificate-chain verifier for this release.

Use a high-entropy per-build credential injected only into official DitchNow release builds. This is a practical gate that prevents someone with only the Community source tree from using hosted Relay services. It is not hardware-backed remote attestation: a determined attacker can extract a credential from a distributed binary. Document that limitation accurately. Mitigate it with per-build rotation, hashing at rest, short-lived sessions, machine signatures, rate limits, revocation, environment separation, and audit logs. Do not describe it as cryptographic proof that the running executable is unmodified.

## Objective

- Community source builds retain all local and SSH functionality but receive `official_build_required` from protected hosted APIs.
- Official Community builds can fetch the current license and Stripe-backed offer names, and can use eligible Community updates.
- Active official Commercial Monthly and Lifetime builds retain private Commercial updates and Remote Control.
- The existing P-256 installation signature remains mandatory and identifies the machine. The new official-build session is an additional gate.
- Plan display names come dynamically from the normalized Stripe catalog, never from a hard-coded app switch.

## Existing architecture to preserve

- `src/auth.ts` verifies per-request installation/device signatures and replay nonces.
- `src/pairing.ts` registers self-generated machine keys.
- `src/billing/commercial.ts`, `catalog.ts`, and `entitlements.ts` own billing state.
- `src/releases.ts` owns Commercial releases and authorized Sparkle sessions.
- `src/worker.ts` owns routing and stable error envelopes.
- Staging and production remain strictly isolated.

## 1. Official build registry

Add a forward-only D1 migration for an `official_builds` table containing at least:

- environment;
- edition (`community` or `commercial`);
- version;
- globally monotonic build number, unique per environment;
- channel;
- SHA-256 hash of a random build credential (never store the raw credential);
- state (`enabled`, `revoked`, or equivalent);
- publication/creation timestamps;
- optional Community revision and audit metadata.

Add an `official_build_sessions` table containing a hash of a random session bearer, machine ID, official-build ID, environment, expiry, creation time, last-used time, and revocation state. Index expiry, machine, build, and state. Never log raw credentials or session bearers.

Provide the authenticated, environment-isolated `POST /v1/internal/official-builds` publisher route used by Community release CI. It accepts the registry fields above in a protocol-version-1 JSON body, including `community_revision`. Authenticate it with the Relay's existing publisher bearer convention. Make exact retries idempotent; reject any attempt to replace an existing environment/build/edition tuple with different metadata or a different credential hash. Registration is immutable except for explicit revocation/state changes.

The Commercial release publisher now adds `environment`, `edition`, and
`official_build_credential_sha256` to its existing release-registration JSON;
accept those fields and register the official build atomically with the
Commercial release. Provide the equivalent authenticated registration command
for public Community releases, whose current GitHub workflow does not call the
Commercial release publisher.

The raw build credential is unpadded base64url, at least 32 random bytes (43 characters) and at most 128 characters. Staging and production credentials must differ. Rotate it for every published build.

The current Community workflows call the endpoint at the matching Relay origin with `DITCH_OFFICIAL_BUILD_REGISTER_TOKEN_STAGING` or `DITCH_OFFICIAL_BUILD_REGISTER_TOKEN_PRODUCTION`. Document creation and least-privilege scoping of those GitHub environment secrets. The client packager injects the credential only into the signed in-process macOS runtime host; CLI and SSH runtime artifacts remain credential-free.

## 2. Session exchange contract

Add:

`POST /v1/official-build/session`

This request must carry the **existing machine-signature headers** and:

`X-Ditch-Official-Build: <raw per-build credential>`

Its canonical JSON request body is:

```json
{
  "protocol_version": 1,
  "build": 107,
  "version": "0.1.0",
  "edition": "community",
  "deployment_environment": "staging"
}
```

Requirements:

- verify the existing machine signature, timestamp, nonce, and replay rules;
- hash and timing-safely compare the build credential;
- require an enabled build matching environment, build, version, and edition exactly;
- rate-limit by machine and IP;
- issue a random, opaque, base64url session bearer with a 15-minute lifetime;
- bind the session to the machine, build, edition, and environment;
- return the existing stable envelope style with:

```json
{
  "protocol_version": 1,
  "official_build_session": {
    "bearer": "opaque high-entropy base64url token",
    "expires_at": "RFC3339 UTC timestamp",
    "build": 107,
    "version": "0.1.0"
  }
}
```

All other protected requests carry `X-Ditch-Official-Build: <short-lived session bearer>` in addition to the existing machine-signature headers. On those routes, never accept the long-lived build credential as a session.

## 3. Authorization policy

Add a reusable `requireOfficialBuild(request, env, principal)` policy. Hash and look up the bearer, require an unexpired/non-revoked session, and require exact machine, environment, and enabled-build binding. A valid official-build session never replaces machine authentication.

Protect hosted application services, including:

- Commercial bootstrap, entitlement, offers, checkout, redemption/recovery, and customer portal;
- Commercial device activation/list/deactivation and release/update-session routes;
- Remote Control pairing, confirmation, device roster, socket-ticket creation, WebSocket machine access, projections, and commands.

Keep public only health, Stripe webhooks, authenticated publisher/admin routes, website-facing checkout routes, machine registration, the official-build session exchange, and the public Community Sparkle feed/artifact.

Missing, expired, revoked, source-build, or mismatched credentials/sessions return HTTP `403` with code `official_build_required`, a user-safe non-retryable message, and request ID. Do not misreport this as an expired subscription or `action_not_allowed`.

## 4. Dynamic current-license contract

Extend the provider-neutral entitlement response while preserving legacy fields during migration:

```json
{
  "protocol_version": 1,
  "current_license": {
    "edition": "community",
    "status": "active",
    "display_name": "Ditch Community",
    "plans": []
  },
  "plan": "community",
  "status": "inactive",
  "valid_until": null,
  "billing_management_available": false,
  "renewal_available": false,
  "capabilities": {}
}
```

For paid owners, `current_license.plans` contains provider-neutral records:

```json
{
  "kind": "commercial_monthly",
  "display_name": "the current Stripe Product name",
  "billing_type": "recurring",
  "status": "active",
  "valid_until": "RFC3339 or null"
}
```

Requirements:

- Source `display_name` from verified normalized Stripe Product/catalog metadata; persist a safe snapshot for archived or renamed products.
- Never return Stripe Product, Price, Customer, Subscription, Checkout, Coupon, or PaymentIntent IDs.
- Active paid/admin sources produce `edition: commercial`; otherwise use Community.
- Return all independent base plans in deterministic order; lifetime capacity add-ons are not base plan names.
- Administrative grants use a Relay-configured display label.
- `billing_management_available` is true only when portal creation can actually succeed for this owner.
- `renewal_available` is true only when a visible eligible renewal/acquisition offer exists.

## 5. Offers and updates

- New Community owner: Monthly/Lifetime offers use `acquire`.
- Expired Monthly: Monthly uses `renew`; Lifetime may use `upgrade`.
- Active Monthly: no duplicate Monthly acquisition; Lifetime may use `upgrade`.
- Active Lifetime: only eligible capacity add-ons; no renewal.
- Refunded/revoked states use explicit policy.
- Active Monthly/Lifetime sessions may obtain compatible private Commercial releases.
- Community and inactive/expired/refunded licenses never receive private Commercial artifacts and may use the public signed Community Sparkle channel.

## 6. Rollout

Implement `disabled`, `audit`, and `enforced` modes. In audit mode accept legacy callers but record which protected requests would fail. Do not enable production enforcement until build 107 (or a later configured bridge build) is distributed. If needed, retain a narrowly scoped, auditable legacy exception that lets already activated paid machines fetch only the bridge release; it must not expose pricing, checkout, or Remote Control.

## 7. Tests

Cover:

- valid session exchange and every build/environment/edition/version mismatch;
- missing, malformed, wrong, revoked, and cross-environment build credentials;
- session expiry/revocation/cross-machine use and build revocation;
- machine signature remains required with either credential type;
- source builds receive `official_build_required` for every protected route;
- long-lived build credentials are rejected on ordinary protected routes;
- official Community can see dynamic Stripe offer names;
- Community, Monthly, Lifetime, expired, administrative, refunded, and add-on license combinations;
- exact portal and renewal availability;
- Commercial versus Community update eligibility;
- disabled/audit/enforced modes and the bridge-release limitation;
- no raw credentials/bearers/provider IDs in storage, logs, or responses.

Run the complete Relay verification suite. Report migrations, configuration, registration command, exact contracts, protected/public routes, rollout state, tests, and the explicit extractability limitation of macOS build credentials.
