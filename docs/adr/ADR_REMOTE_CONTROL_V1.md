# ADR: Remote Control Plane v1

Status: accepted for implementation

## Context

Ditch is local-first: Flutter is a local client and `ditchd` owns processes,
PTYs, session lifecycle, filesystem operations, and SQLite truth. Remote mobile
control needs reachability without opening a Mac port and without turning a
cloud database into a second runtime authority.

The current product has working session start, prompt, graceful stop, transcript
pagination, projects, and attention. Interactive approvals and worktree
integration are not implemented in the shipped runtime. Remote v1 must fail
closed for those unavailable use cases until their normal desktop application
services exist.

## Decision

Add an optional `remote-control` adapter owned by `ditchd`. The daemon initiates
an authenticated WSS connection to one Ditch-managed Cloudflare Worker. The
Worker authorizes requests using D1 metadata and routes sockets into one
Durable Object per machine. Phones also connect outbound. No Cloudflare Tunnel,
inbound Mac listener, generic RPC, shell, PTY, or filesystem operation exists.

Machine private keys live in macOS Keychain and are owned by `ditchd`; only
public keys and opaque identifiers enter SQLite. Sensitive phone↔machine payloads
are pairwise encrypted above TLS. D1 stores only anonymous relationships,
sanitized projections, attention metadata, push tokens, and privacy-safe audit
metadata. Transcript pages are live ciphertext relay only.

Remote command adapters invoke the same daemon functions used by the local IPC
dispatcher. The command allowlist is exhaustive. Idempotency and expiry are
checked before execution. Offline commands fail synchronously and are never
queued. Cloudflare Queue is named and used only for APNs delivery.

## Consequences

Local use remains available with Remote Control disabled or unreachable. The
privacy statement must no longer say that no Ditch metadata leaves the Mac when
Remote Control is enabled. Cloud availability affects remote visibility, not
local execution. There is intentionally no account recovery if every paired
device and machine is lost.

The Cloudflare service is separately deployable TypeScript and adds no Workers
dependencies to Rust or Flutter. Protocol schemas and fixtures are exported as
a versioned bundle for the separate iPhone repository.

