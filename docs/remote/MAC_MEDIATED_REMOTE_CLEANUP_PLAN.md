# Mac-mediated SSH agent recovery: cleanup plan

Status: implemented in source. See `REMOTE_RUNTIME_RECOVERY_IMPLEMENTATION.md`
for verification, the replacement patches and remaining staging acceptance.

This plan supersedes the direct-host enrollment and Mac-offline Mobile design in
`REMOTE_RUNTIME_RECOVERY_IMPLEMENTATION.md` and the two existing recovery patches.
The patches at the original filenames have now been replaced; use their current
manifest hashes rather than an earlier downloaded copy.

## Required architecture

```text
Mobile <-> Relay <-> Mac ditchd <-> SSH <-> Remote ditchd <-> Agents
```

- The Mac runtime is the only Mobile/Relay endpoint for its local and SSH projects.
- The Mac owns SSH credentials, host connections, project routing and the Mobile
  projection. The remote runtime owns remote processes, native agent threads and
  durable remote transcripts. The Mac's view of them is a synchronized mirror.
- Mobile retains its existing device identity and paired Mac public key for the
  existing encrypted Mobile-to-Mac channel. It receives no SSH credentials, no
  remote-host encryption keys and no separately paired SSH machines.
- Relay retains its existing Mac/device authentication and encrypted routing role.
  SSH runtimes do not enroll, pair with phones, or open Mobile Relay connections.
- Mobile control requires the Mac runtime to be online. Losing the Mac connection
  must not itself stop remote agents. Restoring it rejoins existing sessions.
- A host reboot or OS termination of the daemon can interrupt a turn. Rejoin reads
  persisted state; explicit continuation resumes a saved thread. Neither is an
  excuse to silently resend a prompt or start another agent.

## Findings that determine the cleanup

The changes in `crates/ditchd/src/remote_control.rs` exclude SSH projects, agents
and attention from `ProjectionSnapshot::from_state`. `owns_session` and
`SessionStart` also reject SSH projects. These must become Mac-managed-target
checks, preserving authorization while admitting configured SSH projects.

The shared runtime already routes `ListAgentMessages`, `RejoinAgent`, agent
commands, approval decisions and question answers to SSH hosts. Remote snapshots
already import agent, pending-permission and attention records into the Mac.
Use these paths rather than introducing another executor or authority.

Direct-host functionality predates some of this task: the private protocol and
edition dispatcher already have `RemoteMachine*Pairing` forwarding routes, and
the original Relay has remote-node enrollment endpoints. Reverting only the new
diff would therefore leave a second architecture available. Retire these paths
explicitly for the required architecture, preserving unrelated data and features.

## Implementation sequence

### 1. Remove direct-host Mobile capabilities

Files: `crates/ditchd/src/edition.rs`, `remote_control.rs`, private
`crates/ditch_protocol/src/lib.rs`, `apps/macos/lib/commercial_remote_surface.dart`,
and the corresponding Community edition settings interface.

- Remove the new prepare/claim/enroll protocol requests and implementations,
  headless enrollment entitlement branch, project-binding rewrite on enrollment,
  remote-device revocation/disable forwarding and remote enrollment response.
- Retire host-specific pairing/status/identity routes used solely for direct
  Mobile access, including preexisting forwarding routes. Remove callers and
  tests together; obsolete clients receive an explicit unsupported response.
- Guard commercial startup so `daemon --remote` does not start the Mobile Relay
  connection or its enrollment-dependent entitlement polling. Guard relevant
  request paths as well; startup suppression alone is insufficient.
- Keep Mac pairing, Mac entitlement enforcement, device revocation and existing
  Mobile-to-Mac encryption. Keep remote installation identity where SSH handshake,
  session routing or persistence needs it; this is distinct from Relay enrollment.
- Remove per-SSH-host Mobile settings and enrollment buttons. Preserve normal SSH
  setup/reconnect controls and the Mac's Remote Control settings.
- Remove helpers added solely for headless enrollment, including the shared
  entitlement decoder extraction if it has no remaining caller.

Acceptance: an SSH daemon starts and runs agents without registering with Relay;
existing local Mac pairing and commercial enforcement remain functional.

### 2. Project and authorize remote work through the Mac

Files: private `remote_control.rs`, `crates/ditch_remote/src/lib.rs`, shared
`runtime_shared.rs`, and protocol projection contracts only where necessary.

- Include configured SSH projects, their mirrored agents and attention in the
  Mac's projection. Keep routing under the paired Mac machine ID; remote target
  identity remains execution metadata, not a new Mobile machine or authority.
- Replace local-only checks with validation that the Mac manages the requested
  project/session. Resolve its execution target from registered project metadata.
  Never accept a caller-supplied arbitrary hostname or reinterpret a remote path
  as a local path when disconnected.
- Route start, prompt, stop, force-stop, transcript, approval detail, approval,
  denial, answers and attention acknowledgment through the existing shared target
  dispatch. Audit each command; do not assume the current generic adapter covers
  attention or request freshness automatically.
- Validate permission/request identity against the owning remote runtime before
  accepting a decision. After an approval is resolved on either client, publish
  the resolution back through the Mac so both clients clear the request.
- Verify stable project/session/request identity across reconnects and multiple
  SSH hosts; reject conflicting target mappings rather than dispatching to the
  wrong host. Do not derive ownership solely from a transient connection cache.

Acceptance: Mobile controls a configured SSH agent through the paired Mac using
the same owning-runtime services as desktop; local agents use their existing path.

### 3. Complete recovery across both network hops

