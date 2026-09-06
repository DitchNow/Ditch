# The Ditch: official-build authorization, private Community updates, and license-aware upgrades

Implement the macOS/client/release-pipeline half of Ditch's official-build authorization and private update delivery. Work in the current, possibly dirty, `/Users/mtn/Documents/Personal/The Ditch v2` worktree. Preserve every existing change, inspect before editing, and integrate with the current Community submodule, Commercial overlay, Rust runtime, Flutter UI, AppKit/Sparkle updater, signing/notarization, and release scripts. Do not reset, replace, or discard unrelated work.

This prompt is intentionally implementation-specific to the repository as it exists now. Complete and harden the partial official-build work already present; do not layer a second competing design on top of it.

## Product outcome

Deliver these exact behaviors:

1. Anyone may continue to clone the public Community source, build it, and use all local and local/SSH Community functionality.
2. A Community or Commercial macOS build produced by DitchNow's private release process can establish a short-lived official-build session with Ditch Relay.
3. A third-party/source build cannot use Relay-hosted application services or discover Relay's Community or Commercial release catalog. Protected operations fail locally when possible and fail closed at Relay with `official_build_required`.
4. An official Community installation can discover and install a newer official Community release through Relay without buying a license.
5. An official installation with a usable paid or administrative Commercial entitlement continues to use the existing private Commercial update path.
6. An official installation whose paid entitlement is expired, refunded, revoked, or otherwise unusable falls back to the private Community update path. It must never be forced to lose projects, sessions, settings, SSH configuration, Keychain identity, or local Community functionality.
7. The current license name shown by the app comes from Relay's provider-neutral `current_license` response. Do not hard-code marketing plan names such as `Commercial Monthly` or `Commercial Lifetime` as the current license.
8. Initial/manual Community downloads may remain available from the public Ditch website. In-app Community release discovery, appcasts, and artifacts are private Relay resources after enforcement.

This protects access to Relay and its private release metadata. It cannot make a version number secret if the same number is independently published in Git tags, source files, release notes, the website, or an official application's UI.

## Critical platform fact: do not implement Apple App Attest on macOS

Native macOS applications cannot use `DCAppAttestService` for this purpose. Apple documents that `DCAppAttestService.isSupported` is always `false` in an app running on a Mac, including Mac Catalyst and iOS/iPadOS apps on Apple silicon:

<https://developer.apple.com/documentation/devicecheck/dcappattestservice/issupported>

The installed Xcode macOS SDK repeats this explicitly in `DeviceCheck.framework/Headers/DCAppAttestService.h`. A local probe on the current development Mac also reports both `DCAppAttestService.shared.isSupported` and `DCDevice.current.isSupported` as false outside a provisioned app context. DeviceCheck is not a substitute for per-request App Attest assertions and must not be presented as proof of an unmodified Developer-ID application.

Therefore:

- remove the unfinished `DCAppAttestService`/challenge/assertion client path from `community/crates/ditch_upgrade/src/lib.rs`;
- do not add App Attest entitlements, `DeviceCheck`, AAGUID/`aclBlob` logic, Apple attestation keys, or Apple Developer portal App Attest setup;
- do not build an XPC or Unix-socket "attestation helper";
- do not claim that Developer ID signing can be remotely verified by Relay merely because the local process can inspect its own code signature.

Use the practical official-build credential design below. Document its limitation honestly: a determined attacker who obtains an official distributed binary can extract an embedded credential. This mechanism prevents someone with only the public source tree from accessing protected Relay APIs, but it is not hardware-backed remote attestation and does not prove that a running binary is unmodified.

Possession of an official Community DMG necessarily lets someone run that official build, and may let a determined person extract or proxy through its credential-bearing runtime. Preventing that is outside what a native Developer-ID macOS app can reliably enforce with the available Apple APIs. The enforceable target here is narrower and explicit: the public source tree and ordinary third-party builds contain no valid Relay credential. Do not claim stronger anti-tamper or anti-reuse guarantees.

## Security model

Keep three independent decisions separate:

