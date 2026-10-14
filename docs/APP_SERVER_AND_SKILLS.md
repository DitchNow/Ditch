# Local App Server and Skills Directory

The integration uses a shared local and SSH Codex App Server transport and a federated Skills
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

New desktop sessions default to **Use App Server**. Explicit legacy execution
remains available for local sessions without skills. Task acceptance requires
App Server; it never silently switches transport. Imported threads keep their
recorded skills. Start a fresh thread to attach a different skill set.

| Execution choice (local or SSH) | Approval policy | Sandbox |
| --- | --- | --- |
| Ask | `on-request` | `workspaceWrite`, canonical project root |
| Approve for me | `never` | Same workspace-write boundary |
| Full Access | `never` | `dangerFullAccess`, explicit unrestricted access |

Workspace-write permits network access and excludes implicit `/tmp` and
`TMPDIR` writable roots. SSH hosts must support the selected sandbox; failure
is surfaced without silently removing containment. SSH reservations track each agent independently. Approvals that expand access
record the changed scope without excluding other user agents. Coordinator-owned
work retains one active worker per group and project. Denial and cancellation remain available.
Local acceptance tasks additionally protect Ditch state, configuration, and
application files with a named restricted permission profile.

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
and a real SSH host sync/approval walkthrough remain release validation gates for the combined integration. Third-party skill installation and release
publishing are separate user actions.

Protocol references: [Codex App Server](https://learn.chatgpt.com/docs/app-server),
[Codex skills](https://learn.chatgpt.com/docs/build-skills), and the seeded
[Superpowers source](https://github.com/obra/superpowers). The installed CLI's
generated schema and capability results govern field compatibility.
