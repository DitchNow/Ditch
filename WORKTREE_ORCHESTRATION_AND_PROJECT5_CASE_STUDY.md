# The Ditch Git Worktree Orchestration

## Comprehensive implementation overview and Project#5 case study

**Document date:** 2026-08-25  
**The Ditch repository reviewed:** /Users/mtn/Documents/Personal/The Ditch v2  
**Case-study project:** /Users/mtn/Documents/Personal/SentinelProjects/Project#5

This document is intended as a self-contained handoff to an external reviewer. It covers the product goal, architectural boundaries, code structure, persisted model, complete worktree lifecycle, Git algorithms, validation system, UI behavior, recovery and safety controls, automated tests, and the exact failure observed in Project#5.

The implementation described here is the current local working-tree implementation and may include changes not yet committed. Project#5 facts were verified read-only against its Git repository and The Ditch SQLite database on 2026-08-25. No Project#5 files, refs, worktrees, or database records were changed during the investigation.

---

## 1. Executive summary

The Ditch is a local macOS application for running multiple Codex coding agents against the same software project. Its intended promise is:

> Run several agents on one project without making Git the user's job.

The implementation uses real Git linked worktrees. Every serious Git-backed agent receives an independent physical checkout, deterministic Ditch-owned branch, immutable base commit, and a Codex process whose current working directory is inside that checkout. The Rust daemon, ditchd, is the sole authority for Git mutations. Flutter is a projection and control surface.

The current subsystem already provides substantial safety:

- one locked linked worktree and branch per agent;
- immutable base and target OIDs;
- Codex PTY ownership rooted in the correct agent directory;
- NUL-delimited, machine-readable Git parsing;
- repository-scoped Git mutation serialization;
- live actual changed-path tracking;
- path and intent overlap projection;
- non-destructive checkpoint commits through a temporary index;
- real Git merge simulation through git merge-tree;
- internal two-parent integration candidates;
- combined-result validation in detached worktrees;
- compare-and-swap target verification before apply;
- dirty canonical-workspace protection;
- isolated manual conflict-resolution worktrees;
- SQLite journaling and startup recovery;
- Ditch ownership verification before cleanup.

Project#5 exposed a major gap between Git safety and completion UX:

1. Two agents were given effectively the same website-building task.
2. They independently generated nearly the same application.
3. Their files were safely isolated and preserved.
4. Git merge simulation succeeded for each candidate against the unchanged project.
5. Combined validation attempted npm run lint.
6. The validation worktree did not have node_modules.
7. ESLint existed in each original agent worktree but not in the canonical project directory.
8. The validation runner only reused dependencies from the canonical project directory.
9. Both checks failed with exit code 127 and “eslint: command not found.”
10. This was an environment/setup failure, not a lint result from inspecting the source.
11. The UI classified it as ValidationFailed and hid Apply.
12. Check again and Retry checks execute the same operation and repeat the same failure.
13. The canonical Project#5 folder stayed nearly empty because neither candidate was applied.
14. Both complete websites remain recoverable in Ditch checkpoint and candidate refs.

The central conclusion is:

> The subsystem protects Git state better than it guides the user to a completed outcome. It needs first-class distinction between code failures, environment failures, unavailable checks, duplicate work, Git conflicts, and user decisions.

---

## 2. Product goal and safety contract

For each Git-backed agent session, The Ditch conceptually owns:

~~~text
1 AgentRun
1 ManagedWorktree record
1 Ditch-owned branch
1 locked linked worktree
1 immutable base commit OID
1 target branch and target OID-at-start
1 Codex child process / PTY rooted in that worktree
0 or 1 internal checkpoint refs
0 or 1 internal integration-candidate refs
0 or 1 isolated conflict-resolution worktrees
~~~

The truth hierarchy is:

~~~text
Agent prompt / semantic signal    advisory intent
Filesystem activity              fast signal
git status / git diff             actual changed files
git merge-tree                    Git merge compatibility
tests / lint / build              combined software validation
SQLite                            durable Ditch lifecycle state
Flutter                           projection and control surface
~~~

The realistic guarantee is not that arbitrary software conflicts are impossible. It is:

> Agents cannot overwrite one another's working files, unsafe changes are not silently integrated, the canonical project is not overwritten while dirty, and isolated work remains recoverable.

Worktrees solve physical file isolation. They do not eliminate logical, Git, or semantic conflicts.

---

## 3. High-level architecture

~~~text
Flutter macOS application
    |
    | JSON protocol commands and sequenced events
    v
ditchd Rust daemon
    |
    +-- RuntimeState
    |     projects
    |     agents
    |     child processes / PTYs
    |     managed worktrees
    |     subscribers / attention
    |
    +-- GitCoordinator
    |     launch snapshots and save points
    |     worktree lifecycle
    |     changed-path reconciliation
    |     overlap projection
    |     checkpoints
    |     merge simulation
    |     validation worktrees
    |     apply / discard / archive
    |     conflict-resolution worktrees
    |
    +-- DitchStore / SQLite
    |     projects / agents / messages
    |     managed_worktrees JSON
    |     project_git_operations
    |     git_lifecycle_events
    |
    v