- the existing P-256 installation identity and `DITCH1` request signature identify and authenticate a machine;
- a private, random, per-build credential proves that the caller possesses material injected by DitchNow's release process and lets it obtain a short-lived official-build session;
- the reconciled entitlement decides which paid capabilities and Commercial artifacts that official installation may use.

The official-build session never replaces the installation signature. A Community official build needs no paid entitlement for private Community updates. A Commercial entitlement never makes an unrecognized source build official.

Do not use a credential committed to Git, a credential shared by every release, the Sparkle private key, the release-manifest private key, the Developer ID private key, a Team ID, bundle ID, code-signing requirement, User-Agent, edition string, build number, static signed JSON file, or a public verification key as the official-build secret.

Use a fresh cryptographically random credential for every `(deployment_environment, build, edition)` official release. The raw value is unpadded base64url encoding of at least 32 random bytes, 43-128 ASCII characters. Staging and production credentials must be different. Relay stores only `SHA-256(raw ASCII credential)` and issues a separate opaque session bearer with a recommended 15-minute lifetime.

Mitigate the known extraction limitation with per-build rotation, short-lived sessions, exact machine/build/environment binding, rate limits, audit logs, explicit build/session revocation, no redirects on credential-bearing requests, and a bridge rollout that can be removed.

## Current architecture and partial implementation to preserve

Verify these facts against the live tree before changing them:

- The root workspace is the proprietary Commercial composition. It pins the public `community/` repository and reuses its crates and macOS UI. `COMMUNITY_REVISION` and `scripts/verify-community-pin` enforce that relationship.
- The public `ditch_upgrade` crate already owns installation-signed Relay requests, entitlement/offer models, Commercial release verification, and the beginnings of an official-build session cache.
- `community/crates/ditch_upgrade/src/lib.rs` currently contains an incomplete App Attest FFI bridge (`ditch_set_app_attest_callback`), App Attest challenge/register/session calls, and a global bearer cache. Replace this partial path; do not leave dead or selectable App Attest behavior.
- `community/crates/ditch_identity/src/lib.rs` owns the P-256 installation identity. The local macOS identity is stored in the login Keychain under `ai.theditch.installation-identity.v1`; SSH runtimes use a mode-0600 file. Preserve identity continuity and the legacy-file migration.
- The single local runtime is statically linked into `The Ditch Runtime.app` (`ai.theditch.runtime`) by `apps/macos/macos/package_status_host.sh` and its Community counterpart. The helper launches `ditch_runtime_run()` in-process. There is no general XPC attestation service.
- The notification-control Unix socket in `StatusHost.swift` is unrelated to hosted authorization. Never expose a credential, bearer, arbitrary signing primitive, or Relay proxy through it.
- `crates/ditch_commercial/src/lib.rs` and `crates/ditchd/src/remote_control.rs` already add a cached `X-Ditch-Official-Build` bearer to some Commercial/Remote requests. Preserve and complete this integration.
- `apps/macos/macos/Runner/AppDelegate.swift` already has a carefully constrained authenticated Commercial Sparkle flow. It validates the environment/host/path/release ID/item metadata, adds the update bearer only to the authorized appcast and exact artifact, and clears it afterward.
- The same AppDelegate still has a separate public Community path using `https://updates.ditchnow.nl/community/appcast.xml`. Replace that public in-app path with Relay-authorized Community update sessions.
- Both root and Community `Runner/Info.plist` files still contain static `SUFeedURL` values. Static public Community/Commercial feeds must not be the post-migration authority.
- `community/apps/macos/lib/main.dart` and runtime transport currently distinguish `checkCommunityUpdate` from the authorized Commercial flow. Generalize the authorized flow instead of duplicating security-sensitive URL/header logic.
- `scripts/macos-release` is the hardened Commercial build/sign/notarize/upload/register pipeline. `community/scripts/package-community-dmg` only packages an already-built Community app and is not yet an equivalent private Community publication pipeline.
- `apps/macos/macos/package_status_host.sh` currently builds `ditchd`, `libditchd.a`, `ditch_cli`, and remote-runtime resources in one process. This is a critical credential-leak boundary: the official-build credential must be present only in the local macOS runtime host and must never enter standalone `ditchd`, `ditch_cli`, or SSH remote artifacts.
- The worktree contains duplicated/mirrored root and `community/` macOS files. Identify the canonical source for each change and intentionally synchronize only the required mirrors. Do not blindly edit generated build output or the untracked `commercial/` mirror.

