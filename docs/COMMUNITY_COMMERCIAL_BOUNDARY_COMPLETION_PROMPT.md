# Codex Implementation Prompt — Complete the Ditch Community/Commercial Source Boundary

## Mission

Work in these two existing repositories:

```text
Community:
/Users/mtn/Documents/Personal/The Ditch v2
remote: git@github.com:DitchNow/Ditch.git

Commercial:
/Users/mtn/Documents/Personal/The Ditch v2/commercial
remote: git@github.com:DitchNow/Ditch-Commercial.git
```

Complete the Community/Commercial extraction in one coherent implementation.

The existing root `DitchNow/Ditch` repository is the authoritative Community
source. The Commercial repository must consume one exact committed Community
revision through its `community` submodule and statically compose proprietary
capabilities into the same authoritative runtime.

The finished Community application must build independently from its own
repository and upgrade in place to the official signed Commercial application
after Relay confirms entitlement. Projects, sessions, Codex identity, Ditch
installation identity, settings, SSH projects, Keychain entries, and local
state must survive that replacement.

Do not stop after reconnaissance. Implement, test, create the approved local
Community commit, pin it in Commercial, and create the approved local
Commercial commit.

Do not push, deploy, publish, upload release artifacts, change Stripe, change
live Cloudflare state, rewrite Git history, or change repository visibility.

## Confirmed product decisions

These decisions are final for this task:

1. `DitchNow/Ditch` is the future public Community source of truth.
2. Community and Commercial both use:

   ```text
   application name: The Ditch.app
   bundle ID: ai.theditch.app
   Apple Team ID: HH3KSCHUQF
   ```

3. The normal upgrade replaces Community with Commercial. It must not install
   two applications side-by-side.
4. Both editions use exactly one runtime executable named `ditchd`.
5. Local Community commits are authorized for this task: Community first,
   Commercial second. Pushing and deployment are not authorized.
6. Community remains `AGPL-3.0-only`. Commercial remains proprietary.
7. SSH projects and SSH execution are Community functionality.
8. Remote Control means proprietary iPhone control through Ditch Relay. It is
   not SSH and must not remain in Community implementation source.
9. A separate CLA is not being adopted in this task. Until qualified counsel
   approves and maintainers publish a no-separate-CLA inbound licensing and
   sign-off workflow that grants all rights required by the static Commercial
   composition, external source contributions must not be merged into release
   branches. Free availability of contributed functionality in Community does
   not itself grant proprietary distribution rights.

## Current repository facts

Verify these facts before changing anything; adapt to current source if another
agent has advanced it:

- The root Community repository is on the `ssh` development line and already
  contains substantial uncommitted extraction work.
- In the root working tree, `crates/ditch_remote`,
  `crates/ditchd/src/remote_control.rs`, private Remote Protocol docs, fixtures,
  schemas, and Relay export code are already marked for deletion.
- Root Community already contains new Community-safe work including:

  ```text
  crates/ditch_identity
  crates/ditch_product
  crates/ditch_upgrade
  crates/ditchd/src/runtime_shared.rs
  crates/ditchd/src/edition.rs
  scripts/check-community-leakage
  scripts/package-community-dmg
  ```

- Root Community `ditchd/src/main.rs` already composes `runtime_shared.rs` with
  the no-op Community `edition` provider and `ssh_remote`.
- Commercial already contains private composition work including:

  ```text
  crates/ditch_commercial
  crates/ditch_remote
  crates/ditch_store      (private store extension)
  crates/ditch_protocol   (private extended protocol)
  crates/ditchd/src/edition.rs
  crates/ditchd/src/remote_control.rs
  apps/macos/lib/commercial_remote_surface.dart
  ```

- Commercial `ditchd` includes the exact Community `runtime_shared.rs` and
  supplies proprietary edition hooks. It must continue producing one `ditchd`.
- `commercial/community` is currently pinned to old commit
  `4a8fad14747246b8950f28dd9de01c3f48d62e92` and contains independent
  uncommitted changes. It is stale and must not become an independently edited
  third source tree.
- `COMMUNITY_REVISION` currently names that old commit.
- The old commit history contains Remote Control followed by SSH development.
- The root repository has both Community and Commercial remotes configured.