Files: shared `runtime_shared.rs`, `ssh_remote.rs`, `remote_app_server_runtime.rs`,
private `remote_control.rs`, and Mobile error/presence handling.

- Preserve App Server startup/ID fixes, bounded I/O, independent host reconnect
  loops, event epoch/sequence replay, snapshot fallback and read-only transcript
  rejoin. Preserve final output draining and request reconciliation.
- Expose SSH-target unavailable/reconnecting status through the Mac. A live Mac
  Relay socket must not imply every SSH host is available. Keep last-known agent
  state distinguishable from current target availability.
- Preserve actionable error outcomes: Mac offline, SSH target unavailable,
  expired approval and unknown mutation outcome. Replace the current adapter's
  broad conversion of runtime failures to `action_not_allowed` where needed.
- Persist a stable association between an accepted Mobile mutation and its SSH
  operation receipt before dispatch. On a lost acknowledgment or Mac restart,
  look up that same receipt; do not manufacture another operation ID and resend.
  A crash between reservation and dispatch can remain explicitly unknown.
- Avoid doing a blocking SSH RPC on the Relay socket reader. Use bounded command
  dispatch so a slow host does not block heartbeat, revocation or other results.
- Do not queue new offline prompts for automatic execution on reconnect. Rejoin
  reads state; sending new work remains an explicit action.

Acceptance: losing either network hop and restoring it does not duplicate starts,
prompts or approvals, and an unreachable host does not block local work.

### 4. Remove the mandatory service/lingering dependency

Files: shared `ssh_remote.rs`, remote daemon startup/socket lifecycle, setup UI
and remote runtime documentation.

- Remove the Linux `Linger=yes` enrollment gate with the enrollment path.
- Replace the installer's mandatory user-service-manager requirement with a
  user-account bootstrap: discover the existing runtime, validate its handshake,
  or start the installed daemon detached from the SSH bridge with redirected
  standard streams and bounded readiness polling.
- Make concurrent bootstrap safe with a startup lock and socket/daemon identity
  validation. Do not restart a healthy daemon on ordinary reconnect, delete a live
  socket, or create a second process supervisor. Coordinate with an existing
  installed service to avoid competing owners.
- Keep service installation/autostart an explicit optional capability. Report
  actual host persistence limitations without blocking ordinary SSH use or
  claiming that detachment overrides logout-kill policy or reboot.
- Reconnect never upgrades a runtime. Explicit setup/upgrade remains separate and
  reports an incompatible protocol rather than silently disrupting an active turn.

Acceptance: ordinary SSH setup and reconnect work with `Linger=no` and without
a usable user service manager on a host that permits background user processes.

### 5. Replace the Relay and Mobile patches

Work in the existing writable test copies under `build/remote-recovery/`; generate
replacement patches against the original sibling repositories. Do not write to
those originals from this workspace.

Relay:

- Drop migration `0017_bound_remote_enrollment.sql`, enrollment key binding,
  headless authentication exceptions and added remote agreement-key discovery.
- Retire the existing direct remote-node enrollment routes and prevent SSH nodes
  from acting as Mobile machine endpoints. Audit ticket issuance, socket upgrade,
  resumed sockets, device roster and per-machine authorization together.
- Preserve existing migration history and unrelated account/activation data.
  Do not delete legacy machine rows as a shortcut. Old SSH-node records must not
  appear as usable Mobile endpoints; existing active access must fail closed.
- Retain only contract/validation changes needed by the Mac-mediated commands,
  approval details, typed answers and target availability, with focused tests.

Mobile:

- Remove new SSH-host agreement-key discovery/pinning and direct-host targeting.
  Preserve existing Mac pairing keys and the phone's cryptographic identity.
- Display SSH projects/agents within the paired Mac connection; any host label is
  descriptive, not a separate pairing or connection.
- Keep useful approval/question UI and connection timeout/keepalive fixes. Route
  every command and encrypted query to the Mac. Show Mac-offline and SSH-target
  unavailable states accurately, with controls disabled where appropriate.

Regenerate both patches, manifest hashes and exported contract pins. Check they
apply to their current baselines. Replace the previous rollout report and remove
Mac-offline Mobile acceptance claims and mandatory lingering instructions.

### 6. Verify and deliver

Automated checks should demonstrate behavior, not merely removed symbol names:

1. Local agent start/prompt/approval/stop and existing Mac pairing still work.
2. Every supported Mobile action on an SSH agent reaches the selected host through
   the Mac; no remote-host Relay connection or Mobile host-key acquisition occurs.
3. SSH loss during output/approval followed by reconnect restores one session,
   durable transcript and current pending requests. Rejoin sends no prompt.
4. Lost mutation acknowledgment and Mac restart recover the same operation or
   report unknown, without another mutation. Stale approval never affects a new turn.
5. Phone network loss/foreground return restores the Mac projection. Mac offline
   disables Mobile control; remote execution can continue, then rejoin on return.
6. Two hosts, one unreachable, plus local work: no blocking, cross-host routing or
   identity confusion. Local-looking remote paths never run on the Mac.
7. No-linger bootstrap, simultaneous reconnect and already-running runtime tests
   prove single ownership and no reconnect-induced restart.
8. Direct enrollment and SSH-node Mobile endpoint attempts are rejected while
   normal Mac authentication, pairing and revocation pass.

Run affected Rust workspace tests, desktop/Mobile analysis and tests, Relay
TypeScript/tests, contract checks, patch applicability and whitespace checks.
Then perform staging installed-build SSH-loss/Mobile tests. Source tests alone
are not evidence of production recovery. No deployment is part of proposing this
plan. Preserve unrelated user changes throughout; avoid blanket file reverts.