## Contract dependency and mismatch gate

Before implementation, compare this prompt with the Relay implementation or the Relay-side official-build prompt copied into the repository. The required Relay design is:

- private per-build credential exchange at `POST /v1/official-build/session`;
- short-lived official-build bearers on protected machine requests;
- private Community release discovery/appcast/artifact routes;
- the existing private Commercial release flow;
- dynamic `current_license` and offer-action availability fields.

If the Relay contract still specifies Apple App Attest endpoints such as `/v1/official-build/attestation/challenge`, do not implement both designs and do not invent compatibility aliases. Record the conflict and update the Relay contract first. Apple App Attest cannot be the native macOS production gate.

Keep route names and JSON fields synchronized through checked-in cross-repository fixtures. If the live Relay has already chosen different canonical names, update this prompt/fixture and the client together rather than silently supporting two contracts.

## 1. Replace the partial App Attest path with a build-credential session exchange

Refactor `community/crates/ditch_upgrade/src/lib.rs` so official-build authorization works as follows.

### Credential injection boundary

Add a narrow Rust FFI entry point for the in-process signed macOS runtime host, for example:

```text
ditch_set_official_build_credential(bytes, length) -> status
```

Requirements:

- accept the credential exactly once before `ditch_runtime_run()` starts;
- copy it into a redacted/zeroizing Rust-owned value;
- reject empty, malformed, oversized, repeated, or late configuration;
- never expose the raw value through a getter, runtime socket response, logs, errors, panic text, metrics, crash annotations, Dart, preferences, SQLite, Keychain, environment plist, or command-line arguments;
- keep `cached_official_build_bearer()` limited to the in-process Commercial/Remote composition; it returns only the short-lived session bearer, never the raw build credential;
- source/debug/headless runtimes in which the setter is never called are explicitly unofficial.

Do not pass the credential through a public IPC service. The Swift-to-Rust call is direct inside the linked runtime process. The existing notification-control socket must remain unable to read it or request authenticated Relay calls.

### Session request

After the installation identity has been loaded and `/v1/machines/register` has succeeded, obtain an official-build session with:

```http
POST /v1/official-build/session
X-Ditch-Official-Build: <raw per-build credential>
X-Ditch-Protocol: 1
X-Ditch-Principal-Type: machine
X-Ditch-Principal-Id: <installation UUID>
X-Ditch-Timestamp: <milliseconds>
X-Ditch-Nonce: <base64url>
X-Ditch-Signature: <installation signature>
Content-Type: application/json
```

Sign and send the exact UTF-8 JSON body bytes:

```json
{
  "protocol_version": 1,
  "build": 107,
  "version": "0.1.0",
  "edition": "community",
  "deployment_environment": "staging"
}
```

Derive `version`, `build`, `edition`, and deployment environment from the exact local runtime build configuration. Validate them before any network request. The Relay treats them only as values bound to the secret's registered build record; the credential is the authority.

Send the raw build credential only to the exact configured Relay origin and exact `/v1/official-build/session` path. Use HTTPS, reject credentials/userinfo/ports/query/fragment, disable redirects, and never forward the header cross-origin. Ordinary protected routes must never receive the raw build credential.

Strictly parse this response shape:

```json
{
  "protocol_version": 1,
  "official_build_session": {
    "bearer": "opaque high-entropy base64url token",
    "expires_at": "RFC3339 UTC timestamp",
    "build": 107,
    "version": "0.1.0",
    "edition": "community",
    "channel": "beta"
  }
}
```

Require protocol, build, version, edition, environment-implied channel, bearer syntax, and a bounded future expiry to match the local build. Do not cache an inconsistent response.

Cache only the session bearer in memory with a safety margin. Key the cache by machine/build/edition/environment, make refresh single-flight under concurrency, and clear it on expiry, explicit authorization rejection, runtime shutdown, or build mismatch. One rejected/expired session may cause one bounded refresh and retry; do not recurse or retry a revoked/unknown build indefinitely.