Do not assume that every existing change is correct. Preserve user work, audit
all diffs, and resolve the architecture deliberately. Do not use destructive
Git commands such as `git reset --hard` or `git checkout --`.

## Target architecture

```text
DitchNow/Ditch — Community, AGPL-3.0-only
├── complete local macOS product
├── authoritative local/SSH runtime services
├── Community ditchd composition
├── local projects, Codex, agents, PTYs and Git
├── project/session/settings persistence
├── SSH configuration, browsing and execution
├── Community-safe protocol/store
├── installation identity
├── product capability discovery metadata
├── provider-neutral Commercial upgrade bootstrap
├── authorized signed Commercial installer integration
└── no proprietary Remote Control implementation

DitchNow/Ditch-Commercial — proprietary
├── exact Community commit as submodule
├── private capability registry/providers
├── private extended protocol and store migrations
├── Remote Control, Relay, pairing, crypto and projections
├── private Flutter edition surface
└── Commercial ditchd composition using the exact Community runtime
```

Runtime invariant:

```text
Community installation -> one Community ditchd
Commercial installation -> one Commercial ditchd
```

Never create `ditch-servicesd`, a plugin daemon, a proprietary dynamic plugin
ABI, a second session authority, a second PTY owner, or a second project
database.

## Phase 1 — Protect and classify existing work

Before editing:

1. Record status, branch, HEAD, remotes, submodule status, and
   `COMMUNITY_REVISION` for both repositories.
2. Inventory every modified, deleted, and untracked file in root Community,
   Commercial, and `commercial/community`.
3. Compare the stale submodule changes against both root Community and private
   Commercial. Classify each unique hunk as:

   ```text
   Community-safe shared runtime
   Community upgrade/bootstrap
   proprietary Commercial capability
   obsolete duplicate
   unrelated user work
   ```

4. Integrate every useful unique change into its correct authoritative
   repository before cleaning the submodule.
5. Never discard a unique submodule change. If changing the submodule checkout
   later would overwrite local changes, preserve them first using a recoverable
   local mechanism and report it.
6. Do not create or pin a temporary contaminated “Community” commit containing
   Remote Control merely to satisfy release preflight.

The root Community repository and Commercial repository are the two sources of
truth. `commercial/community` becomes only a clean checkout of a root Community
commit.

## Phase 2 — Finish the Community source boundary

### Remove proprietary implementation

Community source, tracked build inputs, generated outputs, documentation, and
release archives must not contain implementation equivalent to:

```text
ditch_remote
remote_control.rs
mobile Remote Protocol request/response contracts
pairing protocol and QR payload implementation
Relay WebSocket control client
Remote Control machine/device crypto
encrypted mobile command routing
remote device persistence
mobile project/session projections
transcript relay
APNs Remote Control integration
private relay configuration
private protocol export scripts/fixtures/schemas
commercial-only providers or integrations
```

Complete and verify removal from at least:

- Cargo workspace membership and dependencies;
- `ditchd` modules and request routing;
- public protocol enums, structs, serialization and tests;
- Community store schema, migrations and queries;
- Flutter imports, screens, pairing/device widgets and tests;
- macOS build phases and bundled artifacts;
- README, architecture/security/remote docs and scripts;
- generated contracts and copied Relay configuration;
- CI, packaging and release outputs.

Deleting current files is not sufficient if another public tracked file embeds
the private contract or implementation. Inspect content, not only paths.

### Preserve Community functionality

Do not regress or paywall:

```text
local project creation and discovery
Codex authentication/readiness/launch/resume
agents and session lifecycle
PTYs and terminal setup
Git and project metadata
local SQLite persistence and session history
settings and Codex home
SSH config parsing and host discovery
Remote Project -> SSH host flow
remote project browser
remote Linux/macOS ditchd bootstrap
remote Codex execution
SSH password/credential Keychain access
Community updates
Commercial discovery and upgrade bootstrap
```

Remote SSH execution must never require a Commercial entitlement.

### Keep only Community-safe Commercial bootstrap

Community may contain:

