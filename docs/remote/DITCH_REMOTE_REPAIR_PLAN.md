# The Ditch Mac Remote-Control Repair Plan

## Repository and ownership

Implement this plan in:

```text
/Users/mtn/Documents/Personal/The Ditch v2
```

This repository owns the canonical Remote Protocol v1 contract, the Flutter Mac UI, and `ditchd`. Do not edit the relay or iPhone repositories while executing this plan.

`ditchd` remains the sole execution authority. The remote adapter must invoke the same application services used by the desktop UI. Do not add remote shell, arbitrary PTY input, generic RPC, filesystem writes, or direct Git operations.

## Confirmed production failures

The current production state proves that pairing succeeds but synchronization does not converge:

- The relay accepted projection sequence `1`, the initial `full_snapshot`.
- The relay has zero project and session rows for the paired machine.
- The Mac continues advancing its local projection sequence by roughly one complete snapshot at a time.
- `projection_frames` currently adds `full_snapshot` on every call, including ordinary local updates.
- `ditchd` ignores `projection_gap` because its socket handler only acts on command frames.
- The socket loop drains all queued projection frames before returning to heartbeat and inbound processing.
- Mac device reconciliation fails when the relay omits required device fields. The relay agent will correct that response, but the Mac must retain strict validation because the public keys are security-critical.

## Required outcome

The Mac must:

1. Send one complete sanitized snapshot after connection or explicit reconciliation.
2. Send only changed projection records during normal operation.
3. Bound and coalesce outbound projection work.
4. Track relay acknowledgements.
5. Recover automatically from a sequence gap using a fresh epoch.
6. Keep heartbeat, revocation, commands, and inbound control frames responsive under heavy local activity.
7. Reconcile the machine-scoped device roster before displaying Remote Settings.

## Protocol contract changes

Update the canonical protocol documents, schemas, fixtures, and exported contract. Define these envelopes precisely:

```text
projection_snapshot_begin
projection_snapshot_record
projection_snapshot_commit
projection_update
projection_ack
projection_gap
presence
revoked
machine_access_revoked
```

All envelopes retain `protocol_version = 1`, a UUID `message_id`, and an integer millisecond `created_at`.

`projection_ack` must identify:

```json
{
  "epoch": "uuid",
  "accepted_sequence": 42,
  "status": "applied"
}
```

Permitted statuses are `applied` and `duplicate`. Unknown statuses are rejected.

`projection_gap` must identify:

```json
{
  "epoch": "uuid",
  "expected_sequence": 42,
  "received_sequence": 44,
  "full_snapshot_required": true
}
```

Document snapshot staging semantics: a snapshot is not remotely visible until its commit is acknowledged. Add golden JSON fixtures and update the contract checksum/export mechanism.

## Projector redesign

Replace the current behavior in `crates/ditchd/src/remote_control.rs` where every `publish_projection` call constructs a complete snapshot.

Maintain two distinct paths:

```text
reconciliation path
  -> build full sanitized snapshot
  -> begin / records / commit

incremental path
  -> derive affected safe projection records
  -> coalesce by (kind, stable ID)
  -> send bounded updates
```

Do not serialize entire local domain objects. Continue using explicit `ProjectProjection`, `SessionProjection`, and `AttentionProjection` DTOs with default-deny field selection.

Use stable record identities:

```text
project: project_id
session: session_id
attention: attention_id
```

If the existing event callback does not identify affected records, initially recompute the sanitized projection, compare it with the persisted projection cache, and emit only records whose canonical JSON changed, plus explicit deletions. Do not continuously transmit unchanged records.

## Bounded outbound state

Replace the effectively unbounded stream of complete snapshots with bounded state:

```text
last_generated_sequence
last_acked_sequence
current_epoch
inflight batch
pending records keyed by kind and ID
reconciliation_required
```

Recommended constraints:

- At most 32 records per batch.
- A small bounded number of in-flight batches; one is acceptable for v1.
- Coalesce repeated pending changes to the newest record.
- If capacity is exceeded, discard obsolete pending incrementals, set `reconciliation_required`, and schedule one fresh snapshot.
- Never block local session execution because the relay is slow.

Persist only the minimum reconciliation metadata needed to restart deterministically. Cloud projection state must never become local runtime truth.

## Socket loop and recovery

Restructure the connection loop so every iteration gives priority to:

1. Reading inbound frames.
2. Acting on revocation immediately.
3. Sending a due heartbeat.
4. Sending a bounded amount of projection work.
5. Returning to inbound reads.

Handle every trusted relay frame explicitly:

```text
hello
presence
projection_ack
projection_gap
revoked
machine_access_revoked
command
```

Unknown frame types are denied or safely ignored according to the canonical contract; they are never executed.

On `projection_gap`:

1. Stop transmitting the affected epoch.
2. Drop queued frames from that epoch.
3. Generate a new random epoch.
4. Reset its sequence to zero.
5. Build a fresh sanitized staged snapshot.
6. Resume only after the relay accepts the new snapshot sequence.

Do not keep sending a high sequence when the relay expects an earlier frame.

On reconnect or Mac wake, always use a fresh connection epoch and perform full reconciliation. No command submitted while the Mac was offline may execute later.

## Device roster reconciliation

The relay will expose the machine-scoped endpoint:

```text
GET /v1/machines/{machine_id}/devices
```

Require each active record to contain:

```text
id
name
platform
state
signing_public_key
agreement_public_key
key_version
authorized_at
last_seen_at
```

The Mac must:

- Require `state == active`.
- Validate both public keys and positive `key_version`.
- Replace `remote_devices` transactionally only after the entire response validates.
- Delete local rows omitted by a successful machine-scoped response.
- Retain the previous local rows if the network request or validation fails.
- Refresh before Remote Settings renders device rows.
- Never treat a same-owner phone as authorized unless the relay returned it for this machine.

## Tests

Add focused Rust and daemon integration tests for:

- Exact integer-millisecond JSON serialization.
- One full snapshot per connection or reconciliation, not per local event.
- Incremental change detection and explicit deletion.
- Coalescing repeated updates for one session.
- Bounded queue overflow causing reconciliation.
- ACK advancement and duplicate ACK handling.
- Gap handling creates a fresh epoch.
- Disconnect during begin, record, and commit.
- Heartbeats remain timely during heavy projection traffic.
- Commands and revocation are processed while projection work is pending.
- Restart and sleep/wake reconciliation.
- Exact machine-device response parsing.
- Phone-side revocation removes the local device row after refresh.
- Malformed or incomplete roster responses do not partially replace local state.
- No source paths, transcript bodies, prompts, environment values, or terminal output enter projections.

Update the deterministic fake relay to exercise ACK, gap, staged snapshot, reconnect, and revocation behavior without public Internet access.

## Definition of done

- Remote Settings lists only phones authorized for this Mac.
- Ordinary local events produce bounded incremental projection traffic.
- A gap converges automatically without restarting `ditchd`.
- Heartbeats are not starved.
- A failed snapshot cannot erase the last complete cloud projection.
- The relay eventually contains the same sanitized project/session/attention state as local truth.
- Desktop and remote commands still use the same application services.
- Existing Rust, Flutter, protocol, security, and integration tests pass.

## Non-goals

Do not implement the iPhone application, relay Worker, Cloudflare Tunnel, remote terminal, generic RPC, source upload, cloud transcripts, or an offline command queue in this repository.
