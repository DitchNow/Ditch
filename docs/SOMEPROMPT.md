# The Ditch: macOS 27 App Attest and official-build authorization implementation plan

Implement the Ditch client, macOS runtime-host, packaging, release-pipeline, and UI side of hardware-backed official-build authorization in `/Users/mtn/Documents/Personal/The Ditch v2`.

Work in the current, heavily modified worktree. Preserve all existing user changes. Inspect every overlapping diff before editing, make narrow changes, and never reset, replace, or discard whole files to remove the superseded embedded-credential design. The root repository, its `community/` submodule, and the untracked `commercial/` mirror are not interchangeable: identify the canonical source for each file, make the change there, and synchronize only through the repository's intended composition workflow. Do not blindly edit generated build output or the untracked `commercial/` mirror.

This is an implementation plan and execution prompt for the Ditch repository only. The Relay must implement the matching protocol and validation policy independently before end-to-end completion. Do not invent compatibility aliases when the live Relay contract differs; update the shared fixture and both implementation prompts first.

## Corrected Apple platform fact

Do not repeat the obsolete blanket conclusion that App Attest is unavailable on macOS.

Apple announced at WWDC26 that App Attest is supported for full Mac applications on macOS 27 and later. Apple also states that availability must still be gated through `DCAppAttestService.isSupported`, because not every process or extension type is eligible:

- <https://developer.apple.com/videos/play/wwdc2026/201/>
- <https://developer.apple.com/forums/thread/836329>
- <https://developer.apple.com/documentation/devicecheck/establishing-your-app-s-integrity>

The older `isSupported` property page still says the value is false for an app running on a Mac. Treat that page as describing pre-macOS-27 behavior unless Apple publishes reconciled version-specific documentation:

- <https://developer.apple.com/documentation/devicecheck/dcappattestservice/issupported>

`The Ditch Runtime.app` is currently a plausible App Attest host because it is an `APPL` bundle with identifier `ai.theditch.runtime`, creates an `NSApplication`, uses `LSUIElement`, and runs in the logged-in user's context. Its executable name `ditchd` and login-item role do not by themselves make it a daemon. This eligibility is not proven until a real signed, provisioned build reports `isSupported == true`, successfully generates a key, and receives a valid Apple attestation on macOS 27.

## Product and security outcome

Deliver these exact behaviors:

1. Anyone may build the public Community source and use all local and SSH Community functionality.
2. A Community or Commercial macOS application signed and provisioned by DitchNow can establish App Attest identity through `The Ditch Runtime.app` on macOS 27 or later.
3. Relay validates an Apple-certified key whose relying-party identity is DitchNow's Team ID plus `ai.theditch.runtime`, binds it to the existing installation identity, and issues short-lived official-build sessions.
4. A source/ad-hoc build, or a build signed by another Apple team, cannot obtain a valid DitchNow attestation and cannot access protected Relay services or Relay's private Community/Commercial release catalogs.
5. An attested official Community installation can receive newer private Community updates without a paid license.
6. An attested installation with a usable paid entitlement can receive Commercial updates and use entitled hosted Remote Control services.
7. An attested Commercial installation whose paid entitlement is no longer usable retains local/SSH behavior and may receive a strictly newer Community release. Never downgrade to an older Community build.
8. macOS 11 through 26 remain supported for local and SSH features, but they do not receive protected Relay access under this plan. They get a clear manual-download/OS-requirement message instead of an embedded-secret fallback.
9. iOS keeps its existing device authentication. SSH remote nodes do not receive or generate a desktop App Attest key and cannot sponsor themselves.

The security boundary is precise: Relay trusts an Apple App Attest key belonging to the official runtime App ID, not the public source, a version string, Developer ID metadata sent by the client, or a hidden value embedded in the binary.

App Attest does not make fraud impossible. A compromised official Mac app can act as a broker, and any same-user process may attempt to drive an installed official runtime through its existing local protocols. Do not expose a generic attestation/signing oracle over those protocols. Relay-side counters, rate limits, machine binding, build/release state, entitlement checks, and Apple's fraud metric remain necessary.

## Chosen compatibility policy

Use the following fail-closed policy unless the product owner explicitly changes it before implementation:

