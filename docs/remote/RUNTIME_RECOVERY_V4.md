# SSH runtime recovery protocol 4

The remote daemon owns processes and durable state. SSH RPC/event bridges are
clients: disconnecting a bridge never stops an agent or starts another turn.
Local projects retain the existing local execution path.

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
upgrade the remote artifact first. Ordinary reconnection does not install or
restart services. The SSH bridge starts a detached user daemon when none is
reachable; a lifetime lock prevents competing daemon owners. Neither a service
manager nor Linux lingering is required. Host policies that kill user processes
and host reboot can interrupt live turns; this protocol preserves recoverable
session identity, not live processes. Service/autostart configuration is optional.

In Commercial, Mobile and Relay connect only to the Mac runtime. The Mac routes
remote commands, approval details and transcript reads over SSH and publishes
remote agents in its own projection. Remote daemons never enroll in Mobile Relay.
Mobile control requires the Mac runtime online; SSH reconnect rejoins existing
remote execution without submitting another prompt.