Local Git repository and linked worktrees
~~~

Flutter does not independently create or remove worktrees, create or delete branches, merge, rebase, update refs, decide integration safety, or manage Git locks.

---

## 4. Code structure

### 4.1 Core domain model

**File:** crates/ditch_core/src/lib.rs

Important types:

- **Project**
  - project ID, name, root, creation/archive times;
  - Git policy;
  - integration policy.
- **ManagedWorktree**
  - project/session/worktree ownership IDs;
  - physical path and agent CWD;
  - branch, base branch, target branch;
  - immutable base commit and target OID-at-start;
  - current head;
  - checkpoint and candidate OIDs;
  - resolver path/branch/target;
  - state, lock state, dirty state;
  - changed paths and semantic intent;
  - overlap projection;
  - conflict, integration, and validation states;
  - active operation and last error;
  - reconciliation timestamps.
- **ProjectGitOperation**
  - journals snapshot/save-point flows crossing SQLite and Git;
  - stores expected tree, expected old target OID, created commit, state, and error.
- **WorktreeReview**
  - changes relative to the agent starting point;
  - combined changes relative to the target used for validation;
  - candidate staleness.
- **ValidationCheck**
  - check name, pass/fail, exit code, bounded output.

Persisted worktree states:

~~~text
Creating
Ready
Active
Dirty
Waiting
Finished
NeedsReview
QueuedForIntegration
CheckingMerge
ConflictRisk
Validating
ReadyToApply
Applying
Integrated
Discarded
CleanupPending
Orphaned
RecoveryNeeded
Failed
~~~

Independent dimensions:

~~~text
ConflictState:
  None
  IntentOverlap
  PathOverlap
  GitConflict
  ValidationFailed

OverlapRisk:
  None
  Intent
  Path

IntegrationState:
  NotRequested
  Queued
  Checking
  Conflict
  Validating
  Ready
  Applying
  Applied
  Blocked

ValidationState:
  NotRun
  Running
  Passed
  Failed
  NotConfigured

IntegrationPolicy:
  ReviewBeforeApply
  AutoApplyAfterValidation
~~~

AutoApplyAfterValidation is the domain default.

### 4.2 Git coordinator

**File:** crates/ditchd/src/git_orchestration.rs

This is the Git control plane.

| Method | Responsibility |
|---|---|
| inspect_initial_snapshot | Compare canonical visible files with the current Git tree and identify preparation hazards. |
| create_initial_snapshot | Create an initial commit or later Ditch save-point commit through a temporary index and CAS ref update. |
| reconcile_initial_snapshot | Finish or quarantine interrupted snapshot operations. |
| create_journaled | Create a deterministic branch and locked linked worktree. |
| refresh | Reconcile persisted worktree state with actual Git state. |
| reconcile_operation | Recover interrupted lifecycle operations. |
| checkpoint | Capture the complete agent result without changing its normal index. |
| prepare_integration | Merge-simulate, create candidate, and run combined validation. |
| review | Produce machine-readable agent and combined changed-file lists. |
| apply | CAS-check and safely update the target/canonical checkout. |
| create_conflict_resolution | Materialize a real conflict in an isolated resolver worktree. |
| finalize_conflict_resolution | Validate a resolved candidate. |
| discard | Explicitly discard Ditch-owned work after confirmation. |
| archive_integrated | Remove the physical worktree after successful integration. |

The coordinator lock registry is keyed by canonical repository root. Two Ditch projects inside one Git repository therefore share the Git control-plane lock.

### 4.3 Daemon orchestration

**File:** crates/ditchd/src/main.rs

Key functions:

- start_codex_session
- resume_codex_session
- prepare_project_snapshot_for_launch
- create_journaled_worktree
- monitor_worktree
- refresh_worktree_inner
- finish_agent
- prepare_integration
- apply_integration
- create_conflict_resolution
- finalize_conflict_resolution
- discard_worktree

ditchd owns the Codex child process. It starts Codex with cwd equal to the managed agent directory.

The prompt is extended with a boundary telling Codex to:

- treat the current directory as the complete project;
- remain on the Ditch branch;
- avoid merge/rebase/worktree operations;
- avoid the canonical project and other worktrees;
- avoid force-resetting shared refs;
- not instruct the user to repair Ditch-owned Git infrastructure.

### 4.4 Protocol

**File:** crates/ditch_protocol/src/lib.rs

Relevant commands:

~~~text
InspectProjectAgentReadiness
CreateInitialProjectSnapshot
StartCodexSession
ResumeCodexSession
RegisterChangeIntent
RefreshWorktree
OverrideWorktreeOverlap
GetWorktreeReview
PrepareIntegration
ApplyIntegration
ApplyIntegrationWithoutValidation
CreateConflictResolution
FinalizeConflictResolution
DiscardWorktree
SetProjectIntegrationPolicy
~~~

