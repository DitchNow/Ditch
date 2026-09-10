# Local App Server and Skills Directory

Phase 2 adds an opt-in local Codex App Server transport and a federated Skills
directory. The daemon owns processes, approvals, writer reservations, catalog
metadata, task bindings, and attempt provenance. Community contains the shared
implementation; Commercial composes it without a separate task or skill store.

## Compatibility and execution

The installed Codex CLI used for protocol and containment verification was
**0.153.4**. This is the tested version, not a promise that every earlier version
is incompatible. Runtime initialization probes compatibility. The Skills
capability action additionally probes `skills/extraRoots/set` and a harmless
`command/exec` under workspace-write containment. Installation and discovery
must succeed through `skills/list` before a managed skill becomes usable.

This CLI exposes process-local `skills/extraRoots/set`; it does not expose the
newer per-CWD extra-roots field. Each discovery/turn process receives only its
selected roots. No global configuration change is needed for managed skills.
Explicitly toggling a Codex-owned user/repository skill uses Codex's supported
`skills/config/write` operation.

Enable **Use local App Server** when creating an agent or starting a task.
Legacy remains the default pending live end-to-end parity validation. A failed
preflight falls back to legacy execution only without selected skills, and
writes a visible transcript warning that interactive approvals and explicit
skills are unavailable. A failure after launch is an error rather than a second
launch. Imported threads cannot acquire new skills; start a fresh thread.

| Execution choice | Approval policy | Sandbox / writer reservation |
| --- | --- | --- |
| Local Ask | `untrusted` | `workspaceWrite`, canonical project root only |
| Local Approve for me | `never` | Same workspace-write boundary |
| Local Full Access | `never` | `dangerFullAccess`, global exclusive writer |
| SSH | `untrusted` | Guarded but unconfined; SSH account is the boundary |

Workspace-write permits network access and excludes implicit `/tmp` and
`TMPDIR` writable roots. Full Access is an explicit choice. An interactive
approval that could expand access reserves global exclusive writer ownership
before delivery; competing writers block approval. Denial remains available.
SSH stays unconfined in this release even if a capability probe succeeds; the
UI reports this limitation. Approvals are not filesystem containment.

App Server turns persist thread IDs, titles, selected model/effort, exact skill
paths/hashes, transcript and completion state. Assistant deltas are transient;
completed messages are durable. Duplicate thread responses and stale
thread/turn events are filtered. Process groups participate in the Phase 1
writer lifecycle, stop/kill handling, and restart reconciliation.

## Catalog and assignment

Open **Skills**, select a project, then use Installed or Available. Discovery
uses the selected Codex installation and CODEX_HOME, project skills, explicit
local collection roots, and Ditch-managed revisions. Superpowers is a seeded
source URL, not bundled runtime code or an automatically installed dependency.
Source browsing and fetching require explicit network consent.

An install/update has a reviewable plan containing source, requested ref,
resolved revision when known, license, included paths, script warning, content
hash, and destination. Confirmation promotes a validated staged revision into
the daemon's application-support `skills/versions` directory. Cancellation
discards the staged plan. No setup scripts, hooks, filters, submodules, or skill
executables run during installation. Git sources are inspected as blobs without
checking out their working tree. Unknown license/provenance stays unknown.

Validation rejects traversal, absolute paths, symlinks, special files, duplicate
paths, invalid manifests, and excessive payloads. Limits include 512 files,
1 MiB per file, 8 MiB per selected skill, 64 KiB main instructions, 16 directory
levels, bounded source output, a 45-second Git timeout, and a 64 MiB temporary
Git object budget. App Server provides final semantic recognition.

Updates retain previous immutable revisions. Rollback validates the retained
revision before selecting it. An interrupted promotion can be retried; it does
not replace the prior catalog selection before recognition succeeds. Remove
blocks while task or recorded session bindings reference that skill. Disable
excludes managed roots from subsequent execution.

Bindings are ordered and persisted with identity, canonical path, hash,
revision, user origin, and timestamp. The reusable picker appears in New Agent,
the task editor, and task start/review. More than three skills requires an
explicit warning override. Changing bytes after selection blocks launch;
deselect/reselect a valid discovered revision to accept it. Modified managed
revisions must be restored or explicitly updated. Missing/disabled skills and
unresolved dependencies also block launch. Existing threads keep their recorded
skill set. Each new turn supplies explicit `skill` input items, not name-only
instructions. Agent chips come from recorded launch metadata.

SSH discovery uses the remote runtime. Explicit Sync transfers only the chosen
managed revision, retains available provenance, verifies the checksum, and
requires remote App Server recognition. It never substitutes a same-named
remote skill. Declining sync leaves the local skill unavailable remotely.

Catalog metadata lives in Community SQLite app settings; task bindings and
attempt profiles use the existing durable JSON records with additive defaults.
The typed protocol adds paginated skill operations and change notifications.
No new database migration or parallel persistence authority is introduced.

## Verification and remaining live checks

Automated coverage includes fixture App Server start/resume/stop, explicit
inputs, exit races, event ordering, fallback, policy mapping, catalog discovery,
selective install/update/rollback/remove, interrupted promotion, binding
restart/concurrency, hostile files, dependency/hash errors, and remote import
checksum validation. Flutter tests exercise catalog rendering, pagination,
selection, unavailable skills, override warnings, and install confirmation.

The opt-in ignored Rust test
`installed_codex_recognizes_managed_fixture_and_enforces_workspace` verifies a
temporary managed fixture against the actual CLI, without changing the user's
global config. It checks assigned-root writes and denial outside the root.
Run it with `DITCH_TEST_CODEX` pointing to the installed CLI and `--ignored`.

Live authenticated model turns, interactive desktop approval walkthroughs,
and a real SSH host sync/approval walkthrough remain manual validation before
making App Server the default. Third-party skill installation and release
publishing are separate user actions.

Protocol references: [Codex App Server](https://learn.chatgpt.com/docs/app-server),
[Codex skills](https://learn.chatgpt.com/docs/build-skills), and the seeded
[Superpowers source](https://github.com/obra/superpowers). The installed CLI's
generated schema and capability results govern field compatibility.