- macOS 27+, supported full Ditch runtime app, successful attestation/assertion: Relay features may operate according to license and route policy;
- macOS 27+ but `isSupported == false`: treat as unsupported/suspicious for protected Relay access; local/SSH continue;
- macOS 11–26: local/SSH continue; protected Relay features and private in-app update discovery are unavailable;
- no per-build embedded credential, Developer ID parsing shortcut, static build token, public-key challenge masquerading as attestation, or automatic weak fallback;
- legacy Community users use the public website for the manual bridge/update path;
- legacy paid users may use only a narrowly configured Relay-side bridge release if the Relay prompt explicitly retains one.

Do not raise the application's global deployment target merely to enforce this. Keep older systems functional locally, compile against the macOS 27 SDK, weak-link/use availability guards, and gate only hosted functionality.

## Current code and dirty-worktree facts to preserve

The implementation must start from the code as it exists, not from an assumed clean revision:

- `apps/macos/macos/StatusHost/StatusHost.swift` and `community/apps/macos/macos/StatusHost/StatusHost.swift` own the nested menu-bar runtime application and call `ditch_runtime_run()` in-process on a background queue.
- `apps/macos/macos/package_status_host.sh` and its Community counterpart construct `The Ditch Runtime.app`, currently hard-code runtime version `1.0.0`/build `1`, statically link the Rust runtime, and sign the helper manually.
- The nested runtime currently has `CFBundlePackageType=APPL`, `CFBundleIdentifier=ai.theditch.runtime`, `LSUIElement=true`, and `NSPrincipalClass=NSApplication`.
- The main application uses `ai.theditch.app`; the Apple Team ID presently configured in Xcode is `HH3KSCHUQF`. Confirm it from the actual signing identity/profile rather than trusting this prompt blindly.
- The main Release entitlements currently contain only `com.apple.security.app-sandbox=false`. The nested runtime has no dedicated checked-in App Attest entitlements or provisioning-profile pipeline.
- `community/crates/ditch_upgrade/src/lib.rs` owns installation-signed Relay HTTP requests, the official-build session cache, Commercial discovery, signed release verification, and license/offer contracts.
- `crates/ditch_commercial/src/lib.rs` and `crates/ditchd/src/remote_control.rs` already propagate a cached `X-Ditch-Official-Build` session to some protected requests.
- `community/crates/ditchd/src/runtime_shared.rs` exposes Commercial/update operations over the normal local runtime protocol. That protocol must never gain raw App Attest key, key-ID, attestation-object, assertion-object, challenge, or generic-signing operations.
- `scripts/macos-release`, `scripts/package-commercial-dmg`, and `docs/MACOS_RELEASES.md` own the current Commercial release/signing/notarization path.
- Community release workflows and Community macOS packaging live under `community/.github/workflows/`, `community/apps/macos/`, and the Community release scripts.
- The current dirty worktree contains partially implemented per-build credential changes: `DITCH_OFFICIAL_BUILD_CREDENTIAL_FILE`, temporary generated Swift containing the credential, `ditch_set_official_build_credential`, `configure_official_build_credential`, `OFFICIAL_BUILD_CREDENTIAL`, build-credential hashing/registration, and related documentation. Replace only those trust-mechanism pieces. Preserve the concurrent Community-update, dynamic-license, offer, Commercial release, UI, and Remote Control work in the same files.

## Phase 0: mandatory real-platform feasibility gate

Do not delete the credential-path work or commit to the production protocol until this gate succeeds.

### Apple account and toolchain setup

On a controlled release/test Mac:

1. Install Xcode 27 and the macOS 27 SDK.
2. Run on real Apple-silicon hardware booted into macOS 27 with Full Security and System Integrity Protection enabled.
3. Confirm or register the explicit macOS App ID `HH3KSCHUQF.ai.theditch.runtime` in Certificates, Identifiers & Profiles.
4. Enable the App Attest capability for that App ID using the macOS 27/Xcode 27 workflow.
5. Generate fresh development and Developer ID provisioning profiles after enabling the capability. Enabling a capability invalidates older affected profiles.
6. Let Xcode 27 generate or document the exact supported macOS 27 entitlements. The known App Attest environment entitlement is `com.apple.developer.devicecheck.appattest-environment`, but do not invent or commit undocumented entitlement keys from forum posts. If Xcode 27 adds a separate macOS opt-in/CDHash entitlement, use only the value present in an Apple-issued provisioning profile and validate it during signing.
7. Confirm explicitly whether Developer ID distribution is supported for this App ID/capability. Do not infer eligibility merely because development signing works.

