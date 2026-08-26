# Build TheDitchRelay as an Independent Cloudflare Remote Control Plane

Use this prompt in the separate repository:

`/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay`

Do not implement the relay inside the Mac or iPhone repositories. Do not edit
either client repository unless explicitly asked after identifying a contract
defect.

## Mission

Build the independently deployable backend for The Ditch Remote Control v1.
The relay belongs to the product operator's single Cloudflare account and will
eventually run at a product domain such as `remote.theditch.app`. Individual
Ditch users do not create Cloudflare accounts. They are anonymous application
domain `Owner`, `Machine`, and `Device` records inside this deployment.

The three repository boundaries are:

```text
/Users/mtn/Documents/Personal/The Ditch v2
  Mac UI and ditchd; local execution authority; protocol client

/Users/mtn/Documents/Personal/SentinelProjects/TheDitchMobile
  iPhone client; QR scanning; device identity; remote UI

/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay
  Cloudflare Worker, D1, Durable Objects, notification Queue, APNs
```

The relay is connectivity, authorization, safe metadata projection, and
ciphertext routing. It is not cloud execution or application hosting.

## First: inspect reality

Before writing code, read all instructions and existing files in all three
repositories. In particular, read these Mac-owned protocol inputs completely:

```text
/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/REMOTE_PROTOCOL_V1.md
/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/openapi-v1.yaml
/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/schemas/
/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/fixtures/
/Users/mtn/Documents/Personal/The Ditch v2/docs/adr/ADR_REMOTE_CONTROL_V1.md
/Users/mtn/Documents/Personal/The Ditch v2/docs/security/REMOTE_CONTROL_THREAT_MODEL_V1.md
/Users/mtn/Documents/Personal/The Ditch v2/crates/ditch_remote/src/lib.rs
/Users/mtn/Documents/Personal/The Ditch v2/crates/ditchd/src/remote_control.rs
```

Inspect `TheDitchMobile` for its actual key formats, QR parser, signed REST
canonicalization, WebSocket envelopes, pairing state machine, encrypted command
format, projections, errors, and APNs registration. Do not assume the mobile
implementation already matches the contract.

A source handoff from the initial in-tree prototype may be available at:

```text
/tmp/TheDitchRelay-source-handoff-20260825.tar.gz
SHA-256: 25db46d4d283c6a949977b2fb1d13377e015d6e3d5861a0b83dcf0a3696da576
```

Treat it as reference code to audit, not as proof of correctness. Never copy
`node_modules`, generated Wrangler configuration, credentials, `.dev.vars`, or
production resource identifiers.

Before implementation, verify current official Cloudflare documentation for
module Workers, D1, Durable Objects, SQLite-backed Durable Objects, WebSocket
Hibernation, Queues, Web Crypto, Wrangler, custom domains, environment
configuration, secrets, and local Vitest/Miniflare testing. Verify current Apple
APNs token-authentication and provider requirements. Use primary documentation.

## Architectural invariants

Preserve these boundaries:

```text
ditchd = execution authority
Flutter desktop = local UI
PTY = local agent control
SQLite/event store on Mac = durable local truth
Git/worktree services on Mac = integration authority
TheDitchRelay = authentication, connectivity, projections, ciphertext relay
iPhone = remote command and attention surface
```

The relay must never:

```text
run Codex or another agent
own a PTY
accept a generic RPC
execute shell commands
write a filesystem
perform Git operations
host or upload repositories
store source code
store full transcripts
open an inbound Mac connection
use Cloudflare Tunnel
queue offline user commands
infer executable actions from agent/model prose
```

Both Mac and iPhone connect outbound:

```text
ditchd --authenticated WSS--> Worker --> MachineHub(machine_id)
iPhone --authenticated WSS--> Worker --> MachineHub(machine_id)

attention event --> Cloudflare Queue --> APNs --> iPhone
```

Cloudflare Queue is exclusively notification infrastructure. It must not retain
`session.start`, `session.prompt`, stop, approval, attention, integration, or any
other user command for later execution.

