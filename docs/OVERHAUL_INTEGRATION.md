# Overhaul integration — 2026-09-27

This integration branch starts from released Community main `9d83c13615c7d5e228f27ec8918fdfa5c6020002`, merges parked recovery `637d594d54ac6e4bc9703fc5e233eaa59697ac89`, then overhaul `d0ec6b9cbfbf95728cab9a63482d0b28ce1a3718`.

## Resolved behavior

- Keep remote protocol 4, operation receipts, event replay/epoch checks, project isolation, paginated rejoin, and the detached SSH runtime lifetime.
- Preserve task boards, skills, acceptance attempts, immutable review evidence, writer leases and explicit human acceptance. Task snapshots retain generation checks alongside event replay sequence checks.
- Use bounded App Server discovery, explicit skill inputs and task containment together with session recovery, namespaced request IDs, confirmed approval responses, cancellation/interrupt, model tracking, and output-drain-before-finalization.
- New desktop sessions default to App Server. Local legacy sessions remain explicitly available without skills; task acceptance cannot silently fall back. Existing threads retain their skill set. Local and SSH approval presets match `runtime-sessions.md`; unsupported SSH sandboxing fails rather than silently becoming unrestricted. Remote reservations remain conservatively exclusive.
- Keep per-project composer settings; selected skills, model and reasoning effort survive profile hydration and follow-ups.
- Preserve hardened runtime signing and refresh signed resource checksums; unsigned validation builds skip signing.
- Fix released issue #16: focused terminal applications receive Escape. Outside the terminal, the existing pane restore shortcut remains.
- Fix released issue #22: inferred remote project names follow directory navigation; a manually entered name is preserved.
- Carry generic server-enabled extension capability IDs in the same sanitized entitlement snapshot. No private implementation moves into Community.
- Ignore Finder `.DS_Store` metadata in the runtime build fingerprint.

## Validation and release gate

Rust workspace tests, strict Clippy, Flutter analysis/tests, Community source isolation, offer-contract validation and native ad-hoc runtime signing tests are run during integration. The focused regression suite includes terminal Escape, remote directory naming, session approval/rejoin/model/stop lifecycle, and stale task snapshots.

This is an integration candidate, not a released build. Native Codex containment checks were attempted but the execution environment rejected nested `sandbox-exec` (`sandbox_apply: Operation not permitted`). Xcode package resolution also encountered denied writes to the user's SwiftPM cache, even with DerivedData redirected to temporary storage. Those are unpassed gates, not waived checks.

Before release, run native containment and cancellation tests in a normal terminal, authenticated local and SSH session smoke tests, the packaged Mac → SSH runtime flow with disconnect/reconnect, and an isolated upgrade from 0.1.0.132. Then build/sign/notarize and validate fresh staging artifacts. Publish and advance main only after those gates pass. Existing planning documents describe their original branch snapshots; this document records the combined behavior.

## Recorded automated results

Community: **227 Rust test executions passed**, 8 ignored (four live tests repeated in lib/bin targets); **164 Flutter tests passed**.

Both editions pass Rust workspace debug builds, strict Clippy (`--all-targets --all-features -- -D warnings`), formatting and Flutter analysis. Community source and debug binary isolation pass, as does the offer contract. Four real Mach-O/ad-hoc signing tests and 29 release-script fixture tests pass. Native app packaging, model/SSH acceptance, upgrade acceptance, signed staging and production remain unverified as described above.
