# Mac-mediated remote runtime recovery

Implemented in the working tree on 2026-09-11. This report replaces the earlier
independent remote-host design and its rollout instructions. The patches at the
same filenames have been regenerated for the architecture below.

```text
Mobile ↔ Relay ↔ Mac ditchd ↔ SSH ↔ Remote ditchd ↔ Agents
```

The Mac is the only Mobile endpoint for its local and SSH projects. It owns SSH
credentials, connections and routing. Remote `ditchd` owns remote agent processes,
threads and durable transcripts. Mobile retains its existing identity and paired
Mac encryption key; it does not discover remote-host keys or connect to SSH hosts.
Mobile control requires the Mac runtime online.

## Removed

- New remote-host enrollment, headless Relay authentication/entitlement path and
  remote-to-Relay connection startup.
- Preexisting direct-host pairing/status forwarding routes and per-host Mobile UI.
- Mobile remote-host agreement-key discovery and separately usable SSH machines.
- Relay direct remote-node enrollment implementation and SSH-node Mobile access,
  including authentication, tickets, existing socket authorization and old pairing
  claims. Legacy database rows/migration history are preserved, not destroyed.
- Mandatory lingering and service-manager requirements for ordinary SSH setup.
- The previously proposed `0017_bound_remote_enrollment.sql` migration.

## Implemented and retained

- The Mac projects configured SSH work and attention under its own Mobile identity.
  Project target availability is separate from last-known agent state.
- Mobile start, prompt, stop, force-stop, transcript queries, approval details,
  approvals, denials and question answers use the Mac's shared target dispatch.
  Approval details are fetched from the owning runtime. Resolved requests clear
  through event/snapshot reconciliation. Missing projects cannot fall back to a
  local launch; conflicting agent target identities cannot overwrite another host.
- App Server request-ID/startup fixes, bounded writes, final-output draining,
  independent SSH reconnect loops, replay/snapshot recovery and read-only rejoin
  remain. Rejoin sends no prompt. Local execution retains its existing executor.
- Mobile commands retain an operation ID through SSH. The Mac persists payload
  and target bindings before dispatch; duplicates recover receipts without
  resending mutations. Different payloads cannot reuse the same receipt.
- Relay socket handling dispatches commands through bounded background workers,
  so SSH waits do not block heartbeat and revocation processing. On reconnect,
  saved mutation results are re-encrypted for authorized phones and redelivered.
- The SSH bridge starts a detached user daemon when needed. A daemon lifetime lock
  prevents competing owners. Reconnect reuses a live daemon. Existing user services
  are recognized and can be restarted during explicit setup/upgrade; ordinary
  reconnect neither installs a service nor upgrades a running runtime.
- Mobile distinguishes a missing Mac connection from an unavailable SSH target,
  disables affected controls, and retains connection timeout/keepalive fixes.

## Verification

- Commercial and Community Rust workspace suites pass, including shared recovery
  tests and Mac-mediated routing, duplicate payload rejection, receipt recovery
  after Mac restart, saved-result redelivery, stale approvals and SSH daemon
  rejection of Mobile pairing/commands.
- Commercial desktop: analysis clean, 77 tests pass.
- Community desktop: analysis clean, 131 tests pass.
- Mobile: analysis clean, 85 tests pass, including Mac-only SSH routing and blocking
  commands while the Mac or execution target is unavailable.
- Relay: TypeScript clean; 155 Worker, 12 Node and 4 script tests pass. Tests cover
  retired SSH-node access and target metadata persistence through projection
  replacement and authenticated HTTP refresh. Contract verification passes.
- A disposable-HOME test with the compiled daemon verified concurrent bridges
  share one daemon, bridge exit preserves it, reconnect keeps its epoch, and direct
  Mobile pairing is rejected. This ran on macOS without configuring a service.
- Both replacement patches pass read-only `git apply --check` against the original
  sibling repositories. Root/Community whitespace checks pass.

Test copies, SDK copy and logs remain under ignored `build/remote-recovery/`.
The original Relay/Mobile repositories were not edited.

## Replacement patches and rollout

- [Relay patch](patches/relay-remote-recovery.patch)
- [Mobile patch](patches/mobile-remote-recovery.patch)
- [Patch and per-file hashes](patches/manifest.json)
- [Cleanup plan](MAC_MEDIATED_REMOTE_CLEANUP_PLAN.md)

Apply only these regenerated patches, not an earlier downloaded copy. The Relay
patch includes `0017_mac_ssh_targets.sql`, adding nullable `target_host` and
`target_status` columns to Mac-owned project projections. It has no enrollment or
host-key migration. Deploy the metadata migration and matching Relay before clients
emit those fields. Migration 0017 from the discarded patch was never deployed by
this session; a separately applied old copy must be reconciled before deployment.

Commit/pin the actual Community revision through the normal split-repo workflow,
then build matching desktop and remote artifacts with SSH runtime protocol 4.
Use explicit Remote Runtime Setup for upgrades; protocol 3 cannot accept protocol
4 mutations. Install the matching Mobile build after Relay supports its contracts.

Nothing was deployed, and no installed local daemon, remote host settings, running
remote daemon, or phone app was changed. Linux artifact packaging and installed
Linux/iPhone acceptance still need staging: interrupt SSH and phone connectivity,
reconnect, resolve requests from either client, stop agents, restart the Mac
runtime, and verify local work continues while another SSH host is unreachable.
Mac-offline Mobile operation is deliberately unsupported.

## Recovery limits

Transport loss alone does not stop the detached remote process. OS policies that
kill user processes, daemon crashes and host reboot can interrupt turns. Persisted
thread identity and transcript support rejoin and explicit continuation; messages
not committed before a crash are not reconstructed from native rollout files.

Mutations with an unconfirmed outcome remain unknown until a receipt is available.
The background result-recovery pass considers up to 64 non-query receipts from the
last 15 minutes and runs on Relay connection/periodically. It never resubmits work.
Older uncertainty requires inspecting the rejoined session. There is no automatic
offline prompt queue or claim of exactly-once execution across every crash window.

General MCP elicitation and arbitrary Mobile shell/PTY control remain outside this
change. The remote runtime still needs a compatible Codex installation and its own
Codex authentication; Mac credentials are not copied to the host.