The snapshot/event protocol includes managed worktree projections. WorktreeChanged events update Flutter without letting Flutter become a Git authority.

### 4.5 Persistence

**File:** crates/ditch_store/src/lib.rs

SQLite schema version is currently 4.

Important tables:

- projects
- agents
- agent_messages
- managed_worktrees
- project_git_operations
- git_lifecycle_events
- tasks
- attention and permission tables
- quarantined message and settings tables

managed_worktrees stores indexed identity/status fields plus the complete serialized ManagedWorktree JSON.

project_git_operations journals non-atomic SQLite/Git preparation operations.

git_lifecycle_events records structured events such as:

~~~text
worktree_create_started
worktree_created
worktree_checkpointed
integration_queued
integration_conflict_detected
validation_passed
validation_failed
integration_applied
worktree_removed
initial_snapshot_state_changed
~~~

### 4.6 Flutter UI

**File:** apps/macos/lib/main.dart

Important UI/data classes:

- DitchProject
- AgentSession
- WorktreeStatusChip
- WorktreeReviewDialog
- DitchRuntimeClient

Relevant controls:

- Review changes
- Check again
- Apply
- Keep separate
- Retry checks
- Apply without checks
- Resolve manually
- Check resolution
- Discard preserved workspace

Advanced tooltips may expose branch, worktree path, base/head/target OIDs, validation state, resolver path, and overlap paths.

---

## 5. Project preparation and seamless launch

### 5.1 Why preparation exists

A linked worktree must be materialized from a Git commit tree. It cannot directly inherit arbitrary uncommitted files from another working directory.

An earlier bug treated any existing HEAD as ready. If HEAD was empty but application files were later created only in the canonical directory, the agent received an empty checkout. Codex correctly reported that it saw only metadata.

### 5.2 Current preparation algorithm

Before each launch, ditchd:

1. Finds repository root and symbolic target branch.
2. Reads changed paths through status porcelain v2 with NUL delimiters.
3. Excludes .ditch metadata from user-change decisions.
4. Detects merge, cherry-pick, revert, rebase, and bisect state.
5. Builds the complete proposed tree through a temporary index.
6. Excludes Ditch metadata.
7. detects credential-like names, nested repositories, non-UTF-8 paths, and very large files.
8. Compares the proposed tree with HEAD tree even if HEAD exists.
9. If safe and different, creates a local Ditch save-point commit with current HEAD as parent.
10. Updates the target with expected-old-OID protection.
11. Aligns the index through git read-tree --reset. This changes the index, not file contents.
12. Re-inspects to catch concurrent file changes.
13. Creates the worktree from the exact prepared commit.
14. Verifies the worktree HEAD, tree OID, and agent directory before starting Codex.

Safe preparation is automatic. Suspected credentials or staged manual Git work require review. Active Git operations block preparation.

### 5.3 Preparation recovery

Operation states:

~~~text
Preparing
CommitCreated
RefUpdated
Completed
RecoveryNeeded
Failed
~~~

The journal stores the expected previous target OID. Startup reconciliation can:

- recognize a ref already updated to the created commit;
- finish a CAS update if the target still equals the expected old OID;
- align the index after a crash;
- stop when external Git activity moved the target;
- mark ambiguous state for recovery without overwriting it.

---

## 6. Worktree and Codex lifecycle

~~~text
User submits task
    ↓
ditchd verifies project and Codex policy
    ↓
prepare complete launch commit
    ↓
DB worktree state = Creating
    ↓
git worktree add --lock --reason ... -b ditch/agent/... path base-OID
    ↓
verify checkout
    ↓
DB state = Ready
    ↓
spawn Codex with cwd = agent_cwd
    ↓
monitor actual Git changes
    ↓
DB state = Active / Dirty
~~~

Branch format:

~~~text
ditch/agent/full-session-uuid/sanitized-task-slug
~~~

Physical worktrees live below The Ditch application-support data directory and are grouped by project/session.

An agent does not have to create commits. Its visible Ditch branch may remain at the base commit while the worktree contains uncommitted changes. Ditch later snapshots those changes into an internal checkpoint ref. Therefore, a branch still pointing at the base is not evidence that work was lost.

---

## 7. Live change tracking and overlap

A background monitor refreshes an active worktree approximately every 1.5 seconds.

Actual changed paths come from machine-readable status and cover:

- tracked modifications;
- staged and unstaged changes;
- untracked files;
- deletions;
- rename original and destination;
- unmerged records.

Paths are repository-relative and normalized. Actual path equality is case-insensitive; intent pattern matching uses component-aware directory semantics.

Intent is currently inferred mostly from path-looking tokens in the prompt. A natural-language prompt with no file paths produces a summary but empty expected paths. intents_overlap compares expected/shared paths, not semantic similarity of summaries.