Provisioning profiles and public entitlement names are not cryptographic secrets, but Apple certificates, private keys, authentication credentials, and release-machine profiles must remain outside public source unless the established release process intentionally embeds a required profile in the signed app.

### Minimal runtime-host spike

Create a temporary, narrowly scoped spike using the actual `The Ditch Runtime.app` shape—not a standalone Swift command-line tool and not the main Flutter Runner.

The spike must:

- import `DeviceCheck` from a full `NSApplication` running as `ai.theditch.runtime` in the user session;
- log only OS/build, bundle ID, signing Team ID, and `isSupported`; never log a key ID or attestation bytes;
- verify `DCAppAttestService.shared.isSupported == true` on macOS 27;
- call `generateKey()` once and retain the returned key ID only in a temporary local Keychain item;
- request a one-time challenge from a controlled staging verifier;
- call `attestKey` using SHA-256 of the exact client-data bytes;
- have the staging verifier validate the Apple chain, nonce, relying-party ID, key binding, environment, initial counter, and the macOS 27 ACL Blob OID/security policy;
- generate one assertion over a second challenge and prove its counter/signature validation;
- repeat after quitting/relaunching the login item;
- repeat after an in-place signed update to confirm intended key continuity;
- verify an ad-hoc/source build and a build signed by another Team ID cannot satisfy the DitchNow relying-party check.

Record the Xcode build, macOS build, hardware, signing style, embedded profile UUID/type, effective entitlements, `isSupported` result, attestation environment, relying-party ID, and server-validation result in a non-secret test report.

If the real Developer ID runtime cannot generate and attest a key, stop. Do not silently fall back to the embedded credential or pretend the development-signed result proves production eligibility. Update the plan with the exact Apple limitation and ask the product owner to choose between macOS-27 Relay disablement, a weaker disclosed fallback, or a different distribution model.

## Phase 1: freeze the exact Relay/App Attest contract

Before production code, check in a versioned JSON fixture/schema shared with TheDitchRelay. The client and Relay must agree on route names, exact JSON keys, bounds, canonical client-data bytes, hashing, challenge expiry, and errors.

Use this route family unless the updated Relay contract deliberately chooses equivalent final names:

- `POST /v1/official-build/attestation/challenge`;
- `POST /v1/official-build/attestation/register`;
- `POST /v1/official-build/session/challenge`;
- `POST /v1/official-build/session`.

All four occur only after `/v1/machines/register`. All requests are protected by the existing `DITCH1` installation signature, timestamp, nonce, body digest, and replay rules. App Attest augments the installation identity; it does not replace it.

### Registration challenge

Request:

```json
{
  "protocol_version": 1,
  "purpose": "register",
  "machine_id": "installation UUID",
  "installation_signing_public_key_sha256": "64 lowercase hex",
  "build": 107,
  "version": "0.1.0",
  "edition": "community",
  "deployment_environment": "staging"
}
```

Response:

```json
{
  "protocol_version": 1,
  "challenge": {
    "challenge_id": "UUID",
    "purpose": "register",
    "value": "unpadded base64url random bytes",
    "expires_at": "RFC3339 UTC"
  }
}
```

Construct the App Attest `clientDataHash` as SHA-256 of one documented canonical byte sequence. Prefer a fixed UTF-8, newline-framed protocol such as:

```text
DITCH-APP-ATTEST-REGISTER-1
<challenge_id>
<challenge_value>
<machine_id>
<installation_signing_public_key_sha256>
<deployment_environment>
<edition>
<version>
<build>
```

Every displayed line, including the final build line, is terminated by one LF byte. The final fixture must define this exact trailing-newline behavior, normalization, casing, integer formatting, and size bounds. Do not hash reserialized dictionaries whose key order can drift between Swift, Rust, and TypeScript.

