# Remote Control Threat Model v1

## Assets and trust boundaries

Highest-value assets are machine/device private keys, prompt and transcript
content, approval decisions, integration authority, runtime ownership, source
code, and cross-owner isolation. `ditchd` and macOS Keychain are trusted. The
Flutter UI is a local client. Agent/model/repository content, the network, phone
display strings, QR screenshots, APNs, and all client-supplied identifiers are
untrusted. The relay is trusted for authorization and routing but is not trusted
with sensitive payload plaintext.

## Threats and controls

| Threat | Control |
|---|---|
| Known UUID accesses another owner | Every lookup derives owner from verified principal and includes owner/machine predicates; isolation tests cover all resources. |
| Leaked QR | 256-bit one-use secret, hash-only storage, three-minute expiry, rate limit, explicit Mac confirmation. |
| REST replay | Signed canonical request, five-minute window, nonce consumption before side effects. |
| Socket-ticket theft | Random one-use 30-second ticket bound to principal, owner, machine, role, protocol, and epoch. |
| Command replay or delivery ambiguity | Unique command/idempotency keys, expiry, local durable outcome ledger, no offline queue. |
| Relay reads prompt/transcript | P-256 ECDH, HKDF-SHA256, AES-256-GCM with routing metadata in AAD. |
| Ciphertext retargeting | AAD binds protocol, IDs, type, and timestamps; mismatch fails authentication. |
| Agent text injects an action | Display text and trusted action descriptor types are separate; descriptors are created only by Ditch domain code. |
| Remote shell escape | Exhaustive command enum; no generic RPC, PTY write, filesystem, Git, signal, cwd, env, or binary fields. |
| Foreign PID kill | Runtime acts only on a live child/process group owned by the target session. |
| Stale approval/apply | Revalidate pending operation and target state at daemon execution time. Unimplemented canonical services return `action_not_allowed`. |
| Revoked socket remains useful | D1 state changes immediately; hub receives close request; future REST/tickets fail; daemon revalidates device on command. |
| APNs leaks content | Minimal IDs and generic copy by default; no prompt, transcript, terminal, source, or secret data. |
| Projection leaks local data | Explicit outbound DTO allowlist; never serialize domain objects wholesale; tests reject root/current prompt/evidence fields. |
| Projection becomes authority | Epoch/sequence reconciliation converges D1 to SQLite; no cloud projection mutates runtime. |
| Notification queue executes command | Queue binding accepts only `PushJob`; command gateway checks live machine socket and has no persistence path. |
| Key extraction from SQLite/files | Private key material is stored only in Keychain; SQLite holds Keychain labels and public keys. |

## Residual risks and v1 limits

Compromise of an unlocked Mac process or unlocked paired phone can exercise the
user's authority. Device-owner authentication is asserted by the mobile client
and bound into the signed/encrypted command; the separate iPhone implementation
must use LocalAuthentication and protected key storage. Project/session names,
status, attention summaries, APNs device tokens, routing types, timing, and audit
metadata are cloud-visible. Traffic analysis remains possible.

Keychain-backed software P-256 keys are the compatibility baseline. Secure
Enclave-backed key agreement/signing requires real-device validation and is not
claimed here. APNs delivery and production Cloudflare configuration require
external credentials and must not be called verified by local tests.

## Release blocker checklist

- No remote shell, PTY write, generic command, source upload, or transcript D1 column.
- No Flutter-owned identity or remote socket.
- No Cloudflare Tunnel or inbound Mac port.
- No private key in SQLite, files, fixtures, logs, or QR.
- Unknown protocol/command fails closed.
- Offline commands are rejected and never persisted for delivery.
- Owner predicates cover lists, detail reads, commands, sockets, push tokens, and revocation.
- Revocation closes live sockets and invalidates push state.
- Approval/integration return unavailable until canonical local services exist.

