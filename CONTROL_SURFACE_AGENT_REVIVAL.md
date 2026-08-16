## Revised macOS recovery and emergency-termination requirements

The previous requirements concerning UI crashes, daemon reconnection, and emergency termination are replaced by this section.

### macOS limitation

Do not assume that `ditchd`, a `LaunchAgent`, a `LaunchDaemon`, an `SMAppService` helper, or an `LSUIElement` agent application will appear in the macOS Force Quit Applications window opened with `Option+Command+Escape`.

Do not repackage the runtime as a normal foreground application solely to force it into that window.

The normal The Ditch application may appear in Force Quit if its existing macOS application configuration permits it. Force-quitting the UI application is not proof that the user intended to stop active agents. `ditchd` must not terminate agents merely because the UI process disappeared unexpectedly.

### Required process structure

Use these responsibilities:

#### The Ditch macOS application

Owns:

* native macOS application lifecycle;
* status-bar item;
* main Flutter window;
* quit confirmation;
* daemon connection and recovery interface.

Does not own:

* Codex or agent PTYs;
* agent process handles;
* agent process groups;
* authoritative runtime state.

#### `ditchd` / The Ditch Runtime

Owns:

* PTYs;
* Codex and agent processes;
* verified process groups;
* runtime/session state;
* recent reconnectable output;
* graceful interruption and force termination;
* shutdown sequencing.

Package and register it using the repository’s appropriate supported macOS service mechanism. If the minimum supported macOS version is 13 or later, evaluate `SMAppService`. Record the final choice in an ADR.

The process and packaged executable must have a recognizable name, preferably:

`The Ditch Runtime`

It must be identifiable in Activity Monitor and diagnostics. Do not leave the user looking for a generic process such as `main`, `runner`, or an opaque hash.

### UI crash behavior

If the main UI process crashes or is force-quit:

* `ditchd` continues running;
* active owned agents continue running;
* UI disconnection is recorded separately from runtime termination;
* relaunching The Ditch attempts to reconnect and hydrate an authoritative snapshot;
* agent sessions are not duplicated;
* UI disappearance never projects agent success, failure, or cancellation by itself.

### Reconnection failure behavior

If The Ditch launches but cannot connect to an existing registered runtime, do not show the ordinary empty state and do not silently launch a second daemon.

Show a native recovery screen before the normal Flutter command center:

Title:

`The Ditch Runtime is not responding`

Body:

`Agent processes may still be running. The Ditch could not reconnect to its local runtime service.`

Display available diagnostics:

* service registration state;
* expected executable name;
* daemon PID when safely observable;
* socket path;
* socket existence and ownership;
* last successful heartbeat;
* number of sessions last known to be active;
* last startup or connection error;
* path to redacted diagnostics.

Actions:

1. `Retry Connection`
2. `Restart Runtime`
3. `Force Stop Runtime and Agents`
4. `Open Activity Monitor`
5. `Export Diagnostics`
6. `Quit UI`

Do not enable `Restart Runtime` until the current runtime is proven stopped. Never start two daemons against the same state database or worktree leases.

### Retry Connection

`Retry Connection` must:

* retry with a bounded timeout;
* rediscover the registered service through its stable service identity;
* reconnect to the existing IPC endpoint;
* request a complete authoritative snapshot;
* resume event streaming from the last acknowledged sequence where supported;
* never start replacement agent sessions.

### Restart Runtime

`Restart Runtime` must:

1. request graceful runtime shutdown if reachable;
2. verify that the registered daemon stopped;
3. never kill agent processes using stale persisted PIDs;
4. start one registered runtime instance;
5. reconnect;
6. reconcile previous sessions honestly.

If the old runtime cannot be proven stopped, refuse restart and direct the user to the force-stop path.

### Force Stop Runtime and Agents

This is an emergency destructive action. It requires explicit native confirmation:

Title:

`Force stop The Ditch Runtime?`

Body:

`This will terminate The Ditch Runtime and every active agent process that it can verify it owns. Unsaved agent work may be incomplete. Git worktree changes and recorded history will be preserved where possible.`

