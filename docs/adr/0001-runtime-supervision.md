# ADR 0001: Status helper supervises The Ditch Runtime

## Status

Accepted.

## Decision

The foreground AppKit/Flutter application and the menu-bar status helper remain
separate processes. The status helper is the only UI process that starts and
stops `ditchd`. The foreground application only checks availability, connects,
hydrates an authoritative snapshot, and sends agent commands.

The helper is packaged as an `LSUIElement` login item. On macOS 13 and later the
main application registers it with `SMAppService.loginItem`; macOS 10.15–12 use
`SMLoginItemSetEnabled` while that deployment target remains supported.

`ditchd` owns agent child processes and durable runtime state. Losing or quitting
the foreground UI never sends runtime shutdown. Explicitly quitting the status
helper confirms active-session termination, asks `ditchd` to stop, and exits only
after the request succeeds or the runtime was already unavailable.

## Consequences

- A Flutter/AppKit crash does not stop active agents.
- Relaunching the UI reconnects instead of creating sessions or daemons.
- Status presentation is derived from `ditchd`, not pushed from Flutter memory.
- Raising the deployment target to macOS 13 can remove the legacy registration
  fallback in a future change.