```text
installation identity and proof
provider-neutral offer/catalog models
provider-neutral entitlement summary
checkout creation through Ditch Relay
hosted checkout URL opening
license redemption through Ditch Relay
billing-management URL opening
authorized Commercial release discovery
signed descriptor verification
Sparkle public verification configuration
Commercial installation/retry/progress UI
capability discovery metadata and restrained upsell copy
```

It must not contain Stripe keys, Stripe IDs, Stripe SDKs, billing-provider
secrets, proprietary capabilities, or Remote Control implementation.

The public capability registry may describe discoverable concepts such as
`remote_control`, `rag`, `grok`, `agentic_bots`, and integrations. Descriptive
metadata is allowed; implementation is not.

### Fix private documentation placement

The root Community working tree currently includes implementation prompts and
architecture material added during private development. Review every untracked
or modified document. Move private implementation/security/protocol/release
operator material into `commercial/docs` when it is still useful. Community
may retain public architecture, contribution, upgrade-client, SSH, and
security-boundary documentation.

In particular, do not leave a Relay Commercial release-delivery implementation
prompt in the future public Community tree merely because it is Markdown.

## Phase 3 — Finalize the reusable single-runtime seam

Community owns the authoritative runtime implementation in:

```text
crates/ditchd/src/runtime_shared.rs
```

Keep its edition seam narrow and statically composed. The existing conceptual
hooks are appropriate:

```text
edition::State
edition::initialize_store
edition::extend_runtime_capabilities
edition::after_broadcast
edition::start
edition::handle_request
```

Community `edition.rs` must:

- have no private dependencies;
- use a zero-sized/no-op Community state where appropriate;
- add no private runtime capabilities;
- initialize no private store schema;
- start no Relay/Remote Control worker;
- return a normal unsupported-request response for private requests that can
  only exist in Commercial's extended protocol.

Commercial `edition.rs` must:

- statically own proprietary provider state;
- initialize private additive store migrations;
- extend capabilities from provider-neutral entitlement state;
- start private services inside the one `ditchd` process;
- route private requests to private providers;
- keep `ditchd` the single authority for sessions, PTYs, projects and stores.

Commercial may include the Community shared runtime source at compile time, but
must not copy it or maintain a divergent fork.

Add a compile-time or test-time composition contract so incompatible changes to
Community runtime hooks fail Commercial CI immediately.

## Phase 4 — Finalize protocol ownership

Community protocol contains only local, SSH and upgrade-bootstrap contracts.

Commercial owns the extended private protocol used by Remote Control. Ensure
the Commercial runtime can compile `runtime_shared.rs` against its extended
protocol while Community compiles the same runtime against the public protocol.

Requirements:

- no full mobile Remote Protocol contract in Community;
- no provider-specific billing fields in either public client contract;
- Relay/iPhone contract export remains private and deterministic;
- no hand-maintained duplicate private schemas;
- do not bump the private protocol major version unless wire incompatibility
  actually requires it;
- preserve existing private cryptography and tests unless extraction exposes a
  demonstrated bug.

## Phase 5 — Finalize store ownership and state compatibility

Community store owns shared state:

```text
projects
local and SSH project registrations
sessions/history
settings
shared runtime metadata
Community schema migrations
```

Commercial store extension owns private state:

```text
Remote Control machine/device records
pairing state
projection state
private command/replay metadata
future proprietary capability tables
```

Requirements:

1. Both editions use the same canonical Application Support directory and
   SQLite database path.
2. Commercial opens Community state directly and applies additive private
   migrations.
3. Community can later open a database previously used by Commercial without
   reading, mutating, migrating backward, or deleting unknown private tables.
4. Subscription expiry never deletes shared or private state.
5. Renewal re-enables capabilities without reinstalling when no normal update
   is needed.
6. Project IDs, session IDs, SSH aliases, Codex state, worktrees and settings
   remain unchanged across edition replacement.

Add integration tests for:

```text
Community DB -> Commercial -> same shared state
Commercial DB -> Community -> shared state works, private tables preserved
expired Commercial -> local/SSH work, Remote Control unavailable
renewed Commercial -> private capability restored
```

## Phase 6 — Finalize Flutter composition