## Repository outcome

Create a standalone TypeScript project using current Cloudflare module-worker
conventions. A reasonable layout is:

```text
src/
  worker.ts
  machine_hub.ts
  types.ts
  http.ts
  crypto.ts
  auth/
  pairing/
  projections/
  commands/
  notifications/
migrations/
test/
scripts/
docs/
wrangler.toml or wrangler.jsonc
package.json
README.md
.dev.vars.example
```

Keep runtime dependencies minimal. Pin supported versions, commit the lockfile,
and keep all generated resource IDs and secrets out of source control.

## Canonical contract

Remote Protocol major version starts at `1`. Every REST request, response, QR
payload, WebSocket envelope, command, event, and encrypted payload identifies
the version. Unknown major versions are rejected with `unsupported_protocol`.

Import the Mac protocol documents, schemas, and fixtures without casually
rewriting them. Establish a versioned contract package/export so the Mac and
iPhone repositories can consume immutable artifacts in CI without runtime
repository dependencies. Include a manifest and SHA-256 checksums.

The contract bundle must contain:

```text
OpenAPI
REST JSON Schemas
WebSocket envelope schemas
command and event schemas
stable error codes
pairing QR schema
signature vectors
ECDH/HKDF/AES-GCM vectors
AAD vectors and tamper cases
projection sequencing fixtures
protocol version and checksum manifest
```

Do not claim Rust/TypeScript/Swift interoperability until all three execute the
same fixed vectors successfully.

## Identity and tenancy

Use application-domain identities:

```text
Owner
  Machine(s)
  Device(s)
```

An Owner is not a Cloudflare user/account. A new Mac registers with
`owner_id = NULL`. The first successfully confirmed device pairing atomically
creates an Owner and binds the machine and device. An existing authenticated
device may prove possession of its signing key while claiming a new unowned Mac
so that Mac joins its existing Owner. A new iPhone claiming an owned Mac remains
pending until explicit Mac confirmation and then joins that Owner.

Every authorization query must derive owner scope from the verified principal.
Never trust client-supplied `owner_id`. Never rely on UUID secrecy, global
defaults, first-row lookups, or single-user assumptions. Do not build teams,
organizations, invitations, RBAC, email accounts, passwords, OAuth, billing, or
account recovery.

## Cryptography

TLS is mandatory. Sensitive device-to-machine payloads are additionally
pairwise end-to-end encrypted:

```text
identity signatures: ECDSA P-256 / SHA-256
key agreement:       ECDH P-256
KDF:                 HKDF-SHA256
payload encryption:  AES-256-GCM
nonce:               12 random bytes, never reused with one key
```

Use separate signing and agreement public keys per principal. Cloudflare stores
public keys only. Pairwise derivation context must canonically bind protocol
version, owner ID, machine ID, device ID, and key version with explicit domain
separation. AES-GCM AAD must bind protocol, message/command ID, owner, machine,
device, command/event type, creation time, and expiry. Modified ciphertext,
nonce, or AAD must fail authentication.

Cloudflare may see routing/action metadata. It must not need plaintext prompt
bodies, full transcript pages, approval response text, or long agent responses.

## Signed REST authentication

Implement the exact canonical request defined by Remote Protocol v1:

```text
DITCH1
<METHOD>
<PATH_AND_QUERY>
<SHA256_BODY>
<TIMESTAMP_MS>
<NONCE>
<PRINCIPAL_ID>
```

Verify the exact UTF-8, newline, body hashing, Base64URL-without-padding, and raw
P1363 ES256 signature representation required by the contract. Required
headers are:

```text
X-Ditch-Protocol
X-Ditch-Principal-Type
X-Ditch-Principal-Id
X-Ditch-Timestamp
X-Ditch-Nonce
X-Ditch-Signature
```

Validate public key, non-revoked state, clock window, nonce uniqueness,
signature, route permission, and owner relationship. Persist nonce replay state
with expiry. Test both positive fixtures and modified method, path, query, body,
timestamp, nonce, principal, and signature cases.

