# Bounded acceptance and review

Phase 3 adds daemon-owned task attempts, deterministic validators, bounded
correction attempts, and immutable review submissions. It does not add
Ditchmaster, scheduled repetition, worktrees, or a second runtime authority.

## Using it

Open a task's inspector and choose **Acceptance checks**. Existing text criteria
remain human criteria. Add required file, Git, command, named test-preset,
human, or supporting agent-assertion criteria. Command definitions use argv
arrays, a project-relative working directory, an expected exit code, and a
timeout. Adding a command requires confirmation of that exact definition.
The optional preset name saves an explicitly approved command for the project.

Automatic retries are off by default. Enabling them defaults to three attempts,
a 30-minute wall-clock budget, a 60-second validator timeout, repeated-failure
stopping, and a fresh thread. Resume is a deliberate alternative. The daemon
enforces ceilings of ten attempts per cycle, 24 hours per cycle, and five
minutes per validator. Permission denial always stops the cycle. Requiring a
workspace change is optional; when enabled, unchanged work stops the cycle.

Configured acceptance checks use App Server. Task start makes this transport
requirement visible; it never silently drops checks onto legacy execution.
Legacy one-off workers still produce attempt history, but a final assistant
assertion alone cannot automatically submit their work. Inspect the result and
explicitly submit or revalidate it.

The inspector shows the acceptance matrix, unresolved human checklist,
validator commands/exit codes/durations/bounded output, workspace metadata,
pre-existing dirty paths, current changed-file list, on-demand diff, final
summary, approvals, provider usage when available, and an attempt timeline.
The existing linked-agent chat action remains available.

**Accept** remains human-only and checks that the submission's workspace
fingerprint still matches. Changed evidence blocks acceptance. **Revalidate
evidence** is an explicit action that may execute the already-approved checks
again; incomplete validators are never automatically resumed after a crash.
**Request Changes** requires feedback, preserves the old submission, and
starts a new bounded cycle with that feedback. **Cancel loop** stops worker or
validator execution and leaves the task In Progress with an attention reason.

## Authority and durable boundaries

Each attempt has its own UUID, cycle UUID, ordinal, task ID, allocated agent
owner, optional provider thread, exact profile and skill hashes, prompt,
reviewer feedback, timestamps, terminal result/failure, base/end workspace
evidence, validators, approvals, summary, and optional provider usage.
`validation_only` distinguishes explicit revalidation from a worker launch;
an allocated execution owner is not proof that a model session ran. Review
agent links refer to actual worker attempts when available.

Attempts and immutable submission values live in Community's existing durable
task JSON, with additive serde defaults for older tasks. Task projection,
append-only audit, and mutation receipt commit together in SQLite. Project
preset changes also commit with their request receipt. The migration ledger
adds version 5 for the validator-ownership marker on writer reservations.
No parallel JSON metadata files or separate Commercial acceptance store exist.

Task starts reserve their workspace before worker dispatch. Completion wakes
the coordinator through an event channel. Worker exit is followed by evidence
capture and validators outside the runtime mutex. Each completed validator is
persisted before the next one runs. Required automatic checks must all pass,
with a nonempty final summary, to create an automatic review submission.
Human criteria remain unresolved and assertions remain supporting evidence.

Failures generate correction prompts containing the retained failure details
and reviewer feedback. Repeated identical failure, required-but-absent changes,
denial, cancellation, infrastructure failure, workspace change, or budget
exhaustion stops dispatch and raises distinct task attention. No automatic path
sets Done. Runtime status includes reserved validation work, so an upgrade
cannot mistake validation for an idle runtime.

Process reservations span worker completion, validation, and retries. A coordinator
group retains its project slot throughout that cycle; user-created agents can run
independently in the same project. Workspace fingerprints still invalidate stale
evidence when concurrent edits occur. Stops retain ownership until child processes
exit, and SSH task updates transfer/release their own desktop reservation without
blocking unrelated agents. Retries reuse the original execution profile and pinned
skills. Any interactive approval stops further automatic retries at the attempt
boundary, preventing session permissions from carrying forward silently.

## Validator process ownership

Command validators use Codex `command/exec` with explicit `sandboxPolicy` and
fixed argv. No shell is inserted. Normal local policy allows project writes,
keeps the existing network setting, and excludes implicit temporary writable
roots; Full Access is never inferred or enabled by retry. SSH retains its
documented unconfined boundary and guarded worker approvals.

Codex launches command validators in a separate process group from App Server.
The validator adapter therefore uses connection-owned streaming and
`command/exec/terminate` for cancellation. It waits for the terminal command
response before forcibly cleaning up the App Server process. If termination
cannot be confirmed, the connection is closed gracefully and its durable
reservation remains until process exit is observed. Restart treats ownership
as stale, retains completed evidence, and never launches an automatic retry or
reruns an incomplete command. Closing the originating streaming connection is
also Codex's documented command-cancellation boundary.

The actual installed CLI tested was **0.153.4**. Its generated schema includes
`thread/goal/set`, `thread/goal/get`, `thread/goal/clear`, turn lifecycle events,
usage events, and streaming command termination. Ditch deliberately does not
set provider goals: its attempt/deadline engine remains the sole retry
authority. Usage is retained only from provider notifications; missing usage
stays unavailable.

## Evidence limits and attribution

Git capture uses read-only commands with hooks, external diff, text conversion,
and filesystem-monitor helpers disabled. It records HEAD, porcelain status,
bounded diff statistics, and fingerprints tracked/untracked non-ignored file
contents, including binary content and submodule snapshots. Diff display uses
textual Git output with binary redaction and sensitive-line redaction. Ditch
does not stage, commit, reset, rebase, merge, or clean the workspace.

Ignored files are excluded from the general Git snapshot; explicitly selected
file-validator paths are additionally fingerprinted even when ignored.
Non-Git evidence is a bounded regular-file snapshot with explicit limitations.
File validators reject traversal and symlink paths. Evidence limits include
2 MiB Git output, 20,000 hashed files, 256 MiB aggregate file content, 32 MiB
per hashed file, 1 MiB content predicates, and 4 KiB retained validator output.
Oversized evidence blocks with an actionable error rather than claiming a
complete review. History is bounded to 50 attempts/submissions and 768 KiB of
acceptance data per task; use a follow-up task when that budget is reached.

HEAD changes during work, changes during validation, and changes between
validation and retry stop the engine. Concurrent external edits made *during*
a worker turn cannot reliably be distinguished from the worker's own edits;
the evidence explicitly reports this attribution limit. No automatic reset or
overwrite is used to resolve ambiguity. Pre-existing dirty paths refer to the
start of the particular attempt, not a claim of file authorship.

## Verification

Deterministic tests cover first-pass success, correction then success, budget
exhaustion, identical/no-change stopping, denial, worker/validator cancellation,
timeout/infrastructure failures, duplicate starts, stale acceptance,
request-changes immutability, resume/profile parity, validation lease ownership,
cross-project concurrency, and restart at active durable boundaries with
retained validator evidence. Earlier alias/nesting/global-writer and SSH
regressions continue to run. Flutter tests cover empty/error review states,
human versus automatic criteria, bounded policy defaults, and cancellation.

Explicit installed-Codex tests verify managed-skill recognition, workspace
containment, and cancellation of the validator's separate process group. These
use isolated temporary CODEX_HOME directories and no model turns. Live model
quality and a real SSH-host walkthrough remain manual integration checks;
App Server continues to be opt-in outside configured acceptance tasks.

Protocol reference: [official Codex App Server documentation](https://learn.chatgpt.com/docs/app-server).
