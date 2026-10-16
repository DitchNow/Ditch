# SSH runtime recovery protocol 4

The remote daemon owns processes and durable state. SSH RPC/event bridges are
clients: disconnecting a bridge never stops an agent or starts another turn.
Local and remote projects now share the app-server session adapter. See
[session controls and update requirements](../runtime-sessions.md).

- Startup waits for initialize, thread/start or thread/resume, and turn/start
  responses in order. Client IDs are namespaced strings; server request IDs use
  an independent namespace. Each startup step has a 15-second deadline.
- Runtime events carry a daemon epoch and monotonic sequence. The bounded replay
  ring retains at most 2048 events / 8 MiB. SubscribeEventsSince replays a valid
  cursor, otherwise supplies an authoritative snapshot. Ten-second heartbeats
  consume no event sequence. Slow subscribers are disconnected and reconcile.
- SSH request queues, reads and writes have a 30-second budget. Each host has its
  own reconnect worker, backoff and stream. Streams time out after 30 seconds
  without a frame; cancellation releases the bridge process. SSH keepalives use
  10-second intervals and three missed replies.
- RejoinAgent is read-only: it returns current agent state, pending permissions,
  and up to 200 durable transcript messages after a cursor. It never sends a
  prompt. Continue pagination while has_more is true.
- Starts, resumes, prompts, approvals, answers and stops reserve an operation ID
  before dispatch. Reusing that ID with a different payload fails. A completed
  receipt returns the original response. An interrupted reservation remains
  unknown. After a lost SSH acknowledgement, query the receipt on a new bridge;
  never automatically repeat a possibly accepted prompt.
- A daemon restart creates a new epoch. Active records become stale and pending
  approval attention is dismissed. Saved native threads remain resumable with
  the original Codex home. Rejoining displays the durable transcript; it does
  not reconstruct messages that were never committed before a daemon crash.
- Remote App Server completion is finalized after stdout events are drained.
  Retrying model errors remain nonterminal. Unknown server requests receive an
  explicit protocol error instead of hanging silently.

Use Reconnect for a transport retry and Remote Runtime Setup for installing or
repairing the daemon. Protocol 3 daemons cannot accept new protocol 4 mutations;
upgrade the remote artifact first. New turns additionally require the
`app_server_sessions_v2` capability; protocol 4 recovery controls remain usable
while preparing that upgrade. Ordinary reconnection does not install or
restart services. The SSH bridge starts a detached user daemon when none is
reachable; a lifetime lock prevents competing daemon owners. Neither a service
manager nor Linux lingering is required. Host policies that kill user processes
and host reboot can interrupt live turns; this protocol preserves recoverable
session identity, not live processes. Service/autostart configuration is optional.

## Manager-aware installation and repair

Install / Repair requests one update on the Mac runtime. Closing the setup
dialog does not cancel it. Its local job record survives a Mac runtime restart;
checking setup or reconnecting a saved remote resumes that already requested
job. Busy work is polled automatically and progress stays on the existing setup
screen. Merely connecting does not request an update. Updates cannot progress
while the requesting Mac runtime is offline.

The app uploads its checksum-verified executable once per active job. That executable's
`upgrade-remote` helper performs the handoff on the target host. It selects
systemd or launchd only when the service's reported PID owns the running daemon;
an inactive service or installed service manager does not establish ownership.
Otherwise an existing daemon retains direct-process management. Fresh installs
prefer a usable user service manager and use direct startup when none is usable.
Custom service definitions are not overwritten. Enabling Linux lingering is not
automatic; host policy still determines survival across logout and reboot.

Manager selection is saved in `~/.ditch/run/runtime-manager.json`. Reconnect
reuses a live daemon; if it is missing, it starts the recorded service or a direct
daemon. Reconnect does not upgrade an existing process or submit another turn.

Repair holds a host-side flock and journals progress in
`~/.ditch/run/runtime-upgrade.json`. The replacement is staged before activation.
Runtimes advertising `remote_maintenance_v1` and
`remote_terminal_maintenance_v1` atomically check active agents,
writer reservations, validation, setup operations, and pending mutations, then close
admission to new work. The idle check and writer admission share the runtime
mutex. Background session naming also participates. A live upgrade lock keeps
admission closed even if the normal maintenance lease expires. An abandoned
maintenance lease expires after the helper releases its lock.