Community owns the main desktop application and a narrow edition-surface seam.

Community UI includes:

- all local and SSH workflows;
- contextual Commercial discovery;
- Relay-provided dynamic offers and introductory pricing;
- checkout pending/failed/completed progress;
- license redemption;
- secure Commercial install and retry status;
- subscription-expired messaging that explicitly preserves local/SSH use.

Community UI must not include pairing, phone/device roster, Remote Control
status, relay connection state, mobile projections or private device commands.

Commercial supplies the proprietary surface from private Dart source, such as
the existing `CommercialEditionSurface`, without maintaining a permanent patch
stack against Community `main.dart`.

Verify there is no conditional import in Community that references an absent
private file. A clean Community checkout must pass Flutter analysis/build with
no neighboring Commercial repository.

Ensure active subscribers do not see ordinary acquisition cards. Only
Relay-authorized renewal, upgrade or add-capacity actions may appear.

## Phase 7 — Complete Community-to-Commercial in-place upgrade

Both editions use the confirmed identity:

```text
The Ditch.app
ai.theditch.app
Apple Team HH3KSCHUQF
```

They must also share:

- Application Support paths;
- preferences domain;
- Keychain services/access expectations;
- installation identity location;
- project/store paths;
- runtime login-item identity where compatible.

Community installation flow:

```text
Community Settings
-> Relay-provided offer or Ditch license
-> hosted checkout in browser
-> Relay webhook/entitlement becomes authoritative
-> Community requests authorized Commercial release
-> verify signed descriptor and compatibility
-> obtain short-lived authenticated Sparkle session
-> defer while agents are active
-> Sparkle atomically replaces The Ditch.app
-> restart
-> one Commercial ditchd opens the same state
```

Never treat browser return as payment proof.

Verify all of:

- staging requests `channel=beta`;
- production requests `channel=stable`;
- Community and Commercial embed the corresponding environment's Sparkle
  public key and P-256 release-manifest public key;
- staging and production trust/Relay/R2 configuration never cross;
- descriptor release ID matches Relay release ID;
- authorization bearer is required and short-lived;
- artifact and appcast URLs use the configured Relay host;
- digest, size, edition, bundle ID, Team ID, minimum build/version, signature,
  expiry and rollback sequence are checked;
- active agents cause a visible deferred state, not termination;
- failed installation leaves Community intact and offers a meaningful retry;
- successful replacement restarts into Commercial without side-by-side apps.

Community source-built users must be able to use the same authorized upgrade
path when the source build contains official public verification values.

## Phase 8 — Preserve SSH and one runtime on remote hosts

Community remote artifacts must contain the Community remote runtime for all
officially supported Linux/macOS architectures.

Commercial may package matching Commercial remote runtimes when proprietary
direct iPhone-to-remote-host capability requires them.

On every host:

```text
~/.ditch/bin/ditchd
```

is the only runtime. Never install a second services daemon.

Remote replacement must be atomic and deferred when active agents make it
unsafe. Preserve remote projects, SQLite state, sessions, Git/worktrees and
`~/.codex`.

## Phase 9 — Strengthen leakage and reproducibility gates

Community leakage checks are release-blocking but are defense in depth, not the
source boundary itself.

Inspect:

- tracked and untracked Community source;
- Cargo metadata and lockfile;
- Dart imports and generated plugin/build metadata;
- source archives;
- compiled `ditchd` and CLI strings/symbols;
- built application contents and linked libraries;
- packaged Community DMG;
- docs/contracts/scripts that expose private implementation;
- exactly one bundled local `ditchd`.

The gate must not prohibit legitimate public words such as “Remote Control” in
upsell metadata. It should prohibit implementation paths, protocol symbols,
private schemas, Relay control clients and proprietary dependencies.

Avoid a self-matching gate: forbidden test strings may need deliberate string
construction or exclusions so the scanner does not fail on its own source.

Add a deterministic clean-checkout test proving Community needs no private path,
Git dependency, registry, secret, or neighboring repository.

Commercial gates must prove:

- submodule HEAD equals `COMMUNITY_REVISION`;
- submodule and Commercial worktrees are clean;
- Community revision is an exact commit;
- no dirty-release override exists;
- one Commercial `ditchd` is packaged;
- private providers are compiled into that one runtime;
- Community local/SSH behavior remains functional.

## Phase 10 — Licensing and contribution documentation

Preserve root workspace license:

```text
AGPL-3.0-only
```

Verify Community contribution documentation states:

- Community remains AGPL-3.0-only;
- issues, design discussion, test cases, and suggested patches are welcome;
- external source cannot be merged into release branches until the approved
  no-separate-CLA inbound licensing and sign-off workflow is published;
- final contribution-licensing language requires legal review;
- a DCO by itself is not equivalent to proprietary dual-licensing permission;
- private Commercial implementation must not enter Community PRs.

Do not invent binding CLA language or claim legal compliance.

## Phase 11 — Historical source leakage blocker

The current private `DitchNow/Ditch` Git history contains proprietary Remote
Control source. Removing it from the tip does not remove it from old commits.

Therefore:

1. Do not make the existing remote repository public during this task.
2. Do not push old private branches/tags into a future public repository.
3. Do not rewrite or force-push history automatically.
4. Document a manual pre-publication procedure requiring maintainer review and
   backup. Acceptable later approaches include a fresh public repository/history
   created from the verified clean Community tree, or a reviewed history-filter
   operation.
5. The future public Git history must not make private Remote Control source,
   protocol, crypto, keys, fixtures or security documents recoverable.
6. After clean-history publication, Commercial must pin the exact public
   Community commit actually available to builders/CI.

This is a release/legal/manual blocker, not permission for the implementation
agent to rewrite history.

## Phase 12 — Test matrix

### Community Rust

Run from the root Community repository:

```sh
cargo build --locked -p ditchd -p ditch_cli
cargo test --locked --workspace
cargo clippy --workspace --all-targets
scripts/check-community-leakage
```

Add/fix focused tests proving:

- local project creation;
- local Codex/PTY/session behavior;
- SSH project creation and execution;
- installation identity stability;
- Community composition exposes no private runtime capability;
- private protocol requests/types are absent;
- unknown Commercial tables survive Community access;
- offer/license/checkout client uses only Relay;
- wrong release signature/hash/bundle/team/channel/downgrade/expiry fails;
- active agents defer upgrade.

### Community Flutter/macOS

Run:

```sh
cd apps/macos
flutter analyze
flutter test
flutter build macos
```

Prove:

- local and SSH UI remains;
- Commercial discovery/upgrade UI remains;
- no pairing/device/Remote Control implementation is present;
- dynamic offers render without hardcoded prices;
- pending/failed checkout remains retryable;
- staging/production environment display and channel are correct;
- Community app contains one `ditchd`.

### Commercial

Run from `commercial` after pinning the clean Community commit:

```sh
scripts/verify-community-pin
cargo test --locked --workspace
cargo clippy --workspace --all-targets
cd apps/macos
flutter analyze
flutter test
flutter build macos
```

Prove:

- private capability registry composes Remote Control;
- local and SSH Community behavior still passes;
- same Community DB opens;
- same installation identity is reused;
- exactly one Commercial `ditchd` is packaged;
- private store migrations are additive;
- expiry disables private capability only;
- renewal restores it;
- future capability descriptors/providers can be added without another daemon.

### Cross-edition replacement

Add an automated fixture/integration test for:

```text
Community state with local project + SSH project + sessions + identity
-> Commercial build opens it
-> IDs/data/identity unchanged
-> private capability becomes available when entitled
-> expiry disables private capability
-> local and SSH continue
```

Where full signed/notarized Sparkle automation cannot run in unit tests, test
the contracts deterministically and document the manual staging UAT.

### Release preflight

Run Community and Commercial staging preflight where local signing, Relay,
Cloudflare and toolchain prerequisites allow it. Do not bypass a failed
preflight. Report external configuration blockers precisely.

Do not deploy or publish.

## Phase 13 — Commit and pin safely

Only after the Community source/build/test/leakage gates pass:

1. Review the root diff and ensure no Commercial private implementation remains.
2. Create one coherent local Community commit on the intended development
   branch with a message describing the Community/Commercial boundary.