apply_overlap_projection records:

- overlapping session IDs;
- exact overlapping paths;
- Intent or Path risk.

Important limitation:

> Current overlap projection is advisory. It records risk but does not queue, pause, or stop active agents. A Waiting state is reset to Active or Dirty during projection. Integration remains guarded, but admission control is not fully implemented.

This is why duplicate Project#5 agents both continued.

---

## 8. Non-destructive checkpointing

When an agent finishes, Ditch:

1. Refreshes actual state.
2. Creates a temporary index file.
3. Loads the agent worktree HEAD into that index.
4. Runs git add -A against the temporary index.
5. Writes a tree.
6. Creates a commit whose parent is the worktree HEAD.
7. Updates refs/ditch/checkpoints/session-id with expected-old protection.
8. Removes the temporary index.

Checkpoint message:

~~~text
Ditch checkpoint for session session-id
~~~

This captures committed, staged, unstaged, deleted, and non-ignored untracked task work without changing the agent's normal index or files.

---

## 9. Merge simulation and candidate creation

For a completed task:

1. Resolve the current target ref OID.
2. Run real Git merge computation:

~~~text
git merge-tree --write-tree --messages --name-only -z
    current-target checkpoint
~~~

3. Use the command exit status.
4. Exit code 1 becomes GitConflict.
5. A clean result yields a tree OID.
6. Create a two-parent internal candidate commit:

~~~text
tree     = merge-tree result
parent 1 = current target
parent 2 = agent checkpoint
~~~

7. Store it under refs/ditch/candidates/session-id.
8. Validate the candidate in a detached linked worktree.

Candidate message:

~~~text
Ditch integration candidate for session session-id
~~~

Path overlap is an early risk signal. Git merge-tree is the Git conflict truth.

---

## 10. Combined validation

### 10.1 Validation worktree

Ditch creates:

~~~text
Ditch data dir/validation-worktrees/session-id
~~~

It checks out the candidate detached, discovers commands, executes them with CI=1, captures bounded output, stops after the first failure, and removes the validation worktree.

Each command has a 10-minute timeout and its own process group.

### 10.2 Command discovery

Precedence:

1. Explicit .ditch/validation.json command arrays from project configuration.
2. check, validate, or test targets in Makefile or justfile.
3. Additive ecosystem discovery.

Supported discovery includes:

- npm, pnpm, Yarn, and Bun package scripts;
- Cargo;
- Flutter and Dart;
- Go;
- Swift;
- Gradle and Maven;
- Python uv/Poetry, Ruff, mypy, pytest, tox, and nox;
- Elixir;
- Ruby/Rake;
- Composer;
- .NET;
- Bazel;
- Pants.

Preferred JavaScript scripts:

~~~text
check
validate
lint
typecheck
type-check
test
build
~~~

The default npm “no test specified” placeholder is skipped.

### 10.3 Dependency environment

The runner currently checks only the canonical project configuration root for:

~~~text
node_modules/.bin
node_modules through NODE_PATH
.venv/bin
VIRTUAL_ENV
~~~

It does not currently:

- install dependencies in the candidate worktree;
- run npm ci or frozen equivalents;
- reuse the originating agent worktree dependency directory;
- maintain a dependency environment keyed by lockfile/toolchain/platform;
- classify missing executables separately from code failures.

This caused the Project#5 incident.

### 10.4 State mapping

All configured checks pass:

~~~text
status              ReadyToApply
integration_state   Ready
validation_state    Passed
conflict_state      None
~~~

Any configured check returns failure for any reason:

~~~text
status              NeedsReview
integration_state   Blocked
validation_state    Failed
conflict_state      ValidationFailed
last_error          Combined validation failed
~~~

No command discovered:

~~~text
status              NeedsReview
integration_state   Blocked
validation_state    NotConfigured
conflict_state      None
~~~

There is no distinct state for:

- tool unavailable;
- dependency setup failed;
- timeout;
- validation process crash;
- genuine lint/test assertion failure.

---

## 11. Apply algorithm

Apply is allowed when:

- status is ReadyToApply; or
- the user explicitly confirms unchecked application and validation is exactly NotConfigured.

Failed validation cannot be bypassed in the current implementation.

Immediately before apply, Ditch:

1. Confirms candidate checkpoint equals the latest checkpoint.
2. Reads current target OID.
3. Compares it with candidate_target_oid.
4. Requires revalidation if target advanced.
5. Finds where the target branch is checked out.
6. If canonical checkout owns it, requires no local user changes.
7. Requires canonical checkout still be on the target branch.
8. Uses git merge --ff-only candidate to update branch, index, and files coherently.
9. If target is not checked out, uses update-ref with expected-old protection.
10. Marks Applied and archives the physical worktree.
11. Re-evaluates remaining completed candidates against the new target.

The daemon does not auto-stash, hard-reset, or overwrite a dirty canonical project.

---

## 12. Integration policy

Policies:

~~~text
AutoApplyAfterValidation
ReviewBeforeApply
~~~

AutoApplyAfterValidation is the domain default. A green candidate immediately enters the guarded apply flow.

ReviewBeforeApply leaves a green candidate at ReadyToApply for explicit user approval.

Project#5 is persisted as ReviewBeforeApply. Even if lint had passed, it would have waited for Apply.

---

## 13. Conflict resolution

Meaningful Git conflicts are never silently resolved.

Ditch can create a locked resolver worktree containing a real no-commit merge between the current target and checkpoint.

Resolver branch:

~~~text
ditch/resolver/session-id-without-hyphens
~~~

The user resolves marked files in that isolated directory and chooses Check resolution. Ditch then:

- verifies target OID has not changed;
- rejects remaining unmerged entries;
- creates a resolved two-parent candidate;
- validates the combined result;
- applies only when policy and validation allow.

The canonical project is never used as the resolver workspace.

---

## 14. Recovery, cleanup, ownership, and security

On startup, persisted records are compared with git worktree list --porcelain -z.

Ditch can restore or quarantine:

- active worktrees that still exist;
- missing managed directories;
- partial create/remove operations;
- resolver worktrees;
- integrated worktrees awaiting archive;
- interrupted snapshot ref/index transitions.

Foreign worktrees are left untouched.

Before destructive cleanup, Ditch verifies:

- expected project/session ownership;
- path lies below the managed root;
- branch belongs to the Ditch namespace;
- resolver branch exactly matches the session;
- expected old ref value matches.

Commands use native argument arrays, not concatenated shell strings. Status/diff/worktree parsing uses machine-readable NUL formats. Task names are sanitized before entering branch/path names.

Known special-repository areas requiring further hardening include advanced submodule, Git LFS/filter, sparse-checkout, and large-monorepo behavior.

---

## 15. Current user flows

### Independent task

~~~text
Start
→ prepare project
→ create isolated workspace
→ run
→ track
→ checkpoint
→ simulate merge
→ validate combined result
→ auto-apply or Ready to apply
~~~

### Actual overlap

~~~text
Agents edit same path
→ both workspaces remain safe
→ overlap metadata appears
→ both currently continue
→ integration checks current target
→ after one applies, other candidates are recomputed
~~~

### Git conflict

~~~text
merge-tree conflict
→ canonical untouched
→ Conflict risk
→ Review / Resolve manually
→ isolated resolver
→ Check resolution
→ validate
→ Apply
~~~

### No validation configured

~~~text
clean Git candidate
→ no commands discovered
→ Needs review
→ explicit Apply without checks is available
~~~

### Validation failed

~~~text
configured command returns nonzero or cannot run correctly
→ Needs review
→ Apply hidden
→ Apply without checks hidden
→ Check again / Retry checks available
~~~

The last flow is a dead end when failure is environmental.

---

## 16. Project#5 case study

### 16.1 Project facts

~~~text
Path:
/Users/mtn/Documents/Personal/SentinelProjects/Project#5

Ditch project ID:
2b93252d-2790-4308-8ad8-b3936bcf42d1

Target:
main

Integration policy:
ReviewBeforeApply

HEAD during investigation:
88c1ce6d460b3cc6f7bc8e151bd49e0e8f943fcd
Save current project state (The Ditch)
~~~

Canonical HEAD contains only:

~~~text
.codex-sentinel/AGENTS.codex-sentinel.md
.codex-sentinel/config.json
.codex/config.toml
AGENTS.md
~~~

The only observed untracked canonical item was .ditch/, which Ditch excludes from user-change decisions.

The canonical folder contains neither generated website because neither candidate was applied.

### 16.2 Latest two agents

Both used effectively the same prompt:

~~~text
Create a website for pomegeranate. All info regarding the fruit,
from geographies to varieties to agronomy.
Do not publish the site to chatgpt sites. build it here.
~~~

| UI name | Session ID | Changed paths | Checkpoint | Candidate | State |
|---|---|---:|---|---|---|
| Build website 1 | 010388b9-8a2c-4fe6-a1f4-c56c4d4f1900 | 28 | b1eda2a... | fd14f43... | NeedsReview / ValidationFailed |
| Build website 2 | e15d1a52-d1e0-4090-9cbe-e00d1cc1be69 | 31 | a80a7ad... | 67047c9... | NeedsReview / ValidationFailed |

Both Codex processes exited with code 0 and had no terminal failure.

The visible Ditch branches still point to the shared base 88c1ce6. Actual results live in checkpoint refs and candidate commits. That is consistent with the checkpoint design.

### 16.3 Overlap

Both generated substantially overlapping Next.js/Vinext-style applications.

Shared paths include:

~~~text
.gitignore
.openai/hosting.json
README.md
app/chatgpt-auth.ts
app/globals.css
app/layout.tsx
app/page.tsx
build/sites-vite-plugin.ts
db/index.ts
db/schema.ts
drizzle.config.ts
drizzle/meta/_journal.json
eslint.config.mjs
examples/d1/app/api/notes/route.ts
examples/d1/db/schema.ts
next-env.d.ts
next.config.ts
package-lock.json
package.json
postcss.config.mjs
public/favicon.svg
public/file.svg
public/globe.svg
public/window.svg
tests/rendered-html.test.mjs
tsconfig.json
vite.config.ts
worker/index.ts
~~~

Build website 2 additionally recorded assets including public/favicon.png and public/og.png.

Both records have Path overlap and reference one another. They also overlap an older website agent.

Stored overlapping-path arrays contain repeated groups. That suggests a projection/deduplication quality issue in addition to the product-level overlap problem.

### 16.4 Why duplicate execution happened

The prompt contained no repository paths. Automatic intent inference therefore created empty expected_paths.

Intent overlap compares paths, not semantic similarity of prompt summaries. The system did not recognize that the tasks were duplicates before editing.

Once files appeared, actual overlap was detected. Current behavior preserves both agents and does not pause one, so both completed.

This prevented data loss but spent duplicate compute and left two substitute results without a compare-and-choose workflow.

### 16.5 Git status of the candidates

Each result has:

- a durable checkpoint commit;
- a clean candidate commit against 88c1ce6;
- no GitConflict state;
- candidate_target_oid equal to 88c1ce6;
- candidate_checkpoint_oid equal to its current checkpoint.

Git merge simulation therefore succeeded individually for both. Validation is the blocker.

If one is applied, main advances and the other is re-evaluated. Given the extensive same-path changes, the second may conflict or become redundant.

### 16.6 Exact failure

Build website 1:

~~~text
Check: lint (npm)
Exit: 127

> the-pomegranate-atlas@0.1.0 lint
> eslint . --ignore-pattern dist --ignore-pattern .next
sh: eslint: command not found
~~~

Build website 2:

~~~text
Check: lint (npm)
Exit: 127

> site-creator-vinext-starter@0.1.0 lint
> eslint . --ignore-pattern dist --ignore-pattern .next
sh: eslint: command not found
~~~

Both package.json files declare ESLint 9.39.4 in devDependencies.

Both original agent worktrees currently contain node_modules.

The candidate validation worktree does not contain node_modules because ignored dependency directories are not part of Git commits. The validator looked only under canonical Project#5 for a reusable node_modules environment. Canonical Project#5 has none.

npm itself started, loaded the lint script, and its shell could not resolve eslint. Exit 127 means the executable was unavailable. ESLint did not inspect the source and report lint findings.

### 16.7 Why Check again cannot help

Top-level Check again calls PrepareIntegration.

Retry checks inside Review changes also calls PrepareIntegration.

PrepareIntegration:

1. checkpoints current agent state;
2. recomputes merge against current main;
3. recreates the candidate;
4. creates a new detached validation worktree;
5. discovers npm run lint again;
6. uses canonical dependency environment again.

Because canonical Project#5 still lacks node_modules, the same failure repeats. The lifecycle event log confirms repeated integration_queued and validation_failed events.

### 16.8 What the buttons currently mean

**Review changes**

- Shows agent-relative and candidate-relative file lists.
- Shows validation check names and icons.
- Does not apply, merge, install dependencies, fix source, or ask the agent to continue.

**Check again**

- Re-runs checkpoint, merge simulation, candidate construction, and validation.
- Does not say which checks will run.
- Does not remediate the dependency environment.

**Keep separate**

- Closes the dialog.
- Preserves the work.
- Does not establish a meaningful long-term variant decision.

**Retry checks**

- Functionally duplicates Check again.

**Apply**

- Hidden because validation is Failed.

**Apply without checks**

- Hidden because it is only allowed for NotConfigured, not Failed.

There is no identifiable forward action.

### 16.9 Why the folder appears empty

The flow was:

~~~text
agent completed
→ checkpoint preserved
→ merge candidate produced
→ validation marked failed
→ integration blocked
→ project policy requires review
→ main remained unchanged
~~~

This is safe behavior. The communication failure is that the UI does not plainly say:

> Your website exists and is safe inside Ditch. It has not been added to the project folder because Ditch could not start ESLint. ESLint did not report a source-code problem.

### 16.10 Relevant earlier Project#5 agents

The database also contains:

- an earlier website agent, 4b589a7c-..., with a similar full application and the same missing-eslint validation failure;
- a Docker agent, 4c3e9dee-..., that initially saw only metadata, produced no Docker changes, and later had NotConfigured validation.

These records explain why the latest overlap sets mention more than the two visible duplicate agents.

### 16.11 Observed lifecycle timeline

Times below are UTC as persisted by ditchd:

| Time | Event |
|---|---|
| 14:57:48.859 | Project save-point operation entered Preparing. |
| 14:57:48.867 | Save-point commit created. |
| 14:57:48.875 | Target ref updated. |
| 14:57:48.882 | Project preparation completed. |
| 14:57:49.305 | Build website 1 locked worktree created. |
| 14:58:25.500 | Build website 2 locked worktree created. |
| 15:07:13.529 | Build website 1 checkpoint preserved. |
| 15:07:13.531 | Build website 1 integration candidate queued. |
| 15:07:13.975 | Build website 1 validation classified failed. |
| 15:08:19.613 | Build website 2 checkpoint preserved. |
| 15:08:19.614 | Build website 2 integration candidate queued. |
| 15:08:19.932 | Build website 2 validation classified failed. |
| 15:13:52.451 | Build website 2 was checked again. |
| 15:13:53.044 | The repeated validation failed for the same environment reason. |

### 16.12 Concrete internal provenance

The two current workspaces are under:

~~~text
/Users/mtn/Library/Application Support/The Ditch/worktrees/
  2b93252d279043088ad8b3936bcf42d1/
    010388b98a2c-create-a-website-for-pomegeranate-all-in/
    e15d1a52d1e0-create-a-website-for-pomegeranate-all-in/
~~~

Both contain package.json, package-lock.json, source files, public assets, and node_modules. These paths are implementation details and should normally be exposed only through an advanced “Open result in Terminal” action.

Relevant durable Git objects visible through all refs during investigation:

~~~text
88c1ce6  main: Save current project state (The Ditch)
b1eda2a  checkpoint: Build website 1
fd14f43  candidate: Build website 1
a80a7ad  checkpoint: Build website 2
67047c9  candidate: Build website 2
~~~

The candidate refs prove that the generated websites are stored in Git object history even though main and the canonical directory have not advanced.

---

## 17. What worked correctly in Project#5

- Agents never overwrote one another.
- Canonical files remained untouched after validation failure.
- Both complete results were checkpointed.
- Both candidate commits remain durable.
- Actual path overlap was detected.
- Git merge simulation used the current target.
- Validation targeted combined candidates rather than only the agent branch.
- Repeated checks did not destroy either result.
- No failed candidate was silently applied.
- Session history and lifecycle events persisted.

These strengths should be preserved.

---

## 18. Design gaps exposed

### 18.1 Infrastructure failure is mislabeled as code failure

All nonzero outcomes become ValidationFailed.

Needed conceptual taxonomy:

~~~text
Passed
CodeFailed
EnvironmentUnavailable
DependencySetupFailed
TimedOut
Crashed
NotConfigured
~~~

### 18.2 Candidate dependency setup is incomplete

Possible strategies for expert review:

1. Deterministic install inside each candidate worktree.
2. Ditch dependency environments keyed by lockfile, runtime, platform, and architecture.
3. Package-manager content caches plus candidate-local install trees.
4. Verified reuse from the agent only when lockfile/toolchain identity matches.
5. Explicit setup and check phases in project configuration.

Directly reusing another worktree's node_modules can be unsafe for native modules, relative symlinks, workspace layouts, lifecycle outputs, and differing lockfiles.

### 18.3 Retry does not change the precondition

Retry should remediate, wait for a changeable condition, ask an agent to fix a genuine code issue, or be hidden.

### 18.4 Duplicate actions

Check again and Retry checks invoke the same daemon command.

### 18.5 No result-selection workflow

The product lacks:

- compare alternatives;
- preview each;
- use this result;
- supersede another;
- ask one agent to combine strengths;
- keep both as intentional variants.

### 18.6 Weak intent preflight

Path-token inference does not detect semantically identical natural-language tasks.

### 18.7 Overlap is advisory

Actual overlap does not currently queue or pause work. Product language has implied stronger coordination than the implementation provides.

### 18.8 Confusing dual state

The screenshot shows Completed and Needs review.

This is technically “process complete, integration incomplete,” but a clearer primary label would be:

~~~text
Website created — checks need setup
~~~

### 18.9 Review is not action-oriented

In the failed-validation case, the dialog informs but provides no useful recommended next step.

### 18.10 Canonical folder is an invisible boundary

Not changing canonical files before apply is correct. The UI needs Preview result and explicit “Not added to your project yet” messaging.

### 18.11 Duplicate overlap paths

Repeated path groups in persisted projection add noise and may inflate UI detail.

---

## 19. Recommended target UX

Environment/setup failure:

~~~text
Build website 1
Website created

Could not run project checks
The required project tools have not been prepared yet.
Your project has not been changed.

[Prepare checks and continue]
[Preview website]
[Review files]
[Advanced…]
~~~

After checks pass with duplicate variants:

~~~text
Website created
Checks passed

Another agent created an alternative version of this website.

[Compare versions]
[Use this version]
[Keep both]
~~~

Genuine lint findings:

~~~text
Website needs a small fix
ESLint found 3 issues.

[Ask agent to fix and recheck]
[Review issues]
[Keep separate]
~~~

Checks cannot be made available:

~~~text
Checks unavailable
Git found no merge conflicts, but the software could not be tested.

