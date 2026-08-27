# TheDitchRelay Remote-Control Repair Plan

## Repository and ownership

Implement this plan in:

```text
/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay
```

The relay is independently deployed through the Ditch operator's Cloudflare account. It owns Worker REST endpoints, D1 control-plane metadata, the per-machine `MachineHub` Durable Object, notification queues, and APNs delivery. Do not edit the Mac or iPhone repositories while executing this plan.

The relay is connectivity and projection infrastructure. It is not execution authority and must never queue offline user commands.

## Confirmed production failures

### Machine device roster mismatch

The deployed machine-scoped device query returns:

```text
id
name
platform
key_version
authorized_at
last_seen_at
```

`ditchd` correctly requires `state`, `signing_public_key`, and `agreement_public_key` as well. The missing `state` currently produces:

```text
remote_device_sync_failed: relay device omitted state
```

### Projection synchronization stalls at sequence 1

Production D1 shows an accepted initial `full_snapshot` at sequence `1`, followed by no project or session rows. The Mac continues transmitting later sequences. MachineHub does not provide a reliable ACK-driven, serialized reconciliation protocol, so one missed sequence leaves the stream permanently unable to converge.

The current full snapshot also deletes visible projection rows before replacement records have completed.

## Required outcome

The relay must:

1. Return the complete machine-scoped device cryptographic contract.
2. Serialize projection application per machine.
3. Acknowledge successfully committed sequences.
4. Detect duplicates and gaps deterministically.
5. Stage full snapshots and make them visible only after commit.
6. Keep accurate live presence independent of D1 `last_seen_at`.
7. Preserve strict owner and machine-device association isolation.

## Fix the machine-scoped device endpoint

For:

```text
GET /v1/machines/{machine_id}/devices
```

Return only active device associations for the authenticated machine and owner. Each record must include:

```json
{
  "id": "device-uuid",
  "name": "iPhone 17 Pro",
  "platform": "ios",
  "state": "active",
  "signing_public_key": "base64url-public-key",
  "agreement_public_key": "base64url-public-key",
  "key_version": 1,
  "authorized_at": 1787758000000,
  "last_seen_at": 1787758391210
}
```

Authorize through all of:

```text
principal is the requested machine
principal.owner_id equals requested association owner
machine is active
device is active
machine_devices association is active
```

Do not return all devices owned by the owner. A phone paired to Mac A must not appear in Mac B's settings unless an active Mac B association exists.

The public keys are public cryptographic material and are required by `ditchd` for pairwise encryption. Never return private material.

Update OpenAPI, runtime schemas, fixtures, and contract tests together.

## Projection protocol

Implement the canonical contract exported by The Ditch. Support:

```text
projection_snapshot_begin
projection_snapshot_record
projection_snapshot_commit
projection_update
projection_ack
projection_gap
```

Every frame is authenticated by its already-authorized machine socket and must match the attachment's owner and machine IDs.

### Serialized application

Projection mutations for a MachineHub must not overlap sequence reads and writes. Ensure one projection operation completes before the next begins, including all awaited D1 operations.

Do not rely solely on a volatile JavaScript map for durable sequence state. Use Durable Object storage and/or an explicit event serialization mechanism compatible with hibernation, with D1 state as the durable projection record. The implementation must be tested with intentionally delayed D1 operations.

For an incremental record:

```text
validate envelope and allowlisted record
read committed epoch/sequence
if duplicate: return duplicate ACK
if exact next sequence: apply record and advance sequence atomically
if gap: do not mutate and return projection_gap
if stale/unknown epoch: require fresh snapshot
```

Send `projection_ack` only after the write and sequence advancement succeed.

### Non-destructive staged snapshots

Do not clear the current visible projection at snapshot begin.

Add a migration that permits records to be staged by projection epoch or generation. A suitable model is:

```text
machine_projection_state
  owner_id
  machine_id
  active_epoch
  staging_epoch nullable
  accepted_sequence
  staging_started_at
  updated_at

projects / sessions / attention
  owner_id
  machine_id
  projection_epoch
  stable record ID
  safe fields
```

Snapshot behavior:

```text
begin
  -> establish a fresh staging epoch

records
  -> write only into that staging epoch

commit
  -> verify expected record counts and/or canonical digest
  -> atomically switch active_epoch
  -> ACK commit

cleanup
  -> delete old and abandoned epochs later
```

REST projection queries return only rows belonging to `active_epoch`. If the Mac disconnects before commit, the prior active epoch remains visible.

Add conservative cleanup for expired staging epochs and old committed epochs.

## Presence

MachineHub presence is live socket state, not a D1 timestamp.

- Mark the machine online after successful authenticated socket acceptance.
- Refresh `lastSeenAt` on every valid machine frame and heartbeat.
- Use WebSocket attachments so state survives hibernation.
- Expire online presence after the documented freshness window.
- Broadcast transitions to authorized device sockets.
- A connected TCP socket with stale authenticated traffic is not online.
- A historical `machines.last_seen_at` value is not online.

Return presence alongside project queries as today, but ensure the value comes from the correct machine's Durable Object.

## Device socket behavior

Allow an authorized phone to connect to a MachineHub even while the Mac is offline. This lets it receive a later presence transition. Socket tickets remain one-use, short-lived, and bound to owner, device, machine, role, protocol version, and connection epoch.

Revocation or removal of a machine-device association must:

- Close the affected device socket for that machine.
- Consume outstanding matching socket tickets.
- Reject later project queries and commands.
- Leave unrelated Mac associations intact.

## Compatibility and rollout

Deploy relay support before enabling the new Mac protocol.

During migration:

- Advertise projection capabilities in `hello`.
- Continue accepting the old projection frames temporarily if necessary.
- Never mix old and staged snapshot semantics within the same epoch.
- Prefer the new ACK-driven staged protocol when the Mac advertises support.
- Remove the legacy path only after a deliberate compatibility window.

Do not silently reinterpret an unknown protocol major version.

## Tests

Use the Cloudflare Vitest integration environment with real D1 migrations and Durable Object behavior. Add tests for:

- Exact device response fields.
- Machine A cannot list devices associated only with Mac B.
- Owner A cannot list or address owner B's devices or machines.
- One phone associated with multiple Macs.
- Multiple phones associated with one Mac.
- Back-to-back frames with delayed D1 execution.
- ACK only after commit.
- Duplicate sequence returns duplicate ACK without duplicate mutation.
- Gaps do not mutate projections.
- A gap can recover through a fresh epoch.
- Disconnect midway through staged snapshot preserves prior active rows.
- Commit switches all visible projection categories together.
- Abandoned staging cleanup.
- Durable Object hibernation during reconciliation.
- Presence expiry and recovery.
- Authorized phone socket while Mac is offline.
- Device revocation while requests are in flight.
- Commands fail immediately with `machine_offline` and are never persisted for later delivery.

Add a regression test that transmits a realistic snapshot containing dozens of projects, sessions, and attention records without awaiting between individual WebSocket sends.

## Deployment verification

After applying migrations and deploying:

1. Confirm the Worker version and custom domain.
2. Confirm the machine-device endpoint returns the complete contract.
3. Pair a fresh phone or refresh an existing association.
4. Observe a staged snapshot begin, records, commit, and ACK.
5. Query D1 and verify nonzero project/session rows under the active epoch.
6. Confirm mobile REST returns projects and live presence.
7. Force a gap and verify automatic fresh-epoch reconciliation.
8. Interrupt a snapshot and verify old projects remain visible.

Do not claim real Cloudflare behavior verified unless these steps were actually exercised against the deployed environment.

## Definition of done

- Mac Remote Settings can refresh without an omitted-field error.
- Only phones authorized for that Mac are returned.
- Projection sequences advance monotonically and acknowledge durably.
- Missed frames trigger recoverable reconciliation.
- Partial snapshots never erase the last committed projection.
- Machine presence changes accurately through connect, sleep, wake, and disconnect.
- Cross-owner and cross-machine association tests pass.
- No transcript bodies, prompt text, source code, secrets, or terminal output are persisted in D1.

## Non-goals

Do not add local execution, an offline command queue, Cloudflare Tunnel, generic RPC, remote shell, source storage, team/RBAC features, or customer-specific Cloudflare accounts.
