# Ditch Remote Protocol v1

Status: version 1, initial interoperable contract. This document and the JSON
schemas under `schemas/` are the canonical contract. Unknown protocol versions
and unknown command types are rejected.

## Boundaries

The relay is a connectivity and sanitized-projection service. `ditchd` remains
the execution authority. The relay never receives repository contents, raw PTY
data, environment variables, secrets, or durable transcript bodies. Commands
submitted while the machine has no authenticated, fresh WebSocket are rejected
with `machine_offline`; they are never queued.

## Encoding

JSON is UTF-8 encoded and MUST use Unicode scalar values as supplied; protocol
signing does not apply additional Unicode normalization. Timestamps are integer
milliseconds since the Unix epoch. UUIDs are lowercase hyphenated strings.
Base64URL uses RFC 4648 URL-safe alphabet without `=` padding.

All REST requests send `X-Ditch-Protocol: 1`. WebSocket envelopes contain
`protocol_version: 1`. A different value returns `unsupported_protocol`.

## Signed REST requests

Authenticated principals sign these exact UTF-8 bytes, with one LF after each
line except the final line:

```text
DITCH1
<UPPERCASE_METHOD>
<PATH_AND_QUERY>
<LOWERCASE_HEX_SHA256_OF_EXACT_BODY_BYTES>
<TIMESTAMP_MS_BASE10>
<BASE64URL_128_BIT_OR_LARGER_NONCE>
<PRINCIPAL_ID>
```

`PATH_AND_QUERY` starts with `/`; query parameters are in their transmitted
order and percent encoding. Empty request bodies hash as zero bytes. Headers:

```text
X-Ditch-Protocol
X-Ditch-Principal-Type  machine | device
X-Ditch-Principal-Id
X-Ditch-Timestamp
X-Ditch-Nonce
X-Ditch-Signature
```

Signatures are ECDSA P-256 with SHA-256. Public keys are uncompressed SEC1
points. The signature is IEEE P1363 `r || s` (64 bytes), Base64URL encoded.
The verifier enforces a five-minute clock window, consumes a nonce once for ten
minutes, verifies principal state, and derives owner scope from the database.
Client-supplied `owner_id` is never an authorization source.

## Device roster and revocation

The relay is authoritative for machine-device authorization. Owner identity is
the tenancy boundary but does not automatically grant every owner device access
to every owner machine. Pairing creates an explicit many-to-many authorization
between one machine and one device.

A signed machine `GET /v1/machines/{machine_id}/devices` returns the complete
set of active devices authorized for that machine, including the public keys
needed for pairwise command validation. A machine may request only its own
roster. Before desktop Remote settings are displayed, `ditchd` transactionally
replaces its local `remote_devices` cache with this snapshot. An empty array is
a valid complete snapshot.

`DELETE /v1/machines/{machine_id}/devices/{device_id}` revokes only that
machine-device authorization. Machine-initiated revocation also deletes the
corresponding local row immediately. Device-initiated disconnection is removed
locally by the next settings reconciliation. Global device-identity revocation
is a separate operation that removes all of the device's machine
authorizations. Revoked authorization tombstones are not displayed, and a
failed roster request must not be represented as a successful cached device
list.

## Pairing

The QR is a URL containing only `v`, `relay`, `pairing_id`, `machine_id`,
`secret`, and `expires_at`. `secret` contains 256 random bits. The service stores
only SHA-256(secret bytes), limits claim attempts, expires challenges after
three minutes, and permits transitions:

```text
pending -> claimed -> confirmed -> consumed
       \-> cancelled
       \-> expired
claimed -> rejected
```

Claim supplies a device name/platform and separate P-256 signing/agreement
public keys. Confirmation is a signed machine request. The first confirmation
creates an owner and atomically binds both principals. An already-owned machine
binds the new phone to its owner. A previously-authorized phone claiming a new
unowned Mac binds that Mac to the phone's existing owner. Terminal transitions
are idempotent; a different repeated claim or confirmation is rejected.

## Socket tickets and MachineHub

A signed `POST /v1/auth/socket-ticket` returns a random, one-use ticket valid
for 30 seconds. The ticket is the only credential allowed in the WSS URL. It is
bound to protocol, role, principal, owner, machine, and connection epoch. The
Worker authorizes the owner relationship before forwarding an upgrade to the
per-machine `MachineHub` Durable Object.

The hub accepts exactly one `machine` socket and zero or more authorized
`device` sockets. WebSocket attachments persist the bound identity across
hibernation. The hub never treats in-memory maps as authorization state. A
device command is delivered only while a fresh authenticated machine socket is
present. Disconnect produces immediate `machine_offline` behavior.

## End-to-end payload protection

Machine and device have distinct P-256 ECDSA and ECDH keypairs. Pairwise keys
are derived from ECDH with HKDF-SHA256:

```text
salt = SHA256("Ditch Remote v1 pairwise salt")
info = UTF8("DITCH-REMOTE-PAIRWISE\n1\n<owner_id>\n<machine_id>\n<device_id>\n<key_version>")
length = 32
```