## Pairing

Machine registration is unclaimed and authenticated by proof of possession of
the submitted signing key. A QR contains only:

```text
protocol version
relay origin
pairing_id
machine_id
one-time random secret
expiry
```

Generate at least 256 random bits for the secret and store only its hash. Use a
2–5 minute expiry, attempt rate limits, single use, explicit consumed state, and
idempotent crash-safe transitions:

```text
pending -> claimed -> consumed
pending/claimed -> expired | cancelled | rejected
```

Possession of a QR is not permanent trust. The Mac must receive the pending
device's safe display name and explicitly confirm it. Make claim winner
selection atomic (`UPDATE ... WHERE state='pending' RETURNING ...` or equivalent)
and make confirmation transactional/idempotent using D1-supported semantics.
Never include permanent credentials in the QR or logs.

## WebSocket tickets and MachineHub

Permanent tokens must never appear in WebSocket URLs. Implement:

```text
signed POST /v1/auth/socket-ticket
  -> random, hashed, short-lived, one-use ticket
GET /v1/socket?ticket=...
  -> atomic consume
  -> authorized routing to MachineHub(machine_id)
```

Bind each ticket and connection to protocol version, principal type/ID, owner,
machine, role, connection epoch, expiry, and consumed state. Revalidate current
revocation state at upgrade.

Use one SQLite-backed Durable Object per machine and current WebSocket
Hibernation APIs. Store principal metadata in WebSocket attachments and restore
it after hibernation/restart. Do not rely on volatile in-memory security maps.

MachineHub coordinates:

```text
exactly one current authenticated ditchd socket
zero or more authorized device sockets
fresh presence
live projection/event fan-out
live encrypted query relay
command delivery and ACK/result routing
immediate socket closure on revocation
```

Worker authorization must complete before entering a hub. Knowing a machine ID
does not grant hub access. A Mac is online only when its authenticated socket is
live and fresh; historical D1 `last_seen_at` is not online presence.

## Commands

The exhaustive v1 allowlist is:

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

Explicitly reject unknown commands and generic escape hatches including:

```text
shell.execute
pty.write
run_command
git.execute
filesystem.write
kill_pid
generic RPC
```

Require protocol version, command ID, idempotency key, owner/machine/device
target, exhaustive type, created/expiry timestamps, confirmation class, and
encrypted payload. Reject stale commands. Deduplicate mutations. Store only
privacy-safe audit metadata or ciphertext hashes, never plaintext payloads.

The Worker authorizes the device and owner, asks MachineHub whether ditchd is
freshly online, and returns `machine_offline` immediately when it is not. There
must be no command table/Queue that later redelivers an offline request.
Long-running local work returns delivery/acceptance separately from completion.
The relay never decides whether a requested confirmation is sufficient;
`ditchd` is authoritative and the mobile client cannot downgrade it.

## Projections and transcript relay

D1 may persist only sanitized projections. Project records may contain safe IDs,
display name, factual status/counts, attention count, activity timestamp, and
projection version. Session records may contain safe IDs, title, provider display
name, factual/semantic state, needs-user, counts, validation summary,
integration readiness, activity timestamp, and projection version. Attention
records may contain trusted IDs, type, severity, safe summary, state, and
trusted domain-generated remote actions.

Default-deny unknown fields. Never accept or store local paths, source, Git
objects, environment values, secrets, terminal logs, complete transcripts, or
arbitrary serialized Mac domain objects. Agent/model display text can never
become an executable action descriptor.

Use monotonic per-machine epoch and sequence. Detect duplicates, stale epochs,
out-of-order updates, and gaps. On a gap, ask the Mac for a full sanitized
snapshot. A new epoch begins with a full snapshot. D1 is a projection; local Mac
truth always wins during reconciliation.

Full transcript and review bodies remain on the Mac. MachineHub may relay their
E2E ciphertext with pagination, stable message IDs/order, bounded page sizes,
live continuation, cancellation, expiry, and routing correlation. Never persist
transcript ciphertext or plaintext in D1, logs, Durable Object storage, Queues,
analytics, or error reporting.

