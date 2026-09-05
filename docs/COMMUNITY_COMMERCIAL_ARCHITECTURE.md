# Community and Commercial architecture

This document records the implemented edition boundary and the execution paths found during the split. Community remains `AGPL-3.0-only`. The separate Commercial repository is proprietary and pins an exact Community commit.

## Runtime and state invariants

- `ditchd` is the only execution authority for projects, Codex children, PTYs, sessions, attention state, and SQLite.
- A Community installation runs one Community `ditchd`; a Commercial installation replaces it with one Commercial `ditchd`. No service sidecar or second session authority is permitted.
- The canonical bundle identifier remains `ai.theditch.app`, and both editions use `~/Library/Application Support/The Ditch/`, the same preferences domain, project metadata, SSH credentials, and `CODEX_HOME`.
- The local macOS installation identity is shared by both editions in the login Keychain. Existing mode-0600 identity files are migrated only after a Keychain write/read-back succeeds; headless SSH runtimes retain their protected `~/.ditch` file store.
- SSH hosts use one `~/.ditch/bin/ditchd`. Runtime replacement is refused when liveness cannot be proven and deferred while sessions are active.

## Traced Community execution paths

1. Local project creation starts in `DitchRuntimeClient.createProject`, crosses the versioned Unix-socket protocol, and is validated, registered, and persisted by `ditchd`/`ditch_store`.
2. SSH project creation discovers OpenSSH aliases through `ditch_ssh`, checks or installs the matching remote artifact, asks the remote daemon for its generic host identity, and persists a project whose execution target names that host.
3. The local daemon is linked into the AppKit login-item host and serves the per-user socket while the Flutter window is disposable.
4. A remote daemon serves the same typed contract under `~/.ditch`; `bridge --stdio` is only a byte bridge from system `ssh` to that socket.
5. `scripts/build-remote-artifacts` produces target-qualified Community binaries plus a manifest binding edition, source revision, build identifier, protocol version, size, and SHA-256 digest.
6. Flutter and `ditch_cli` use newline-delimited JSON envelopes over the mode-0600 local Unix socket.
7. Codex launch and resume remain daemon-owned child-process operations with persisted native thread identity and the user's existing `CODEX_HOME`.
8. Project shells are daemon-owned PTYs with explicit open, input, resize, output, and close requests.
9. Projects, sessions, transcripts, attention, settings, and SSH execution targets remain in the canonical SQLite database; Community migrations use their own ledger and preserve unknown Commercial tables.
10. Community contains no Relay enrollment, iPhone pairing, device roster, mobile crypto, projection, or mobile command route.
11. The place where Remote Control settings previously lived now opens a restrained Commercial discovery and activation dialog.
12. Community release packaging builds the exact Rust sources, embeds one local runtime helper, runs the leakage gate, signs/notarizes the canonical app, and emits the public DMG.

## Source composition

The public workspace contains the complete local and SSH product plus three narrow edition-support boundaries:

- `ditch_product`: centralized edition/capability discovery metadata;
- `ditch_identity`: an anonymous installation/host identifier and signing proof with no device enrollment or transport semantics;
- `ditch_upgrade`: pricing display, checkout/redemption/entitlement calls, signed release manifests, digest verification, and private staging helpers.

The public protocol and store contain only Community runtime and upgrade-bootstrap contracts. In particular, the former private remote crate, mobile protocol variants, device tables, Relay client, pairing controller, projections, and Remote Control screens are absent from the public workspace.

The private `Ditch-Commercial` repository owns those removed sources. Its `community/` submodule and `COMMUNITY_REVISION` file establish a deterministic exact-core relationship, and CI rejects a dirty or mismatched pin. Commercial composition is statically linked into its single `ditchd`; it is not a dynamic plug-in and does not introduce a second daemon. Private protocol contracts remain canonical there for deterministic Relay/iPhone export.

## Purchase and installation boundary

Community can request a hosted checkout or redeem a license using its installation identity. A redirect is never payment proof: the daemon polls the authoritative backend entitlement before requesting a Commercial release. Raw license keys are moved into a zeroizing redacted wrapper and are never written to the database or preferences.

The publisher signs durable release metadata binding Commercial edition, release ID/channel, version/build, minimum compatible Community build, exact Community revision, rollback sequence, artifact digest and size, bundle ID, Apple Team ID, and Relay appcast/artifact routes. Relay separately issues a short-lived download session after checking entitlement. Community verifies the signed release metadata before invoking Sparkle 2; Sparkle authenticates to the Relay with that session and verifies the signed appcast/update before atomic application replacement. Application Support, preferences, project files, Keychain items, and Codex state remain in place because neither bundle identity nor data paths change.

Official builds embed the public verification keys for their selected environment. Those public trust roots must be published so source builders can configure the same authorized upgrade path. Signing private keys, billing credentials, artifact authorization, Relay authorization, and Apple release credentials never belong in Community source.

Hosted pricing, entitlement, and Remote Control are separately gated by a per-build credential injected only by the official release environment and exchanged for a short-lived machine-bound Relay session. This prevents a checkout containing only Community source from using hosted services. On current macOS this is not hardware-backed remote attestation, so a determined reverse engineer can extract the distributed build credential; rotation, revocation, environment isolation, machine signatures, and short session lifetimes limit that exposure.

## Entitlement expiry

Commercial entitlement gates only proprietary capabilities. Relay authorization remains the security boundary for hosted Remote Control; client gating is UX and defense in depth. Expiry leaves the installed Commercial binary, database, identity, projects, sessions, and SSH configuration intact. Local and SSH Community capabilities remain available, and renewal restores proprietary capability without a forced reinstall.

## Release gates

Community CI builds and tests without a private checkout, runs Rust and Flutter analysis/tests, builds the macOS app, and runs `scripts/check-community-leakage`. That gate rejects prohibited paths, dependencies, protocol symbols, Relay origins, a second daemon, leaked binary strings, and an app with anything other than one local `ditchd`.

Commercial CI verifies the exact Community pin, runs both test suites, produces a single Commercial `ditchd`, and rejects a second daemon. Commercial artifacts belong only in entitlement-protected storage and never in the public GitHub release.

## Contribution and publication blockers

The current static composition means a third-party Community source contribution would also be incorporated into a proprietary Commercial executable when Commercial advances its Community pin. Making that feature available without an added subscription charge does not itself provide proprietary distribution permission. DitchNow therefore does not currently merge external source into release branches and does not require a separate CLA; a qualified lawyer must approve and maintainers must publish any future no-separate-CLA inbound licensing and sign-off workflow before that gate is opened.

The existing private repository history contains the Remote Control implementation that was removed from the Community tip. The repository must remain private and its old branches and tags must not be published. Before public launch, maintainers must make a verified backup, choose either a fresh public history from the reviewed Community tree or a separately reviewed history-filter procedure, inspect the resulting objects and refs for private source/contracts/keys/fixtures/security material, rerun the clean-checkout and leakage gates, and obtain explicit maintainer approval. Commercial must then pin the exact clean Community commit that public builders and CI can fetch. This task does not authorize history rewriting, force-pushing, or visibility changes.