3. Record its full 40-character commit hash.
4. Ensure all useful changes from `commercial/community` have already been
   integrated into root Community or private Commercial.
5. Preserve remaining submodule work recoverably, then make
   `commercial/community` a clean checkout of the new Community commit.
6. Update `commercial/COMMUNITY_REVISION` to exactly that hash.
7. Verify the Commercial Git submodule entry records the same hash.
8. Run `scripts/verify-community-pin` and the complete Commercial test matrix.
9. Review the Commercial diff and create one coherent local Commercial commit.
10. Verify:

    ```text
    root Community status: clean
    Commercial status: clean
    commercial/community status: clean
    submodule HEAD == COMMUNITY_REVISION == Commercial gitlink
    ```

Do not amend unrelated historical commits. Do not push either commit.

If tests cannot pass, do not create “successful” final commits merely to make
the tree clean. Report the blocker and preserve work safely.

## Immediate Commercial-to-Commercial staging goal

After the source boundary is clean and pinned, the code should be ready for the
separately authorized staging sequence:

```text
install signed/notarized Commercial 1.0.1+101
-> active staging entitlement
-> publish authorized Commercial 1.0.2+102
-> notification bell offers update
-> click Install
-> Relay issues fresh beta update session
-> Sparkle replaces app
-> restart on 1.0.2+102
-> one ditchd
-> unchanged projects/sessions/SSH/identity
```

This task may prepare and test the code and preflight, but must not publish the
artifacts or deploy Relay without separate authorization.

## Do not do these things

Do not:

- create a second Community repository by manually masking Commercial files;
- maintain two unrelated runtime forks;
- leave proprietary code in Community behind a feature flag or boolean;
- create a permanent patch stack against Flutter `main.dart`;
- copy local Codex credentials into Relay/licensing;
- require an online account for ordinary Community local/SSH use;
- remove or paywall SSH;
- create Community and Commercial data directories/databases;
- change Community away from AGPL-3.0-only;
- publish old Git history containing proprietary source;
- use dirty-release overrides;
- create `ditchd + ditch-servicesd`;
- kill active local or remote agents for an upgrade;
- push, deploy, publish, upload, rewrite history, or change visibility.

## Definition of done

The implementation is complete only when:

1. Root Community builds independently without Commercial/private source.
2. Root Community contains local and SSH functionality.
3. Root Community contains no proprietary Remote Control implementation or
   private mobile contract.
4. Community UI contains provider-neutral discovery/purchase/install bootstrap.
5. Community `ditchd` is the shared authoritative runtime with no-op edition
   hooks.
6. Commercial consumes the exact clean Community commit.
7. Commercial statically composes private providers into the same runtime.
8. Both editions package exactly one `ditchd`.
9. Community can securely replace itself with Commercial using the same bundle
   and state identity.
10. State and installation identity survive replacement.
11. Subscription expiry preserves local/SSH functionality and data.
12. Staging uses beta; production uses stable.
13. Leakage and clean-pin gates pass.
14. Community and Commercial tests/builds pass.
15. Approved local Community and Commercial commits exist and all three
    worktrees are clean.
16. Nothing was pushed, deployed or published.
17. Historical source leakage remains explicitly blocked from public release
    until a reviewed clean-history publication is performed.

## Final report

Report only verified facts:

1. Architecture discovered and corrected.
2. Community files removed/moved/refactored.
3. Proprietary Commercial files and composition.
4. Shared runtime seam and single-`ditchd` proof.
5. Protocol and store ownership.
6. SSH preservation.
7. Community upgrade/bootstrap behavior.
8. Bundle, Team, identity and state compatibility.
9. Staging/production channel behavior.
10. Leakage/reproducibility gates.
11. Tests/commands run and actual results.
12. Community commit hash.
13. `COMMUNITY_REVISION`, submodule HEAD and Commercial gitlink comparison.
14. Commercial commit hash.
15. Final status of all worktrees.
16. Any remaining manual staging configuration/UAT.
17. Historical Git cleanup/publication blocker.
18. Remaining legal/manual release work.

Do not claim legal compliance, deployment, publication, notarized upgrade
success, or production readiness without direct evidence.