AES-256-GCM uses a fresh random 12-byte nonce. AAD is the canonical JSON object
formed by lexicographically sorted keys with no whitespace:

```json
{"command_or_event_type":"session.prompt","created_at":0,"device_id":"...","expires_at":0,"machine_id":"...","message_id":"...","owner_id":"...","protocol_version":1}
```

Required keys appear exactly once. AAD mismatch or modified ciphertext fails
closed. Prompt bodies, transcript pages, optional approval response text, and
long review/result content use this envelope. Routing/action names and IDs are
visible to the relay.

## Command lifecycle

Commands use `command.schema.json`. Mutations require a unique idempotency key
and short expiry (at most five minutes). `ditchd` is authoritative for state,
ownership, freshness, and confirmation class. The only v1 commands are:

```text
session.start
session.prompt
session.stop
session.force_kill
approval.respond
attention.execute
attention.acknowledge
attention.snooze
integration.apply
query.session_transcript
query.session_review
```

There is no generic RPC. `shell.execute`, `pty.write`, `run_command`,
`git.execute`, `filesystem.write`, and `kill_pid` are forbidden.

Acceptance is separate from completion. The hub returns `command_ack`; daemon
completion returns `command_result` and/or a projection update. Duplicate IDs
return the persisted prior outcome and never repeat an effect.

## Projection sequencing

Each machine publishes `{epoch, sequence}`. Only one projection frame is in
flight per machine in v1. The relay commits the exact next sequence and then
returns `projection_ack`; an exact duplicate returns a duplicate ACK without
repeating its mutation. The Mac does not advance its durable acknowledged
sequence until that ACK arrives.

A full reconciliation is staged using:

```text
projection_snapshot_begin
projection_snapshot_record (zero or more)
projection_snapshot_commit
```

Begin and commit include exact `projects`, `sessions`, and `attention` record
counts. Snapshot records use the same allowlisted records as incremental
updates. The relay writes the new epoch as staging data. It keeps the previous
committed epoch visible until it validates and commits the complete new
snapshot. A disconnect, timeout, or invalid record before commit cannot clear
the last committed projection.

After snapshot commit, `projection_update` carries one changed record or an
explicit delete record. `ditchd` coalesces repeated changes by stable domain ID,
sends no more than 32 records in an incremental batch, and never queues an
unbounded stream of projection payloads.

Every projection data frame contains exact `owner_id`, `machine_id`, `epoch`,
and positive `sequence` fields. The owner and machine must match the
authenticated WebSocket attachment. Acknowledgements contain:

```json
{"epoch":"uuid","accepted_sequence":42,"status":"applied"}
```

The only ACK statuses are `applied` and `duplicate`. On an out-of-order frame,
the relay does not mutate projection data and returns:

```json
{"epoch":"uuid","expected_sequence":42,"received_sequence":44,"full_snapshot_required":true}
```

On `projection_gap`, `ditchd` abandons all queued and in-flight work for that
epoch, creates a fresh random epoch at sequence one, and sends a new staged
snapshot. Stale ACKs and gaps from an abandoned epoch are ignored. Reconnect
also creates a fresh epoch and staged snapshot. Local SQLite remains
authoritative.

Project, session, and attention projections are explicit allowlists. In
particular, project root paths, prompts, transcript bodies, terminal output,
source, Git objects, and environment data are excluded.

All Remote Protocol v1 timestamp fields, including project and session
`last_activity_at`, are signed 64-bit Unix epoch milliseconds encoded as JSON
integers. RFC3339 strings are not a valid wire representation.

Heartbeat, inbound command, gap, acknowledgement, and revocation processing
take priority over projection transmission. Projection backpressure cannot make
an authenticated but unresponsive socket appear healthy, and cannot block
local execution.

## Transcripts

`query.session_transcript` is live relay only. Pages contain stable message IDs,
ordering sequence, a cursor, `has_more`, and at most 200 messages. D1 never
stores transcript bodies. The normal transcript is the parsed conversation, not
raw terminal scrollback. Query cancellation is a typed socket frame.

## Confirmation classes

`none`, `context_confirmation`, `device_owner_authentication`, and
`desktop_only` are ordered from least to most restrictive. The daemon may raise
the class and the phone cannot lower it. Force kill and integration apply require
device owner authentication. Unsupported destructive operations are desktop
only.

## Errors

Errors use `error.schema.json`. Stable codes are: `unauthorized`,
`device_revoked`, `machine_revoked`, `machine_offline`, `pairing_expired`,
`pairing_used`, `pairing_rejected`, `unsupported_protocol`,
`invalid_signature`, `replay_detected`, `command_expired`, `command_duplicate`,
`session_not_found`, `session_not_running`, `approval_stale`,
`action_not_allowed`, `integration_not_ready`, `integration_conflict`,
`validation_failed`, and `rate_limited`.

## Recovery and deletion

There is no email/password recovery. A surviving Mac revokes a lost phone and
pairs another. Loss of all machines and phones has no v1 recovery path. Remote
identity deletion never deletes local projects. Disabling Remote Control closes
the machine socket and revokes remote authorization while local Ditch continues.