Registration request:

```json
{
  "protocol_version": 1,
  "challenge_id": "UUID",
  "key_id": "opaque App Attest key identifier",
  "client_data": "unpadded base64url of the exact canonical bytes",
  "attestation_object": "unpadded base64url Apple attestation object"
}
```

Relay, not the client, validates the complete Apple object and consumes the challenge atomically. The client validates only response shape and binding; it never declares itself official based on local checks.

### Session assertion

After registration, request a session challenge with the key ID and the same authoritative local build metadata. Construct a separate domain-separated canonical byte sequence beginning with `DITCH-APP-ATTEST-SESSION-1\n`, binding at least:

- challenge ID and value;
- machine ID;
- installation signing-public-key hash;
- App Attest key ID or its agreed digest;
- deployment environment;
- edition;
- version and global build number;
- requested session purpose/protocol version.

Call `generateAssertion` with SHA-256 of those exact bytes, then send:

```json
{
  "protocol_version": 1,
  "challenge_id": "UUID",
  "key_id": "opaque App Attest key identifier",
  "client_data": "unpadded base64url canonical bytes",
  "assertion_object": "unpadded base64url assertion"
}
```

Relay must atomically verify and advance the assertion counter, consume the challenge, validate machine/key/environment/build binding, and return:

```json
{
  "protocol_version": 1,
  "official_build_session": {
    "bearer": "opaque high-entropy base64url token",
    "expires_at": "RFC3339 UTC",
    "build": 107,
    "version": "0.1.0",
    "edition": "community",
    "channel": "beta"
  }
}
```

Continue sending that short-lived bearer as `X-Ditch-Official-Build` alongside normal `DITCH1` headers on protected machine requests. The session cache remains machine/build/environment scoped, single-flight, memory-only, and refreshed with a safety margin.

The Relay must be updated from the superseded credential-hash contract before this client protocol is merged. It must validate the DitchNow RP ID, Apple root/chain, nonce, credential public key, environment AAGUID, initial counter, macOS ACL Blob OID/security conditions, challenge purpose/expiry, official-build registry, and subsequent assertion signatures/counters. The client must not attempt to duplicate server trust decisions.

## Phase 2: implement a narrow Swift App Attest provider in the runtime app

Add a dedicated Swift source beside `StatusHost.swift`, for example `StatusHost/AppAttestProvider.swift`, compiled only into `The Ditch Runtime.app`.

Responsibilities:

- gate every API call with `#available(macOS 27, *)` and `DCAppAttestService.shared.isSupported`;
- generate at most one App Attest key for this installation/environment unless recovery requires rotation;
- keep the opaque key ID in a dedicated non-synchronizing, this-device-only Keychain item;
- call `attestKey` only after receiving a fresh Relay registration challenge;
- call `generateAssertion` only for a fresh Relay session challenge;
- return bounded opaque bytes/errors to the in-process Rust caller;
- serialize key generation/attestation/assertion work to avoid races and counter disorder;
- apply bounded timeouts and cancellation-safe completion handling;
- redact key IDs, challenges, client data, attestations, assertions, receipts, and session bearers from logs/crashes;
- classify unsupported OS/service, missing profile/capability, invalid key, rate limit, Apple server unavailable, and permanent authorization failure separately.

Use one App Attest key per Ditch installation/environment, not per request, release check, app launch, build, Community/Commercial edition, or UI user. Apple keys survive ordinary app updates but not reinstall/device restore. The existing installation identity remains the machine binding.

Keychain requirements:

- use a dedicated service such as `ai.theditch.runtime.app-attest`;
- namespace the account by Relay/App Attest environment without putting secrets into the name;
- set synchronizable false and use an appropriate `ThisDeviceOnly` accessibility class compatible with a login item after user login;
- store only the opaque key ID and non-secret registration metadata; the Secure Enclave private key remains Apple-managed;
- never share the item through a broad Keychain access group unless a proven Apple requirement demands it;
- never expose CRUD for this item over the notification socket, Rust runtime socket, Flutter method channel, CLI, SSH, environment, preferences, or diagnostics.

Recovery rules:

- retry Apple `serverUnavailable`/transient network failures with server-directed exponential backoff;
- do not generate replacement keys in a tight loop;
- if Apple reports an invalid key after reinstall/restore, delete only the stale local key-ID record, obtain a fresh registration challenge, and rotate once within a bounded policy;
- if Relay lost registration for a still-local key, follow the agreed re-registration/rotation result rather than guessing;
- never treat `isSupported == false` as successful Community authorization.

## Phase 3: build a safe Swift-to-Rust bridge

Keep App Attest inside the same `The Ditch Runtime.app` process that contains the statically linked Rust runtime. Do not add an XPC service or Unix-socket attestation helper.

Replace `ditch_set_official_build_credential` with a narrow, once-only App Attest provider registration. A callback-based C ABI is appropriate because App Attest is asynchronous. The final ABI must be explicitly documented and tested for:

- operation enum/version;
- immutable request buffer ownership and length bounds;
- completion callback ownership;
- result/error buffer ownership and destruction;
- exactly-once completion;
- timeout and late-callback behavior;
- concurrency/single-flight rules;
- panic/exception containment across FFI;
- shutdown and use-after-free prevention.

The Rust runtime already runs on a background queue, so it may wait on an internal condition variable for a bounded Swift completion. Never block AppKit's main thread and never dispatch synchronously in a cycle that can deadlock. Prefer Swift actors/serial execution for App Attest and Rust `Arc`-owned completion state for late callback safety.

Expose only these semantic operations internally:

- query supported/unsupported status;
- load or generate the installation's App Attest key ID;
- attest that exact key using a Relay-provided registration `clientDataHash`;
- generate an assertion for that key using a Relay-provided session `clientDataHash`;
- invalidate the stored key ID only under the bounded recovery policy.

Do not expose “sign arbitrary bytes” to any IPC caller. Rust may pass only a locally reconstructed hash of the exact checked-in canonical Relay client data. Swift must reject wrong digest lengths and oversized/unknown operations. The notification-control socket remains limited to its AppKit notification functions.

## Phase 4: replace the Rust credential exchange without disturbing other work

In canonical `community/crates/ditch_upgrade/src/lib.rs` and the composed root:

1. Remove `OFFICIAL_BUILD_CREDENTIAL`, `configure_official_build_credential`, raw credential validation, and the credential argument to `/v1/official-build/session`.
2. Preserve and adapt `OFFICIAL_BUILD_SESSION`, its expiry safety margin, session header propagation, existing machine registration, `DITCH1` signing, strict Relay origins, and the concurrent Community/Commercial work.
3. Add a mockable `AppAttestProvider` abstraction implemented through the registered Swift callbacks only in the local macOS runtime.
4. Add strict types for both challenges, registration, assertion, and session envelopes with `deny_unknown_fields` where compatible with the versioned contract.
5. Generate canonical client data in Rust from compile-time/runtime-authoritative build metadata and the registered installation identity. Never accept edition/build/environment from Flutter or a local socket request.
6. Register the machine first, then load/generate and register the App Attest key when needed, then obtain a session assertion.
7. Cache only the short-lived Relay bearer in memory. A rejected/expired session may cause one bounded new challenge/assertion and retry; never recurse indefinitely.
8. Distinguish unsupported platform from transient network/Apple failures and Relay rejection.
9. Keep protected requests fail-closed when no valid official session exists.
10. Keep iOS and SSH remote-node authentication paths unchanged.

Source builds compile the public App Attest implementation normally. They do not need hidden source or a secret compilation switch. Their Apple/ad-hoc signature and provisioning identity will not produce the DitchNow RP ID, so Relay rejects their attestation. Do not add a developer setting that lets a caller override Team ID, bundle ID, App Attest environment, edition, build, or Relay production origin.

Update `community/crates/ditchd/src/lib.rs` and the composed root to remove `ditch_set_official_build_credential` and register the App Attest callbacks/provider before `ditch_runtime_run()`. Preserve the single-runtime invariant.

## Phase 5: packaging, signing, provisioning, and release metadata

Update both canonical Community and composed Commercial runtime packaging paths.

### Runtime bundle construction