## D1 schema

Create migrations for at least:

```text
owners
machines
devices
pairings
push_devices
machine_projection_state
projects
sessions
attention
command_audit
replay_nonces
socket_tickets
rate_limits
audit_events
```

Add appropriate foreign keys, compound owner-scoped primary/unique constraints,
state constraints where supported, and indexes for owner queries and retention.
Machines allow nullable `owner_id` only while unclaimed. Do not add transcript,
source, repository, terminal, environment, plaintext command, or offline command
delivery tables.

Use bounded retention and a scheduled cleanup handler. Delete expired pairings,
nonces, tickets, old rate windows, and bounded audit metadata. Invalidate APNs
tokens upon device revocation or provider rejection. Deleting relay identity
must never delete local projects on a Mac.

## Device revocation and kill switch

Machine-authenticated revocation must be owner-scoped and immediately:

```text
mark the device revoked
invalidate APNs tokens
close every live socket for that device
make outstanding and future tickets unusable
reject future signed requests and commands
record privacy-safe audit metadata
invalidate pairwise key version/cache state where applicable
```

Machine disable/deletion must close all sockets and remove or disable remote
metadata without touching local Ditch data. Race-test revocation during request,
ticket upgrade, and command delivery.

## APNs

Use direct Apple token-based APNs authentication unless a verified current
platform limitation prevents it. Configuration:

```text
APNS_KEY_ID
APNS_TEAM_ID
APNS_PRIVATE_KEY
APNS_BUNDLE_ID
APNS_ENVIRONMENT=sandbox|production
APNS_GENERIC_CONTENT=true|false
```

Secrets belong in Cloudflare secret storage. Never commit `.p8` material.
Support provider JWT generation/refresh, sandbox and production hosts, token
environment tagging, token changes, invalid/unregistered tokens, retryable
429/5xx responses, expiry, collapse IDs, priority, and minimal payloads.

The notification Queue is only for meaningful attention such as finished,
failed, needs input, approval required, ready to apply, conflict, and unexpected
stop. Do not notify for tool calls, tokens, terminal lines, file modifications,
or heartbeats. Deduplicate by canonical attention identity so full projection
reconciliation cannot spam pushes. APNs is best-effort; durable attention state
remains the source shown after the app opens.

Never push source, prompts, secrets, terminal content, environment values, or
complete agent output. Provide generic-content mode and accurately document
which project/session display metadata Cloudflare and Apple can observe.

## API and errors

Implement the OpenAPI operations, including machine registration; pairing
create/poll/claim/confirm/cancel; socket tickets and socket upgrades; owner-scoped
machine/project/session/attention reads; push-token registration; device
revocation; machine disable/deletion; and command submission.

Return one stable error envelope:

```json
{
  "error": {
    "code": "machine_offline",
    "message": "The Mac is offline.",
    "request_id": "uuid",
    "retryable": false
  }
}
```

Support at least the contract's stable codes: `unauthorized`, `device_revoked`,
`machine_revoked`, `machine_offline`, pairing expiry/use/rejection,
`unsupported_protocol`, `invalid_signature`, `replay_detected`, command
expiry/duplicate, session/approval/action/integration states, validation failure,
and `rate_limited`. Never return internal stack traces or sensitive values.

## Rate limiting, audit, and observability

Rate-limit registration, pairing creation/claims, failed signatures, socket
tickets/upgrades, command submission, transcript queries, and push-token
changes. Avoid limits that break normal active chat. Use stable typed failures.

Keep privacy-safe audit events for pairing, revocation, consequential command
accept/reject, force kill, approval, integration, and authentication failure
thresholds. Correlate request, command, device, machine, and safe session IDs.
Never record prompt or transcript text.

Expose structured operational signals for connection counts, pairing outcomes,
command results/ACK latency, projection lag/gaps, APNs status classes, invalid
tokens, D1/DO failures, auth failures, and rate-limit hits. Redact secrets,
signatures, tickets, QR secrets, device tokens, plaintext, and ciphertext.