Actions:

* `Cancel`
* `Force Stop Runtime and Agents`

The force-stop implementation must:

1. identify the runtime through its registered service identity;
2. stop or unregister the service using the supported macOS service-management mechanism;
3. terminate only agent process groups for which current ownership can be verified;
4. reject arbitrary or stale PIDs;
5. avoid shell interpolation;
6. wait for a bounded shutdown period;
7. verify whether the daemon and owned process groups exited;
8. preserve the database and worktrees;
9. write a recovery marker when possible;
10. report partial failure honestly;
11. never claim agent completion.

If the daemon is unreachable and its in-memory ownership registry cannot be queried, do not blindly kill child PIDs loaded from SQLite. Stop the registered runtime service first. Any remaining processes must be surfaced for manual inspection unless ownership can be established through a stronger mechanism such as an ownership nonce, verified parent/process-group relationship, and executable identity.

### Activity Monitor escape hatch

Provide an `Open Activity Monitor` action that opens Activity Monitor.

Show this instruction in the recovery UI:

`Search for “The Ditch Runtime”. Force Quit Applications does not list macOS background services.`

Do not claim that `Option+Command+Escape` will display `ditchd`.

The packaged runtime must be named clearly enough to locate in Activity Monitor.

Document the exact manual fallback:

1. Open Activity Monitor.
2. Search for `The Ditch Runtime`.
3. Select it.
4. Choose Stop.
5. Use Force Quit only if normal Quit fails.
6. Relaunch The Ditch and run recovery.

### Command-line recovery tool

Provide or extend a bundled recovery command:

`ditch runtime status`
`ditch runtime reconnect`
`ditch runtime stop`
`ditch runtime stop --force`
`ditch doctor`

Requirements:

* use the same service identity and IPC contract as the application;
* print the runtime identity, health, PID when observable, socket state, and active-session count;
* require confirmation for force stop unless `--yes` is explicitly supplied;
* never accept an arbitrary PID as the runtime target;
* return nonzero on incomplete shutdown;
* produce no secrets in diagnostics.

Do not require the user to know `launchctl` syntax for normal recovery.

### Explicit normal quit

Normal explicit quit from the status-bar menu, application menu, or `Cmd+Q` remains coordinated:

* if no sessions are active, request daemon shutdown and quit;
* if sessions are active, show `Quit and Stop Agents` confirmation;
* on confirmation, gracefully interrupt agents, force-kill only verified remaining owned process groups after a timeout, persist outcomes, stop the daemon, and quit;
* on cancellation, keep the application, daemon, and agents running.

Normal quit and emergency force stop must be separate code paths.

### Required tests

Add tests for:

* UI process disconnect does not stop fixture agents;
* forced UI-process termination does not send daemon shutdown;
* relaunch reconnects to the same daemon and session;
* connection failure shows recovery mode instead of an empty dashboard;
* retry does not create a second daemon;
* restart refuses while the existing daemon cannot be proven stopped;
* force stop targets the registered service, not an arbitrary PID;
* stale persisted PIDs are rejected;
* owned fixture process groups are stopped;
* unrelated fixture processes remain alive;
* partial force-stop failure is reported;
* recovery marks affected sessions interrupted, crashed, or stale rather than completed;
* command-line and GUI recovery use the same runtime-management implementation.

### Scope restriction

Do not create a second normal macOS application named `The Ditch Runtime` merely to make it appear in the Force Quit Applications window.

Do not add a permanent Dock icon for the runtime helper.

Do not claim Force Quit Applications compatibility without a real macOS test proving the exact packaged configuration appears there. Apple’s documented default for agent applications is that they do not appear there.

Finish by reporting:

* the selected service-management mechanism;
* how the runtime is named and located in Activity Monitor;
* behavior after UI crash;
* behavior after failed reconnection;
* normal quit versus emergency force-stop semantics;
* tests proving unrelated processes cannot be terminated;
* any behavior that still requires real packaged-app verification.