- add `DeviceCheck.framework` to the Swift link step;
- compile the App Attest Swift provider with the runtime host;
- write the real product `CFBundleShortVersionString` and globally monotonic `CFBundleVersion` into `The Ditch Runtime.app`; remove the current `1.0.0`/`1` constants;
- keep `CFBundleIdentifier=ai.theditch.runtime`, `CFBundlePackageType=APPL`, `NSPrincipalClass=NSApplication`, and the user-context application launch path;
- preserve `LSUIElement` unless the feasibility spike proves it prevents eligibility;
- do not turn the runtime into a LaunchDaemon, bare executable service, system extension, or XPC-only process.

### Dedicated runtime entitlements/profile

Create dedicated development and release entitlements for `The Ditch Runtime.app`; do not reuse the main Runner entitlements by accident. Add the App Attest environment/capability values actually authorized by the Apple-issued profile. Keep Hardened Runtime and avoid unnecessary entitlements.

Because the helper is assembled by shell rather than an Xcode target:

- require an explicit runtime provisioning-profile path in official release configuration if Developer ID App Attest requires an embedded profile;
- verify its Team ID, application identifier, bundle ID, capability entitlements, distribution type, and validity dates before compiling/publishing;
- copy it to the exact profile location expected for a macOS app bundle;
- sign nested executables first and `The Ditch Runtime.app` last with its dedicated entitlements;
- re-signing in `scripts/package-commercial-dmg` must preserve/reapply the runtime entitlements and profile rather than stripping them;
- sign the main app after the runtime app and verify the entire nested chain;
- keep Developer ID timestamping, Hardened Runtime, notarization, stapling, Sparkle signatures, and release-manifest signatures.

Release verification must inspect, not assume:

```sh
codesign --verify --deep --strict --verbose=2 <app>
codesign -d --entitlements :- <runtime-app>
codesign -dv --verbose=4 <runtime-app>
security cms -D -i <runtime-app>/Contents/embedded.provisionprofile
spctl --assess --type execute --verbose=4 <app>
```

Verify that the effective entitlements are a subset authorized by the embedded profile and that the runtime Team ID/bundle ID/version/build exactly match the registered official build. Do not parse those local commands at runtime as a replacement for App Attest; they are release-time checks only.

### Remove embedded credential plumbing

Surgically remove from release/configuration paths:

- `DITCH_OFFICIAL_BUILD_CREDENTIAL_FILE` and `DITCH_OFFICIAL_BUILD_CREDENTIAL`;
- temporary generated `DitchOfficialBuildCredential.swift`;
- raw credential permission/entropy checks;
- credential hashes in build/release registration;
- credential redaction/leakage rules that exist only for this deleted secret;
- documentation claiming an embedded build secret is the trust boundary.

Do not remove generic secret hygiene, environment isolation, session-bearer redaction, or unrelated release signing keys/tokens.

The publisher still registers immutable official-build metadata—environment, edition, version, global build number, channel, Community revision, runtime bundle ID, expected DitchNow Team/RP ID, publication state, and release identity—but no per-build authentication secret or secret hash. Relay trusts Apple attestation plus its build registry.

## Phase 6: preserve private Community and Commercial update behavior

Do not regress the update/license work already in progress.

- Private Community release discovery requires a valid official-build session but no paid entitlement.
- Commercial release discovery requires a valid official-build session plus usable paid/admin entitlement and existing activation rules.
- Expired/refunded/revoked/inactive Commercial authority falls back to Community selection without downgrading.
- The official session's build is authoritative for update selection; ignore UI/query-supplied current builds for authorization.
- Community and Commercial Sparkle appcast/artifact requests continue to use their separate, release-scoped update bearer, not an App Attest object or key ID.
- Developer ID/App Attest establishes official runtime identity; P-256 signed release descriptors and Sparkle Ed25519 signatures continue to establish release/artifact integrity.
- Session bearer refresh must work for Commercial/Remote Control callers already using `cached_official_build_bearer()`.
- The runtime may keep operating while the Flutter window is closed.

For macOS 11–26 or unsupported App Attest:

- local projects, agents, terminals, persistence, and SSH continue;
- the UI must not claim the user needs to buy a license to become official;
- show a stable explanation that protected Relay services and private automatic updates require macOS 27 and an official Ditch build;
- provide a public HTTPS link to manual downloads/release guidance without leaking a private catalog;
- do not repeatedly prompt or retry attestation.