## Required tests

Use current Cloudflare-supported local integration tooling. Tests must not
depend on public Internet or production credentials. Exercise real D1 migrations
and Durable Object behavior rather than mocking every boundary.

Cover at least:

```text
D1 migration and forbidden-column inspection
machine self-registration proof
signed REST canonical fixtures and every tamper case
clock skew and nonce replay
owner A denied every owner B resource/operation
unclaimed first-owner pairing
existing-device second-Mac pairing
owned-Mac second-iPhone pairing
atomic pairing winner, expiry, reuse, cancel, restart/idempotency
one-use socket tickets, expiry, stolen/replayed ticket
hub role enforcement and hibernation attachment restoration
single current machine connection and multiple authorized phones
fresh presence versus stale D1 last_seen
machine offline immediate failure and absence of a command queue
unknown/stale/duplicate commands
ACK/result correlation and disconnect-before-ACK
revocation during requests and socket closure
projection duplicate, out-of-order, gap, stale epoch, full reconciliation
field allowlist and agent-content action injection
transcript ciphertext relay with proof it never persists
APNs payload formatting, environment separation, invalid token, retry
notification deduplication across repeated snapshots
D1 unavailable and Durable Object restart/failure injection
```

Create a deterministic fake Mac and fake phone harness speaking Remote Protocol
v1 so CI can prove pairing, socket establishment, projection sync, command
delivery, offline rejection, reconnect, and revocation without Cloudflare or
Apple credentials. Do not claim complete end-to-end product execution for
actions that the current Mac daemon explicitly fails closed.

## Deployment

Provide repeatable `development`, `staging`, and `production` configuration.
Local tests must use local emulators and must never point at production.

The staging/production bootstrap should:

```text
verify Wrangler authentication/account
create or locate environment-specific D1
create or locate notification Queue and DLQ
write ignored generated configuration with resource IDs
apply D1 migrations by binding
configure Durable Object export/migration correctly
prompt for Cloudflare secrets without echoing them
deploy Worker
print health URL and exact DNS/custom-domain next steps
```

Make reruns safe: discover existing resources instead of failing or creating
duplicates. Do not require resource IDs to be pasted into source. Document
scoped API-token requirements for CI without committing a token.

Support a staging `workers.dev` origin initially and a production custom domain
such as `remote.theditch.app`. Document DNS, Worker custom-domain configuration,
TLS, `/health`, logging/tailing, rollback, migrations, secret rotation, APNs
sandbox smoke testing, and production promotion. Do not deploy or change DNS
without explicit human authorization.

## Documentation and delivery report

Write a README that clearly distinguishes:

```text
Cloudflare account owner = The Ditch operator
Ditch Owner row = anonymous application identity
Machine = one Mac daemon identity
Device = one independently revocable phone identity
```

Document privacy, data retention, recovery limitations, threat boundaries,
incident/revocation operations, environment separation, human credential gates,
and exact commands for local development and deployment.

Before completion, audit explicitly for remote shell escape, generic RPC,
Cloudflare Tunnel, private keys, hardcoded credentials, permanent QR tokens,
weak entropy, replay, stale tickets/commands, cross-owner access, offline command
retention, untrusted action injection, transcripts/source in D1, sensitive push,
unbounded logs, APNs environment mistakes, unknown protocol execution, and
revocation races. Fix findings and rerun all checks.

Final output must report:

```text
architecture and repository boundaries
files and dependencies
protocol version/checksum
D1 schema and migrations
MachineHub behavior
auth and pairing lifecycle
projection and transcript behavior
command/offline semantics
revocation/deletion
APNs implementation
rate limits, retention, audit, observability
tests with exact results
Cloudflare and Apple credentials still required
deployment commands and domain steps
security limitations and remaining known work
```

Do not claim production-ready, secure, cross-language E2E verified, APNs
verified, real-device verified, or deployed unless the corresponding evidence
was actually produced.