[Apply anyway…]
[Open in Terminal]
[Keep separate]
~~~

Offering Apply anyway for an environmental failure is a product-policy decision. It must not be conflated with ignoring genuine failed tests.

---

## 20. Suggested remediation sequence

### Phase A: Structured validation outcomes

- Classify spawn failure, exit 126/127, timeout, signal, dependency setup failure, and ordinary assertion failure separately.
- Persist program, arguments, cwd, setup logs, and phase.
- Surface the actual reason in Flutter.

### Phase B: Deterministic validation environment

- Detect package manager and lockfile.
- Introduce a setup phase.
- Prefer frozen/reproducible installs.
- Cache downloads/artifacts safely.
- Key reuse to lockfile, toolchain, OS, and architecture.
- Add setup timeout, cancellation, disk policy, and lifecycle events.

### Phase C: Action-oriented UI

- Replace Check again with a specific action such as Prepare dependencies and run lint.
- Remove duplicate Retry checks.
- Explain whether canonical project changed.
- Add Preview result without exposing worktree terminology.
- Give every nonterminal state one recommended primary action.

### Phase D: Duplicate task and variant handling

- Compare normalized task semantics before launch.
- Use agent-reported preflight intent.
- Default identical tasks to queue or ask whether an alternative is desired.
- Add Compare results and Use this version.

### Phase E: Operational overlap coordination

- Make path reservation affect scheduling.
- Do not hard-kill existing work.
- Pause/queue the later task at safe boundaries.
- Make Continue separately a durable explicit override.

### Phase F: Missing tests

Add real tests for:

- agent has node_modules while canonical/candidate do not;
- deterministic lockfile install;
- missing eslint classified as environment failure;
- genuine ESLint findings classified as code failure;
- successful retry after setup;
- duplicate prompt admission;
- comparison and selection between alternatives;
- first applied candidate forcing second to recompute;
- previewing isolated results while canonical remains unchanged.

---

## 21. Existing automated coverage

The Rust tests use temporary real Git repositories for critical behavior.

Coverage includes:

- isolated locked worktree creation;
- dirty checkpointing;
- NUL status parsing;
- paths with spaces and renames;
- component-aware overlap;
- independent sequential integrations;
- same-file and new-file conflicts;
- different-line conservative overlap with clean Git merge;
- rename/modify conflict;
- target advance invalidation;
- dirty canonical protection;
- clean merge with failed validation blocked;
- explicit bypass only for missing validation;
- isolated conflict resolution;
- foreign worktree preservation;
- missing managed worktree reconciliation;
- unborn snapshots;
- files added after an empty baseline;
- staged manual work and active Git-operation hazards;
- interrupted creation/snapshot recovery;
- automatic green application.

Flutter tests cover founder-facing worktree language, overlap display, changed-file review, missing-validation continuation, resolver actions, integration-policy parsing, and automatic safe project preparation.

Latest verification before this document:

~~~text
Rust workspace tests: passed
ditchd tests: 51 passed in both lib and bin targets
Flutter tests: 68 passed
Flutter analyze: no issues
Clippy with warnings denied: passed
Rust/Dart formatting and diff checks: passed
~~~

Project#5 reveals missing validation-environment coverage despite strong Git coverage.

---

## 22. Questions for an external reviewer

1. Is checkpoint → merge-tree → candidate → detached validation → CAS apply architecturally sound?
2. What remaining data-loss or Git-corruption risks are most serious?
3. Is creating target save-point commits for safe founder work the right launch-baseline strategy?
4. What is the safest validation-environment model across JavaScript, Python, Rust, Flutter, and polyglot repositories?
5. Should dependencies be installed per candidate or restored from a lockfile-addressed cache?
6. How should code failure and infrastructure failure be modeled separately?
7. When should Apply despite failed checks exist?
8. How should semantic duplicate tasks be detected without blocking intentional alternatives?
9. What should Continue separately mean after actual overlap?
10. How should process completion and integration readiness be presented?
11. How can every state have one obvious recommended next action?
12. How should web apps be previewed without exposing worktree concepts?
13. Should agent branches remain at their base while checkpoint refs contain work?
14. Is fast-forwarding the two-parent candidate into a checked-out target the best apply mechanism?
15. Which crash/failure-injection tests remain missing?
16. How should submodules, LFS, filters, sparse checkout, and monorepos affect policy?
17. Which features should be simplified rather than expanded?

---

## 23. Bottom line

Project#5 proves the core safety property:

> Two agents created overlapping applications without overwriting one another, failed validation did not modify the canonical project, and both results remain recoverable.

It also proves the current UX is incomplete:

> The user cannot tell that lint never actually ran, cannot remediate the missing environment, cannot choose between duplicate results, and cannot identify the action that will put one website into the project folder.

The next iteration should preserve the Git isolation/candidate pipeline while making validation setup, semantic duplicate detection, variant selection, previewing, and next-action clarity first-class orchestration responsibilities.