## Phase 7: prevent the runtime from becoming an attestation proxy

Audit every local ingress into `The Ditch Runtime.app`:

- the normal mode-0600 Rust runtime socket;
- `notification-control.sock`;
- CLI invocations;
- Flutter method channels;
- SSH bridges;
- URL handlers and distributed notifications;
- any future XPC service.

No ingress may request arbitrary App Attest key generation, attestation, assertion, challenge signing, key deletion, or raw session issuance. Hosted business operations may internally obtain a session only through typed, server-defined flows using compile-time build identity and the authenticated installation identity.

Do not return key IDs, attestation/assertion objects, Relay challenges, counters, Apple receipts, or official session bearers over IPC. Existing UI/runtime calls should return only sanitized business results. Keep socket ownership/mode checks, but do not describe same-user Unix permissions as code-identity authentication.

If a future design needs the disposable main app to invoke an attestation-sensitive operation directly, use an XPC connection that validates the caller's audit token against the expected DitchNow Team ID and `ai.theditch.app` designated requirement. Do not add that XPC surface merely for this task; the current in-process runtime design is smaller and safer.

## Phase 8: key lifecycle, updates, and edition transitions

Test and document these transitions:

- first official Community launch on macOS 27;
- quit/relaunch and login restart;
- Community-to-newer-Community Sparkle update;
- Community-to-Commercial replacement;
- Commercial-to-newer-Commercial update;
- expired Commercial receiving a newer Community build;
- app reinstall and stale Keychain key ID;
- device restore/migration;
- App Attest key revoked by Relay;
- official build revoked while a session/WebSocket exists;
- staging-to-production build replacement;
- downgrade attempt;
- macOS 26-to-27 OS upgrade.

Keys are per device/installation and survive ordinary app updates. Do not rotate merely because edition/version/build changes. Relay sessions remain short-lived and are bound to the current declared/registered build. If Apple exposes bundle-version data in the macOS 27 attestation/assertion format, Relay validates it according to Apple's current validation guide. Do not assume an iOS-only authenticator extension exists on macOS; verify the actual macOS 27 object and shared fixture.

Staging and production must remain isolated. Never let a development/sandbox App Attest key authorize production, or a production assertion authorize staging merely because the RP ID matches.

## Phase 9: errors, UX, and telemetry

Use stable, non-secret classifications across Rust protocol and Flutter UI:

- `official_build_platform_unsupported` for macOS below 27;
- `official_build_attestation_unsupported` for unexpected `isSupported == false` on an otherwise eligible macOS 27 runtime;
- `official_build_attestation_pending` for bounded enrollment work;
- `official_build_attestation_failed` for non-transient Apple/client failure;
- Relay's existing `official_build_required` for rejected/missing official authorization;
- ordinary network/retry errors for transient transport failures;
- existing entitlement errors only after official-build authorization succeeds.

Do not reveal whether a private release exists. Never log or report raw key IDs, challenges, canonical client data, attestation/assertion objects, counters, receipts, session/update bearers, installation public keys, or provider identifiers. Use request IDs and coarse reason codes.

Telemetry for `isSupported == false` on macOS 27 may be a fraud/compatibility signal, but do not block the entire local app or label a legitimate user fraudulent. Gate only Relay-backed features and retain a support path.

## Phase 10: deterministic tests

### Rust and contract tests

- exact canonical client-data bytes, domain separation, trailing newline, casing, bounds, and SHA-256 fixtures;
- strict parsing of challenge/register/session envelopes;
- mock Swift provider: supported, unsupported, key generation, attestation, assertion, timeout, late callback, invalid key, and transient error;
- one key registration under concurrency and single-flight session refresh;
- session expiry/safety margin and one bounded refresh/retry;
- installation/build/environment/edition mismatch rejection;
- no App Attest artifacts or bearer crossing local protocol responses;
- all protected HTTP call sites attach normal machine signatures plus the official session and fail closed if either authorization is unavailable;
- iOS and SSH paths remain independent;
- Community update works without paid entitlement;
- Commercial update still requires entitlement;
- expired paid authority selects only a strictly newer Community release.

