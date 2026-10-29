# Task Board

The Board is a durable task view shared by the desktop editions. Open **Board**
in the sidebar for all projects, or the Board icon in a project's Agents header
for that project. New Agent also offers a create/select-task route; ordinary
agent conversations remain available.

## Workflow

- Create a Backlog task with a title, description, acceptance criteria and priority; prioritize it into To Do before execution.
- Reorder cards by dragging or using their menu. Cross-project ordering is not
  supported; select a project to order its work.
- Start a task agent, or link an existing idle, unlinked agent in the project.
- Execution condition is independent of the five columns: Backlog, To Do, In Progress,
  In Review and Done. A failed or stopped agent does not finish its task.
- Successful linked execution submits its final summary to In Review only
  after the child exits. Missing summaries require manual review/submission.
- Accept explicitly confirms human acceptance. Request Changes requires
  feedback and returns work to In Progress. In this phase another execution
  requires an explicit start or a prompt to the linked agent.
- Stop active agents through their conversation before changing or archiving
  their tasks. Cancel/archive preserves history. Only untouched, unlinked
  drafts can be permanently deleted. Reopen preserves prior acceptance.

The inspector shows the summary, criteria, linked conversation and audit
history. Diff capture, validators and automatic correction loops are later
phases; the inspector explicitly reports their evidence as unavailable.

## Persistence and compatibility

The daemon owns task state. Mutations use expected revisions and UUID request
IDs. Projection updates, audit entries, optional agent links and retry receipts
commit in one SQLite transaction before events are published. Retrying an
identical request does not repeat its mutation or launch; a replay returns the
current task projection. Reusing the ID for different input is rejected.

The additive migration is recorded as Community schema extension 4. It retains
the existing task columns and serialized state variants, backfills task JSON,
and adds ordered audit history, request receipts and writer reservations.
Imported tasks receive an import record without invented review evidence.
Legacy running tasks and interrupted pending executions recover as blocked.
Unknown Commercial tables are preserved. A project containing tasks cannot be
deleted, so project removal cannot silently erase task history.

Protocol additions are `TaskRequest`, `TaskResponse`, `TaskChanged` and
`TaskDeleted`, with the `tasks_v1` capability. SSH requests probe support first;
older remote daemons remain usable for their existing features but cannot
serve the Board. The remote daemon is authoritative for its tasks. Scoped
events and snapshot generations prevent an older poll from replacing newer
remote task state.

## Workspace ownership

Both task-linked and ordinary agents reserve their process identity before launch.
User-created agents may run concurrently in the same project, including Full Access
and SSH sessions. Multiple manually started agents and resumed idle conversations
may run alongside one Kanban execution in that project. Kanban tasks remain
serialized, including while their workers and validators finish cleanup; the next
task does not wait for unrelated manual agents. Opening a board-owned conversation
does not bypass its queue ownership or allow overlapping turns on that agent.
These agents share working files, so concurrent edits can invalidate acceptance
evidence and require fresh validation.

Coordinator-owned workers share one slot per coordinator group
and canonical project folder; aliases of the same folder share that slot.
Reservations survive uncertain stops and reconnects so the same agent cannot launch twice.
Independent daemon installations are not a distributed lock service.

Local workspace-write launches clear additional writable roots and exclude
implicit temporary-directory write access. Full Access remains an explicit
exception. SSH execution retains its existing remote-account access boundary;
it does not claim local project sandbox containment.

Local process-group ownership is persisted before the launch gate opens.
Surviving groups keep their reservation across a daemon restart until they
exit. Ambiguous remote transport failures retain ownership until an
authoritative snapshot reconciles it; a retry may report busy in the meantime.
This prevents duplicate execution of uncertain work while allowing independent user agents.

Task API actor attribution records application actions. It is not an
authentication boundary against another process with access to the same user's
runtime socket or database.
# Acceptance engine update

Phase 3 supersedes summary-only automatic submission for newly started task
workers. See [Bounded acceptance and review](ACCEPTANCE_ENGINE.md) for durable
attempts, validators, retries, immutable submissions, and stale-workspace
acceptance checks. Existing text criteria remain human criteria.