The current executable link switches while admission is closed, so newly opened
SSH bridges use the replacement's upgrade-lock-aware bootstrap. The helper stops
the verified process through its actual manager, waits for exit and release of
the daemon lock, starts the replacement, and checks its running build, edition,
protocol capability and new instance identity. There is no force-kill fallback
for the daemon or agents.
The new runtime starts in maintenance until its health check completes.

Older daemons do not require a confirmation dialog. The helper takes a SQLite
writer transaction on the existing database, checks persisted writer
reservations and agent states, then pauses the identified daemon and inspects
its descendants. The transaction prevents a competing agent reservation from
passing while the process is checked. It changes no database rows. A busy
result resumes the daemon and the Mac retries automatically. The handoff uses
the verified manager or signals only the identified daemon; it does not invoke
the old Shutdown request, which could kill agents. Linux uses pidfds for signal
identity; macOS checks native process start identity. A detached watchdog
resumes a paused daemon if its helper disappears. Process exit is followed by
verification that the daemon ownership lock was released, including the short
kernel teardown interval after exit becomes observable.

Once agent work is idle, an explicitly requested upgrade closes sessions in
Ditch's project terminal pane, including commands running inside those sessions.
Current runtimes use their registered project-terminal child handles. Older
runtimes (including the first maintenance release) use a fenced migration:
their only direct PTY launch paths are project terminals and Codex setup. The
helper identifies daemon-owned PTY session leaders, excludes login and sandbox
setup commands, and closes the selected sessions and their command processes.
Other daemon children continue to defer migration. Terminals outside Ditch are
not selected. Signals use captured process identities; commands that ignore
hangup can be killed, but that escalation never applies to the daemon or agent
process trees. Exited records are reaped without deleting project or saved
conversation data. After a detected remote daemon restart, the app clears stale
terminal views and suppresses automatic shell reopening. Users can open a new
terminal themselves. There is no extra confirmation dialog.

Clients accept a matching runtime protocol with the required execution
capabilities and edition, without requiring an identical build identifier.
Updated installers refuse to replace a newer same-edition build with an older
one; an incompatible newer runtime requires updating the Mac app. Artifact
builds carry the release build number on Linux as well as macOS. This policy
cannot change the behavior of installers in already released old Mac apps.

Failed activation restores the previous executable and restarts it when safe.
Interrupted operations are reconciled on the next repair; a verified replacement
with a lost response is retained without another restart. An unexpected daemon
instance is never killed to force rollback. Errors distinguish activation from
recovery failures. Project files, saved sessions, identity and the runtime
database are not removed by repair. A failed desktop reconnection after verified
activation retains the new installation and retries transient transport errors.
Real failures appear inline on the existing setup screen.

Regression coverage uses disposable daemon processes and Unix sockets for old
and new direct-runtime upgrades, busy deferral, failed-start recovery, interrupted
activation, locking and repeat repair. Runtime tests cover maintenance ownership
and writer admission; Flutter tests cover a single-click update, pending-job
reattachment, lost progress replies, and inline errors without confirmation.
`scripts/test-remote-runtime-migration.py --previous <native-140-ditchd>
--candidate <native-candidate-ditchd>` creates an isolated temporary home and
tests actual legacy wire behavior, reservation protection, daemon replacement,
project and saved-message preservation, and repeat repair. It also verifies
watchdog recovery after a lost helper, closure of idle, busy, exec-replaced and
hangup-resistant Ditch terminals, preservation of an unrelated terminal, and
registered-terminal maintenance in the candidate. It seeds a saved
conversation rather than calling a model provider. Run it with native binaries
and a short writable `TMPDIR`; no real SSH account or service is modified.
Release validation must additionally exercise packaged previous-release binaries
over real Linux SSH (with and without systemd) and macOS launchd. Fixture results
do not establish that native service or packaged release validation has passed.

In Commercial, Mobile and Relay connect only to the Mac runtime. The Mac routes
remote commands, approval details and transcript reads over SSH and publishes
remote agents in its own projection. Remote daemons never enroll in Mobile Relay.
Mobile control requires the Mac runtime online; SSH reconnect rejoins existing
remote execution without submitting another prompt.