### Swift/FFI tests

- ABI layout and buffer ownership from both languages;
- exactly-once completion, timeout, cancellation, shutdown, and late callback safety;
- serial assertion generation and counter ordering;
- Keychain namespace/accessibility/non-synchronization and stale-ID recovery;
- unsupported macOS path never calls App Attest APIs;
- no main-thread deadlock;
- notification/runtime sockets expose no attestation operation.

Use a deterministic fake provider for CI. Fake attestation must be structurally impossible to enable in production Release builds. Do not make normal CI depend on Apple attestation servers.

### Packaging and security tests

- Community and Commercial runtime bundles have correct bundle ID/version/build;
- official Release runtime carries the expected profile and authorized entitlements;
- source/ad-hoc build does not claim DitchNow profile/Team identity;
- every nested code signature, timestamp, Hardened Runtime flag, notarization ticket, and Sparkle signature remains valid;
- re-signing the DMG/app does not strip runtime App Attest entitlements/profile;
- no old build credential, credential hash, credential file variable, or generated credential source remains;
- CLI and SSH artifacts contain no App Attest callback surface or Relay session bearer;
- public Community checkout builds/tests without any private Apple or Relay file.

### Real staging acceptance on macOS 27

Unit tests are insufficient. Complete all of these on real hardware:

1. Signed/provisioned Community runtime reports supported and registers one valid key.
2. Relay validates RP ID `HH3KSCHUQF.ai.theditch.runtime`, Apple environment, ACL Blob/security policy, nonce, and counter.
3. Relaunch obtains a session assertion without generating a new key.
4. An official Community installation discovers and installs a newer private Community update without payment.
5. That installation with usable entitlement discovers and installs a Commercial release.
6. Commercial with unusable entitlement retains local/SSH and receives only a newer Community fallback.
7. Public source/ad-hoc and other-Team builds are denied and learn no private catalog metadata.
8. macOS 26 retains local/SSH and receives the explicit unsupported/manual-update UX.
9. Key/build revocation stops new HTTP sessions and WebSocket admission within Relay's documented bound.
10. Reinstall/key loss performs one safe key rotation; update preserves the existing key.
11. Staging keys/sessions cannot cross to production.
12. The runtime cannot be used as a generic signing oracle through either local socket.

## Rollout order

1. Complete Phase 0 using a temporary staging verifier.
2. Freeze and check in the cross-repository contract fixture/schema.
3. Implement Relay attestation validation and keep enforcement disabled.
4. Implement the Swift provider, FFI bridge, and Rust client behind staging-only feature/configuration gates.
5. Replace embedded credential plumbing only after real App Attest registration/assertion succeeds.
6. Complete deterministic suites and real staging acceptance.
7. Ship one signed/notarized manual Community bridge and, if necessary, one narrow paid Commercial bridge.
8. Run Relay `audit` mode and inspect failures without admitting private Community discovery anonymously.
9. Enable enforced mode only for macOS 27+ official runtimes after the complete acceptance matrix passes.
10. Remove bridge exceptions on their declared deadline and retain public manual guidance for older macOS.

Production must not be marked complete merely because the app compiles or a development-signed spike works.

## Completion report

At completion, report:

1. exact canonical files and composed mirrors changed;
2. Phase 0 hardware/OS/Xcode/signing/profile/entitlement results;
3. final Apple Developer portal configuration for `ai.theditch.runtime`;
4. exact challenge/register/assertion/session contract and fixture checksum;
5. Swift provider, Keychain lifecycle, and FFI ownership model;
6. how generic signing-proxy access is prevented;
7. embedded-credential code/configuration/documentation removed without losing unrelated work;
8. runtime signing/provisioning/notarization verification;
9. macOS 11–26 behavior and macOS 27 enforcement behavior;
10. Community, Commercial, expired-license, iOS, SSH, and WebSocket outcomes;
11. deterministic test commands and exact results;
12. real staging acceptance evidence;
13. remaining Relay or Apple-account blockers before production enforcement.

Do not claim hardware-backed official-build authorization until a real Developer ID build of `The Ditch Runtime.app` on macOS 27 produces an attestation that Relay validates end to end.