All protected machine requests add the short-lived session as `X-Ditch-Official-Build` in addition to the normal `DITCH1` machine-signature headers. Continue to sign the exact method, path/query, body digest, timestamp, nonce, and principal as the existing code requires.

### Source-build behavior

When no release-injected credential exists:

- local and SSH features continue normally;
- hosted license, offer, checkout, pairing, Remote Control, Community release, and Commercial release actions return a stable local `official_build_required` error before sending protected network traffic where practical;
- server enforcement remains mandatory even though the UI/client fails early;
- do not expose a setting, environment variable, CLI flag, plist field, or runtime socket command that lets an installed source build supply a credential at runtime.

Tests may inject a credential through a test-only constructor/hook that cannot compile into production behavior accidentally.

## 2. Inject the credential only into the local macOS runtime host

Extend the root and canonical Community macOS packaging paths without committing the raw credential.

The recommended shape is a generated Swift source file in a dedicated mode-0700 build-temporary directory, with the file itself mode `0600`, that contains the release credential and invokes the narrow Rust setter immediately before `ditch_runtime_run()`. Generate and compile this file only for an official Release archive. Remove the source and credential-bearing compiler intermediates on exit. Do not place it in `Contents/Resources`, `DitchEnvironment.plist`, `Info.plist`, an xcconfig, Dart defines, the main Flutter Runner executable, a shell command argument, or a committed/generated file under the source tree.

An equally narrow implementation is acceptable if it satisfies all of these gates:

- official Release builds require `DITCH_OFFICIAL_BUILD_CREDENTIAL_FILE`, owned by the current user and mode `0600`, whose contents pass the base64url/entropy-length validation;
- the file path may appear in private local release settings, but the raw value may not;
- ad-hoc, Debug, ordinary `flutter build`, and public-source builds compile with no credential and remain unofficial;
- a credential-bearing build fails unless it is using the configured Developer ID Application identity and the expected edition/environment/version/build;
- the credential is embedded only in `The Ditch Runtime.app/Contents/MacOS/ditchd`, which contains the local statically linked runtime;
- the standalone `ditchd` executable, `ditch_cli`, `ditchd-remote-*`, and every cross-built Linux/macOS SSH runtime are built in a credential-free build context and verified not to contain the raw credential or its digest;
- Cargo target reuse cannot accidentally carry a credential from an official build into a later source/SSH artifact. Use separate target directories or explicit clean-room outputs for credential-bearing versus credential-free builds;
- the raw credential is never printed, including with `set -x`, diagnostics, verification failures, CI masking misses, `strings`, or diff output.

The current `package_status_host.sh` builds the Rust binary, static library, CLI, and remote resource together. Split those build contexts before adding credential injection. This is non-negotiable.

Update the nested runtime app's generated `Info.plist`: `CFBundleShortVersionString` and `CFBundleVersion` must equal the actual Ditch release version/build rather than the current hard-coded `1.0.0` and `1`. Keep `CFBundleIdentifier=ai.theditch.runtime`. Verify the main app remains `ai.theditch.app` and both Community/Commercial replacements preserve the same application data locations.

Developer ID signing, Hardened Runtime, notarization, stapling, and Sparkle Ed25519 signatures remain mandatory distribution integrity controls. They are not Relay attestation. No new Apple App Attest capability or portal configuration is required.

## 3. Add private Community release discovery to the Rust/runtime contract

Implement the client half of this route family unless the checked-in Relay fixture proves that final names differ:

- `GET /v1/community/releases/current` — machine-signed plus official-build session;
- `GET|HEAD /v1/community/releases/{release_id}/appcast` — release-session bearer;
- `GET|HEAD /v1/community/releases/{release_id}/artifact/{filename}` — release-session bearer.

The discovery response must contain strongly typed, bounded release metadata plus a short-lived `update_session`. Reuse the security properties of `SignedCommercialRelease` and `CommercialUpdateSession`, but choose neutral types such as `AuthorizedRelease`/`AuthorizedUpdateSession` where doing so removes duplication without weakening Commercial verification.

Validate at least:

- immutable UUID release ID;
- `edition == community` for the Community route;
- exact environment channel (`beta` for staging, `stable` for production);
- canonical bundle ID `ai.theditch.app` and configured Apple Team ID;
- semantic version syntax and a positive numeric build/release sequence;
- `candidate.build > current official-build session build`; never trust a UI-supplied current build;
- artifact HTTPS origin, exact Relay host, exact release-scoped appcast path, exact release-scoped artifact prefix, no credentials/port/query/fragment;
- artifact digest, size, filename, and any signed manifest metadata used by the existing release verifier;
- update bearer syntax and bounded future expiry.

Add versioned runtime requests for checking and obtaining a Community release, parallel to `CheckCommercialRelease` and `CurrentCommercialRelease`. Keep the non-installing check free of side effects beyond Relay's update-session issuance. Return an explicit no-update/unavailable result consistently; do not turn "already current" into an alarming error.

Preserve the current active-session safety rule: never replace the application/runtime while agents are active. A deferred Community or Commercial install must retain the selected release only as long as its authorization remains valid, otherwise request a fresh release/update session before retrying.

## 4. Generalize the AppKit/Sparkle authorized updater

Refactor `apps/macos/macos/Runner/AppDelegate.swift` and its canonical Community counterpart so both editions use one authenticated update entry point, for example `installAuthorizedUpdate` with an explicit validated `edition`.

Preserve and extend the existing `AuthorizedUpdateContext` protections:

- accept only `community` or `commercial`;
- require the exact environment channel;
- require lowercase canonical UUID release ID;
- validate HTTPS URL structure and the exact configured Relay host;
- require `/v1/{edition}/releases/{release_id}/appcast` exactly;
- require `/v1/{edition}/releases/{release_id}/artifact/{filename}` with a single safe filename segment;
- reject percent-encoded path confusion, `..`, empty filenames, credentials, ports, queries, fragments, unexpected hosts, and malformed bearer/expiry/build/size values;
- require the Sparkle item to exactly match artifact URL, content length, display version, build/version string, and channel;
- set `Authorization: Bearer <update-session>` only for the exact authorized appcast and exact artifact request;
- clear headers after the appcast is authenticated, re-add them only to the exact artifact request, and clear all state on completion, abort, timeout, mismatch, or app termination;
- refuse redirects rather than risk forwarding the bearer;
- continue to rely on Sparkle's Ed25519 signature and signed-feed verification before extraction/installation;
- never persist or log the update bearer.

Remove `communityUpdateRequested`, `communityUpdateFeedURL`, the unauthenticated `checkCommunityUpdate` path, and the public-feed-specific delegate branches after the private path is operational. Remove static `SUFeedURL` values or ensure the updater delegate can never fall back to them. Clear any legacy Sparkle feed URL cached in user defaults during migration.

Do not accept arbitrary URLs from Dart merely because the host appears in `AllowedUpdateHosts`; the route family, release ID, edition, and artifact URL must match exactly. Prefer the Relay host only. Keep release notes unauthenticated only if they are an independently public fixed URL and never receive a bearer; otherwise disable them as today.

Add native tests for Community and Commercial authorized contexts, every URL/path confusion case, metadata mismatch, header scoping, cleanup, expired sessions, concurrent update attempts, and the absence of a public fallback feed.

## 5. Make update selection license-aware in Rust and Flutter

Use Relay's canonical `current_license`, not the currently installed binary edition alone:

- usable active Monthly, Lifetime, or administrative Commercial authority: check the compatible private Commercial release path;
- canonical Community license, including expired/refunded/revoked/inactive paid sources: check the private Community release path without requiring payment;
- source build/no official credential: show the manual official-download action and do not query a private release route;
- invalid machine authentication remains an authentication error; missing official-build authorization maps to `official_build_required`; inactive payment must not be mislabeled as a source build.

The automatic product update check in `community/apps/macos/lib/main.dart` currently runs only when `runtimeStatus.edition == commercial`. Change it so every official build can check the correct license-selected channel. The Upgrade dialog's Community branch must call the new runtime Community-release request and invoke the same authorized native Sparkle entry point as Commercial.

Use one Dart parser for authorized release arguments. Require explicit edition, release ID, appcast/artifact identity, version, build, channel, size, bearer, and expiry. Do not keep a Community parser that bypasses Relay authorization.

