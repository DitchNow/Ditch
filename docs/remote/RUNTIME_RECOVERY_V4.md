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

The app uploads an exact-build, checksum-verified executable. That executable's
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
Runtimes advertising `remote_maintenance_v1` atomically check active agents,
writer reservations, validation, terminals, and pending mutations, then close
admission to new work. The idle check and writer admission share the runtime
mutex. Background session naming also participates. A live upgrade lock keeps
admission closed even if the normal maintenance lease expires. An abandoned
maintenance lease expires after the helper releases its lock.

The current executable link switches while admission is closed, so newly opened
SSH bridges use the replacement's upgrade-lock-aware bootstrap. The helper stops
the verified process through its actual manager, waits for exit and release of
the daemon lock, starts the replacement, and checks its running build, edition,
protocol capability and new instance identity. There is no force-kill fallback.
The new runtime starts in maintenance until its health check completes.

Older daemons cannot atomically block new work. Repair therefore requires an
explicit UI confirmation that agents and terminal work have finished and other
Ditch clients are disconnected. Active-session and instance checks run again
immediately before shutdown. This is a one-time maintenance window, not an
atomic guarantee supplied by the old daemon. New runtimes use guarded maintenance
and do not require that compatibility confirmation.

Failed activation restores the previous executable and restarts it when safe.
Interrupted operations are reconciled on the next repair; a verified replacement
with a lost response is retained without another restart. An unexpected daemon
instance is never killed to force rollback. Errors distinguish activation from
recovery failures. Project files, saved sessions, identity and the runtime
database are not removed by repair. A failed desktop reconnection after verified
activation retains the new installation and offers a retry.

Regression coverage uses disposable daemon processes and Unix sockets for old
and new direct-runtime upgrades, busy deferral, failed-start recovery, interrupted
activation, locking and repeat repair. Runtime tests cover maintenance ownership
and writer admission; Flutter tests cover legacy confirmation and busy errors.
Release validation must additionally exercise packaged previous-release binaries
over real Linux SSH (with and without systemd) and macOS launchd. Fixture results
do not establish that native service or packaged release validation has passed.

In Commercial, Mobile and Relay connect only to the Mac runtime. The Mac routes
remote commands, approval details and transcript reads over SSH and publishes
remote agents in its own projection. Remote daemons never enroll in Mobile Relay.
Mobile control requires the Mac runtime online; SSH reconnect rejoins existing
remote execution without submitting another prompt.