## Approved backlog and GitHub workflow (2026-09-28)

New tasks created in the native board start in **Backlog**. Legacy `Create` callers retain their previous To Do default. Backlog → To Do is prioritization only. Dragging into In Progress opens the inspector; execution requires explicit Start Task Agent or Commercial Run selected. In Review and Done still require evidence and human acceptance.

A project board can persist an explicit set of registered subprojects. Combined views preserve every task's owning execution project and provide project/repository filters. Source issue metadata is optional and compatible with older local tasks.

The shared GitHub inbox supports linked-repository issue pages, selected import with stable issue identity, and source metadata refresh preserving local content, revision and review history. Closed GitHub issues remain Backlog with a closed source badge; they do not imply Ditch acceptance. Remote issue writes are absent. GitHub authentication has one Mac-only Connect GitHub action with inline shared-sign-in disclosure. It installs or repairs Ditch's managed CLI, runs browser authorization without terminal prompts, then connects after verifying the account. There is no separate account-confirmation or API-key path. Disconnect cancels integration work without logging other tools out. Live browser/Keychain and distributed-app verification remain required before release.

### SSH board ownership and execution (2026-10-29)

The board remains in the Mac UI. Fresh SSH Backlog tasks are stored on the Mac
until an action needs the remote checkout. Backlog → Todo, Generate with AI,
agent selection, and execution admission transfer the task to the registered
project's SSH runtime first. The host then owns planning, queue execution, agent
conversations and review evidence; its events and snapshots update the same board.
Local project planning and execution keep their existing paths.

Backlog → Todo runs the existing automatic planner on the host, using the title,
description and source context. Saved plans are retained. An unavailable host
leaves the draft in Backlog and reports Waiting for SSH; reconnect and retry the
action. It does not create a failed local planning job. A legacy Todo task with
the old connection error can recover through Generate with AI; conflicting or
active remote copies require review and are never silently replaced.

`board_handoff_v2` is checked before transfer. A transfer has its own durable
request ID and frozen payload, separate from the task ID. The remote task and an
immutable acknowledgment commit atomically. After a lost response, retry reuses
that exact payload. If the user edited the pending local copy, Ditch first
recovers the acknowledgment and then conditionally applies the newer snapshot.
Execution-scope changes or active remote work block reconciliation. Metadata
timestamps do not masquerade as a runtime-version error. Transfer alone never
starts an execution agent.

Remote events cannot replace a locally pending task before acknowledgment.
Queue-created agents enter the Mac's agent registry on their first event, so
their conversation is immediately navigable. Both editions share this code.
Release the updated desktop and SSH runtime artifacts together; older hosts
receive an explicit capability/update message rather than a partial transfer.
GitHub refresh audit records remain stored but are omitted from the inspector's
transition history. No GitHub polling behavior changes.


### Project release groups

Dropping In Review tasks into Done asks for a project release label and explicit
**Accept and move** confirmation. Labels accept arbitrary non-empty names (up to
128 UTF-8 bytes), are unique after trimming within their project, and are unrelated
to Git tags. Choose **Other Tasks** to accept without a label. The inspector uses
the same dialog. Cancelling before confirmation changes nothing.

Click a Review or Done card to select it, Shift-click to extend selection across
the visible range, or Cmd/Ctrl-click to toggle individual cards. Selection stays
within one column; Escape clears it. Double-click a card (or use its Open task /
review action) to open the inspector. Dragging a selected card moves the selected
batch; dragging an unselected card moves only that card.

Click a Done release header, including Other Tasks, to collapse or expand it.
Collapsed headers still accept drops, and collapsing removes that group's tasks
from selection. Expansion is retained while the board is mounted. Done groups retain empty labels;
all-project and subproject boards nest labels under their owning project. Moving
Done cards between groups changes membership and audit history, preserving completion
timestamps and acceptance evidence. Reopening clears membership.

Acceptance is atomic per project, including evidence validation, label creation,
task updates, history, and retry receipts. A mixed-project dialog reports each
project independently; successful batches are not resubmitted. Unconfirmed requests
must be retried with their original request IDs before starting another mutation.
SSH release operations require the owning host's `task_releases_v1` capability.