When an installed Commercial build loses its paid entitlement, do not downgrade it silently. Install a Community release only when its globally monotonic build is newer than the installed Commercial build. Release operations must ensure a newer Community fallback build will eventually exist; until then the app remains locally usable and reports that no newer Community build is available.

Existing Community users who installed a pre-gate build cannot be distinguished from people who built the same public source. Do not add a public Relay bypass for them. Provide a stable public website/download action that lets them manually install the first official bridge Community build. The bridge keeps bundle ID, Application Support, preferences, Keychain installation identity, projects, sessions, and SSH configuration. After that official build starts, it can establish a session and receive later private Community updates.

## 6. Finish the dynamic current-license UI contract

The Rust and Dart models already partially support `current_license`. Complete the contract while preserving legacy fields during rollout.

Canonical entitlement fields include:

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
  "acquisition_available": true,
  "renewal_available": false,
  "upgrade_available": false,
  "capabilities": {}
}
```

Requirements:

- add and strictly parse `acquisition_available` and `upgrade_available` alongside the existing billing-management and renewal fields;
- render `current_license.display_name` and its provider-neutral `plans[*].display_name` for the current license;
- never infer a current marketing name from enum values such as `commercial_monthly` or `commercial_lifetime`;
- use offer `title` only for an offer the user may buy, not as retroactive identity for the current license;
- tolerate the documented legacy top-level status semantics while treating `current_license` as what the app can use now;
- never receive, persist, or display Stripe Product, Price, Customer, Subscription, Checkout, PaymentIntent, Charge, or other provider IDs;
- show Manage Billing, Acquire, Renew, and Upgrade only when the corresponding boolean and a compatible visible action make it usable;
- do not expose Monthly-to-Lifetime upgrade unless Relay explicitly returns an eligible `upgrade` offer backed by the completed anti-double-billing policy.

Update Rust serde tests, Dart strict-parser tests, widgets, empty/error/loading states, and user-facing messages. Preserve safe provider-neutral fallbacks such as `Ditch Commercial` only for legacy/missing verified metadata; do not reintroduce hard-coded Monthly/Lifetime current-license labels.

## 7. Preserve and complete Commercial and Remote Control authorization

Do not rewrite the working Commercial release verifier or weaken its P-256 signed manifest, Sparkle signature, immutable release ID, artifact digest/size, minimum Community compatibility, Apple Team/bundle identity, and short-lived update-session checks.

Complete official-build bearer propagation through:

- `community/crates/ditch_upgrade` Commercial bootstrap, entitlement, offers, checkout, redemption, customer portal, activation, and release lookup;
- `crates/ditch_commercial/src/lib.rs` calls that use Relay directly;
- `crates/ditchd/src/remote_control.rs` pairing, enrollment, device/roster, socket-ticket, and other protected desktop requests;
- reconnect/refresh paths, so a stale bearer cannot create an unauthenticated WebSocket or silently disable Remote Control.

The bearer must stay inside the local runtime process and must not cross Flutter IPC, CLI output, SSH transport, or the mobile protocol. iOS continues to use its existing device authentication. SSH remote nodes never receive an official-build credential or desktop session and cannot sponsor their own enrollment.

Map Relay's `official_build_required` consistently through runtime error envelopes and Flutter messages. Keep network errors retryable where appropriate, but make a revoked/unknown source build actionable: install an official Ditch build from the public website. Do not imply that buying a license will make a source build official.

## 8. Build and publication pipelines

### Shared official-build registration

Add a release helper used by both Community and Commercial publishers that:

1. reads the private credential file without printing it;
2. validates base64url syntax and decoded entropy length;
3. computes lowercase hex `SHA-256` of the raw ASCII credential;
4. calls the authenticated internal official-build capabilities/registration route with environment, edition, version, globally monotonic build, channel, Community revision, and credential hash;
5. treats exact idempotent replay as success and conflicting reuse as failure;
6. records only a non-secret build ID/receipt in `dist/`;
7. ensures the official-build record is enabled before making the corresponding artifact downloadable.

Never send the raw credential to the publisher endpoint if the Relay contract accepts its hash. The raw value goes only into the official local runtime binary and later to the public session-exchange endpoint over HTTPS.

### Community release pipeline

Create a Community release pipeline parallel in rigor to `scripts/macos-release`, rather than extending `package-community-dmg` into an implicit publisher. It must:

- verify the correct Community source revision and a clean release input tree;
- require a globally monotonic positive build and `staging/beta` or `production/stable` pairing;
- run Community Rust/Flutter tests, analysis, leakage checks, and source-build-without-secret tests;
- build the credential-bearing local runtime separately from all credential-free CLI/SSH artifacts;
- sign nested code in the correct order with Developer ID Application, enable Hardened Runtime, notarize, staple, and assess the final DMG;
- generate a signed Sparkle appcast whose enclosure points to the immutable Relay Community artifact route;
- validate app bundle ID, runtime bundle ID, Team ID, version, build, edition, channel, Community revision, artifact digest/size, and Sparkle signature;
- upload appcast/artifact to a private environment-specific R2 prefix without overwriting different bytes;
- idempotently register the official build and Community release through authenticated internal routes;
- verify the private object and Relay registration before publication;
- never put the Community appcast/artifact on the old public update host after enforcement.

### Commercial release pipeline

Extend `scripts/macos-release` without regressing its current preflight/build/verify/publish behavior:

- require a different per-build credential for the Commercial build;
- register the official Commercial build in addition to the existing Commercial release;
- inject the credential only into the local runtime host;
- keep remote artifacts credential-free;
- preserve the existing private Commercial appcast/artifact routes and publisher capability checks;
- verify Community and Commercial build-sequence compatibility and global monotonicity.

Private release settings may contain paths to credential/token/key files. Raw build credentials, publisher tokens, Cloudflare tokens, Sparkle private keys, release-manifest private keys, notarization credentials, and Developer ID private material must remain outside Git and outside committed examples. Public Sparkle/manifest verification keys, Team ID, bundle IDs, and route origins are not secrets.

Update `.gitignore`, secret-file permission checks, CI masking, and `community/scripts/check-community-leakage` so they reject raw credential files and accidental credential propagation without rejecting the public authorization implementation itself. A source-only Community checkout must build and test without any private file.

## 9. Rollout and compatibility

Match Relay's `disabled`, `audit`, and `enforced` rollout modes:

- `disabled`: current protected behavior may remain available while the new client exchange can be tested;
- `audit`: official clients obtain/send sessions, Relay records legacy calls that would fail, and newly private Community release routes already require authorization;
- `enforced`: all protected desktop operations and Community/Commercial discovery require a valid official-build session.

Do not enable production enforcement until all of these are true:

- Relay supports the final build-credential and private Community contracts;
- one real Developer-ID-signed/notarized Community bridge build and one Commercial build can obtain sessions in staging;
- Community and Commercial private Sparkle downloads pass end to end;
- source/debug/CLI/SSH artifacts contain no credential and receive `official_build_required`;
- existing Commercial bridge behavior has been exercised or an explicit migration decision has been made;
- the public manual Community bridge download and user guidance exist;
- build/session revocation has been tested.

Keep any legacy Commercial bridge exception narrowly scoped to an already activated paid machine and one configured release. The client must not rely on it for later updates, pricing, checkout, or Remote Control. There is no equivalent automatic legacy exception for Community because old official and source-built Community binaries are indistinguishable.

## 10. Required tests and verification

Add deterministic unit, integration, packaging, and staging acceptance coverage.

### Rust/client tests

- no credential means no official session and no protected release/catalog request;
- valid credential session request uses exact body bytes and existing machine signature;
- the raw credential appears only on the exact session endpoint;
- redirects, alternate origins, ports, queries, fragments, and cross-origin forwarding are rejected;
- response protocol/build/version/edition/channel/expiry mismatches fail closed;
- session caching, safety margin, single-flight concurrency, one bounded refresh, revocation, and shutdown clearing;
- raw credential and bearer redaction/zeroization/no serialization;
- Community release parsing and `candidate.build > caller.build`;
- Commercial release verification remains intact;
- active paid, Community, expired, refunded, revoked, and administrative update-channel selection;
- runtime error-code mapping for official-build, release unavailable, entitlement, network, verification, and deferred-install cases;
- no credential in CLI or SSH runtime processes.

### Swift/Sparkle tests

- credential injection happens before runtime start and is never available over notification-control IPC;
- Community and Commercial use the same strict authorized updater;
- exact route/release/artifact matching and percent-encoding/path traversal defenses;
- bearer only on the authorized appcast and exact artifact, never on redirects or release-note URLs;
- signed item metadata, size, version, build, and channel mismatch rejection;
- state/header cleanup after success, abort, error, timeout, and concurrent invocation;
- no `SUFeedURL` or cached/default public-feed fallback.

### Release/packaging tests

- source and Debug builds succeed without a credential and are classified unofficial;
- official Release build fails for missing/malformed/weak/wrong-permission credentials;
- staging/production and Community/Commercial credentials cannot be reused;
- only the local runtime host contains the credential; main Flutter executable, plist/resources, CLI, local standalone daemon, DMG metadata, and all SSH artifacts do not;
- credential-free artifacts are built in isolated target directories so Cargo cache cannot leak the secret;
- main/runtime bundle IDs, Developer ID Team, Hardened Runtime, notarization/stapling, version/build, edition/environment, and Sparkle public key are exact;
- official-build registration is idempotent, conflicts fail, and publication cannot precede registration;
- private Community and Commercial objects are immutable and not anonymously readable.

Do not print the secret to test for absence. Tests may compare hashes in memory or scan artifacts for a deterministic test-only canary credential in an isolated fixture build. Production verification must not echo or pass a real credential to `strings`, shell traces, snapshots, or logs.

### End-to-end staging acceptance

Exercise with real signed/notarized artifacts:

1. an official Community build registers its machine, exchanges its build credential for a session, loads Community license/offers, and installs a newer private Community update;
2. the same official Community build with an active paid entitlement obtains and installs the authorized Commercial release;
3. an official Commercial build with an unusable entitlement remains locally functional and installs a newer Community fallback when one with a greater global build exists;
4. a clean public source build retains local/SSH use but cannot discover either private release channel or use protected Relay services;
5. an official-build credential/session/build is revoked and subsequent protected HTTP/WebSocket/update access stops;
6. session/update bearers cannot be replayed across machine, environment, edition, release, appcast, or artifact;
7. a legacy Community installation follows the manual website bridge without losing local data, then receives later private updates.

## 11. Documentation updates

Update `community/docs/COMMUNITY_COMMERCIAL_ARCHITECTURE.md` and release documentation to state:

- public source remains fully usable for local and SSH workflows;
- Relay-hosted services and both in-app update channels require an official DitchNow release build;
- public verification keys let anyone verify releases but do not authorize source builds;
- the build credential is private release material, is intentionally absent from source, and is extractable from a distributed binary;
- Developer ID/Sparkle/notarization protect distribution integrity but are not remotely verifiable App Attest on macOS;
- old Community installs require one manual official bridge; later Community updates are private and automatic;
- Commercial updates continue through Relay and entitlement checks;
- no product claim promises absolute secrecy of independently published version numbers.

Remove or correct current documentation that says source builders can configure "the same authorized upgrade path" merely by using public verification roots.

## Completion report

At completion report:

1. files changed in the Community source and Commercial composition, including how mirrors/submodule changes were synchronized;
2. removal of the partial App Attest path and confirmation that no App Attest/DeviceCheck entitlement or portal work remains;
3. exact official-build session request/response and machine-signature behavior;
4. credential generation, storage, injection, isolation, rotation, registration, revocation, and known extractability limitation;
5. proof that CLI and SSH artifacts do not contain the credential;
6. private Community publication/discovery/Sparkle flow and preserved Commercial flow;
7. source-build, Community, active Commercial, and expired/refunded/revoked behavior;
8. dynamic license and action-availability UI changes;
9. rollout/bridge state and remaining deployment prerequisites;
10. all tests and real staging acceptance performed, with exact results;
11. any Relay contract mismatch that still blocks safe production enforcement.

Do not mark the work complete merely because unit tests pass. Production enforcement remains blocked until the real staging acceptance matrix succeeds with the final Relay implementation and real signed/notarized artifacts.
