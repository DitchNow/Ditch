# Ditch Computer Use

Ditch embeds Peekaboo's automation libraries in its macOS runtime. Codex remains
the agent orchestrator. A separately installed Peekaboo app, CLI, or agent is not
used by this integration.

## Availability

- Select a custom Ollama model and **Full Access** for a local user conversation.
- The selected model must declare tool support. Image observations additionally
  require declared vision support; accessibility observations work without vision.
- macOS 15 or later and Ditch's integrated macOS runtime are required.
- OpenAI sessions retain their existing tools and behavior. SSH/headless,
  mobile-origin, naming, planning, acceptance and unattended worker turns do not
  receive this desktop integration.

The four tools are `desktop_status`, `desktop_list_targets`, `desktop_observe`,
and `desktop_act`. Discover applications, then their windows, using opaque
`target_id` references from the current turn. Focus a selected window before
observing. The default `mode=visual` returns a screenshot without traversing
Accessibility; its single-use snapshot supports foreground click, double/right
click, move, drag, type, key and scroll. Coordinates are pixels of the returned
image, transformed into global display points by the native adapter.
`mode=accessibility` returns elements for the existing background action path
and captures no screenshot unless `include_image=true`.

Target identity uses Peekaboo's kernel process-start identity, not the optional
LaunchServices `NSRunningApplication.launchDate`. References remain scoped to the
turn, process generation, bundle and window. If identity cannot be read, the
adapter reports `processIdentityUnavailable`; it never authorizes input using a
bare PID or describes this as a permission denial.

Discovery returns `targets`, `unavailable` candidate diagnostics, and `status`:
`ready`, `partial`, `unavailable`, `noMatches` or `noVisibleWindows`. One failed
candidate does not discard other usable targets. Only entries in `targets` have
actionable references. An empty discovery is a successful enumeration with no
usable target, not proof of control: it consumes the runtime recovery budget just
like a failed discovery. Status checks and successful lists cannot reset that
budget. A successful observation or action can restore progress. Malformed
arguments, including legacy PID/window fields, report `invalidArguments`.

Application launching, desktop-wide/status-bar targets and arbitrary browser DOM
operations are not exposed by these four tools. This is window automation, not
the entire Peekaboo CLI surface.

Ditch's own application windows are eligible targets, including Kanban and task
forms. Select the exact application returned by discovery: Staging uses
`ai.theditch.app.staging`, while production uses `ai.theditch.app`. A Staging
request must not fall back to production. Creating a task through the New Task
form uses the existing `CreateBacklog` operation; it does not require a web UI.

System Settings, SecurityAgent and loginwindow remain protected targets. This
adapter policy is independent of macOS permissions; `protectedTarget` does not
mean an application's Accessibility API is disabled. Allowing Ditch windows does
not change the Full Access requirement, snapshot validation or lease revocation.
It does not establish a boundary against an agent using Ditch's own settings UI:
the signed-UI check on permission setup authenticates the application, not whether
a human or an automation tool clicked a control.

## OSS tool routing

Each launch now persists a sanitized `desktop-setup` record in `runtime.log` with its agent/run
IDs and an explicit readiness reason. Expected ineligibility (for example,
`requiresFullAccess`, `remoteExecution` or `nativeUnavailable`) permits ordinary
chat with that reason in the current developer instructions. A missing bundled
adapter, unreadable endpoint metadata or failed transport is a setup failure and
stops the model turn. These failures must not silently become chat-only sessions.

After thread discovery, `Ditch OSS turn setup` records the selected endpoint ID,
model, thread, policy version, adapter PID and verified tool names. It deliberately
reports `inference_delivery_verified: false`: submitting instructions to Codex is
not proof of their delivery to inference. Native operations emit a separate
`desktop-operation` record in that log with the same run ID, operation ID, state and
allowlisted error code. No screenshots, captured text, credentials or raw tool
arguments are logged. A completed dispatch still requires a fresh observation.

`oss_tool_policy.rs` scopes inherited MCP servers and installed/configured plugins
out of each OSS thread. `features.apps=false` alone does not disable plugin MCP
servers. The policy sets per-thread `config.plugins.<id>.enabled=false` and sends
`disabledPluginIds` on every `turn/start`, including follow-ups and resumes.
It leaves OpenAI launches and the user's Codex home configuration unchanged.
Codex 0.166.0 explicitly describes `disabledPluginIds` as saved metadata that does
not yet filter capabilities. Ditch therefore relies on the configuration override
and effective inventory verification, not on that field alone.

Before inference, Ditch first queries `mcpServerStatus/list` with both `threadId`
and `serverName: "ditch_tools"` (`detail: "toolsAndAuthOnly"`). Codex 0.166.0
reuses the thread's execution adapter only for this named query. Ditch verifies
its catalogue and captures its authenticated process ID, then audits the broader
inventory and confirms that the original execution process is still alive and
unchanged. A single deadline bounds these discovery steps.
Only disabled inherited servers and, when eligible, the connected `ditch_tools`
server with exactly the four expected tools are accepted. An unknown/active server,
missing adapter, failed discovery, or incompatible Codex produces a startup error;
no model turn is sent. A turn without an eligible native integration can still chat
and code with desktop tools omitted and explicit unavailable instructions.

During `tools/list`, the bundled adapter authenticates to its private runtime socket
with a non-tool handshake. The broad inventory query can start a separate CLI
process even when `threadId` is supplied. Such a helper may authenticate and inspect
schemas; it cannot replace the execution owner or invoke any desktop tool, including
`desktop_status`. Both discovery and execution still require the exact signed CLI,
the run credential and an unrevoked lease. Discovery does not call Peekaboo. An
exited execution adapter requires a fresh turn; authority is never transferred to
the discovery helper.

Registration is not considered complete merely because
the CLI advertises schemas. Status reports `native_available`, `eligible`,
`adapter_connected`, and `tools_registered` separately. The UI shows **Connecting**
until registration succeeds, then **Peekaboo**. Activity still comes from actual
runtime operations, never assistant prose. Handshakes contain no observation data
and do not create action receipts.

Turn instructions require list–observe–act–observe and truthful errors. Ditch's own
apps and status/menu-bar controls remain unsupported; use an ordinary disposable
window for acceptance testing. The instructions discourage shell/AppleScript
substitutes; Full Access shell execution is still available for coding, so this is
tool routing rather than a security boundary against arbitrary shell programs.

## Permissions and Stop

Full Access is the sole Ditch authorization. There is no extra desktop grant or
per-click approval. The existing Full Access confirmation explains computer
control when an Ollama model is selected.

The **Computer use** status beside the model controls opens setup. Grant
Accessibility and Screen Recording to **The Ditch Runtime**, or **The Ditch
Staging Runtime** for staging. Input-control setup is also available when needed
by macOS. These are OS grants to the embedded host, not to standalone Peekaboo.
Ditch can request them but cannot grant them for you. macOS may require a restart.

Stop revokes the run credential, cancels native work and stops the Codex turn.
Changing away from Full Access or changing the model waits for runtime revocation
before updating the selection. If cancellation cannot finish within its deadline,
Ditch reports that input is still stopping and prevents further desktop dispatch.
Physical actions already delivered cannot be undone. Wake/unlock does not resume
revoked input; submit a new turn.

## Per-turn visualizer

The prompt box shows a **Visualizer** switch only for custom Ollama/OSS models.
It starts off and remembers the choice per agent for the current app session.
Full Access, a local desktop, native visualizer support, and macOS permissions
are required to enable it. OpenAI model controls never show this switch.

Send captures the choice for that turn. While submitting or running, the switch
is labelled **Visualizer · next turn**; changing it does not alter the active
turn. Failed submissions retain the choice. Turning it off hides feedback only;
it does not disable computer use or change permissions.

Visual actions move the physical pointer through Peekaboo. A non-interactive
Ditch cursor indicator samples its real position during movement/dragging; it
does not replay a guessed path. Dispatch shows a click/input indicator, and failed
input shows a red error indicator. The native callbacks schedule feedback without
delaying dispatch. The indicator does not prove application-level success.

Background Accessibility actions retain target outlines/pulses and never pretend
to move the pointer. Hidden or covered background targets receive no outline.
Typed text and captured labels never appear in overlays. Fresh observations
remain necessary to verify results.

`turn_options.computer_use_visualizer` is optional request metadata, defaulting
to false. The runtime captures it on the eligible run's lease and supplies it to
the native adapter independently of model tool arguments. The native host owns
non-interactive overlays; it clears them before observation/input, on expiry,
cancel/revocation, desktop lock/sleep, and app/Space changes. Capture starts only
after overlay removal. No overlay data is persisted or relayed.

## Architecture and data

```text
Local Ditch UI -> accepted Full Access Ollama turn -> Codex app-server
                                                      |
                                       bundled ditch_cli tools serve --stdio
                                                      |
                                    private per-run authenticated Unix socket
                                                      |
                               Community runtime integration registry / receipts
                                                      |
                                      async C ABI in the signed Ditch StatusHost
                                                      |
                                  DitchAutomation.framework -> Peekaboo -> macOS
```

Commercial composes the same Community sources. `ToolIntegration` is the provider
boundary for future integrations; model orchestration is not duplicated.

The adapter uses the official `rmcp` SDK, with bounded requests/results and a
stable tool catalog. Codex thread/turn identities bind each run. Release IPC peers
must have matching live code signatures, team and expected executable identities;
debug builds also support explicitly ad-hoc-signed bundled executables.

An OS-backed lock prevents simultaneous desktop operations from Ditch instances.
Snapshots are single-use and discarded on revocation or replacement. Accessibility
snapshots expire after 30 seconds; visual snapshots after 60 seconds. Target
references bind the turn, process launch time, bundle, window ID and bounds.
Foreground input also checks the active application/window and point occlusion;
drag requires an unobscured window. Accessibility actions retain Peekaboo's
exact-target checks. Partial or uncertain input returns `outcome_unknown` and
must be observed before another action; it is not automatically replayed.

The runtime limits each turn to 120 non-status operations, 40 observations, three
observations without a successful action, and three consecutive observation/input
failures. Four identical image observations of the same target stop further
desktop work even if dispatches reported success. Only an in-memory image hash is
used for this check. A fresh user turn resets limits. These bounds stop repeated
tool loops; they cannot guarantee that Qwen interprets every screen correctly.

Images are bounded to 1280 pixels on the longest edge and 2 MiB encoded PNG;
accessibility summaries to 32 KiB and complete native responses to 3 MiB. Raw
observations go to the selected model via Codex. Ditch receipts and the opt-in UI
event stream contain operation metadata, not screenshots or typed text. Raw MCP
results are not added to Ditch's normal transcript or Relay events. Assistant
summaries remain normal conversation text. Codex's local thread history may
retain observations as model context; Ditch does not erase that history.

## Building

Initialize the pinned source without fetching Peekaboo's unrelated app submodules:

```sh
git submodule update --init vendor/peekaboo
```

In a Commercial checkout, use `git -C community submodule update --init vendor/peekaboo`.
The exact pin is `70dbc4183f390a5a72d5dc01d8a77eb672217070` (v4.9.8).
Use an Apple Swift 6.2+ toolchain (Xcode 26+). The implementation was built with
Swift 6.3.3. The optional framework has a macOS 15 deployment target and is loaded
only from the embedded host bundle on supported systems.

The existing macOS packaging phase builds the dynamic library, wraps/signs it as
a framework before signing its containing host, and includes dependency notices.
Only the engine ships; `DitchAutomationProbe` and the synthetic MCP fixture do not.
Settings → **Computer Use Licenses** displays the bundled full notices. The
framework resources also contain `desktop-dependencies.json` with pinned native
dependencies and the exact MCP/async Cargo dependency graph for that edition.

## Validation status and remaining release checks

### November 14 independent-review follow-up

The affected live Qwen session invoked two independent `codex exec` children
through its Full Access shell. Their traces reported an OpenAI provider. The
managed thread's CUA exclusion did not constrain those child processes. The
current host execution path therefore does **not** satisfy whole-run Qwen-only
inference isolation. Do not interpret catalogue verification as containment.

The follow-up source changes reject missing, null, conflicting, or mismatched
provider/model metadata in OSS thread creation/resumption before tool discovery
or `turn/start`. New/resumed launch regressions pass in both editions. This
hardens the managed thread; it does not close the descendant-process escape.

Native errors now return fixed diagnostic categories and recovery text instead
of flattening every upstream failure to `operationFailed`. Capture coordination
includes only typed stage/reason values. Captured text, file paths, and arbitrary
upstream messages are not serialized. Three focused native diagnostic tests pass;
the production Swift module type-checks against the existing built dependencies.
The package test runner could not write an existing build output, so the diagnostic
tests were compiled separately against those dependencies and run through XCTest.
Zero PID/window IDs are rejected before attempting an observation.

No new signed application was deployed by this follow-up. The original Terminal
observation failure has not been reproduced with the new diagnostics. No desktop
action, menu-bar operation, or animated cursor has been qualified by these tests.

Whole-run isolation remains gated on an execution-environment decision and proof.
The implementation session cannot apply even a minimal macOS sandbox profile
(`sandbox_apply: Operation not permitted`); this limits local validation and does
not prove that sandboxing cannot work on the target machine. The user was asked
to choose an isolated local environment or a separate execution host. Either
still requires a controlled host desktop bridge: unrestricted control of an
authenticated host terminal/application would defeat the boundary.

The historical validation notes below retain their original scope.

Earlier staging debug bundles built. Native cancellation/ownership tests, runtime
policy/dedup tests, focused Flutter regressions, Codex session/approval regressions
and nested signing tests pass. The isolated live Qwen3.5:9b probe passed tool
discovery, thread resumption, recall and synthetic image interpretation.

The November 14 routing repair additionally passes a real Codex 0.166.0 regression
with a harmless local CUA fixture plugin: the old apps-only configuration exposes
the plugin, while the new policy excludes it on both new and persisted/resumed
threads. This test uses a disposable Codex home and an offline inference provider;
it also checks the exact four-tool Ditch catalogue with a harmless MCP fixture,
both when enabled and omitted.
It does not establish real Qwen desktop-action or visualizer success. Run it with:

```sh
DITCH_TEST_CODEX_BINARY=/absolute/path/to/codex \
  cargo test -p ditchd --lib oss_tool_policy -- --include-ignored --nocapture
```

The current repair has **not** been deployed or visually qualified in signed staging.
The implementation session's filesystem restrictions blocked Xcode package-cache
access before a new signed bundle could be produced. Rebuild/run signed staging in
the normal developer terminal, then perform the checks below on both a new OSS
conversation and a follow-up in an existing one. Confirm native receipts and visible
target feedback with Visualizer on; repeat with it off and verify actions still work.

The follow-up adapter-ownership regression uses real Codex and the production CLI
with the **production Rust authorization function**, on new and disk-resumed
threads. It verifies that broad discovery can use a second helper without moving
execution authority, and that secondary tool calls remain rejected. The transport
test also sends a tool call through the real thread after broad discovery and
confirms that it still reaches the original execution process. The transport
fixture has no native provider and bypasses native signature verification; signed
host/OS permission and real desktop-action acceptance remain separate checks.

```sh
cargo build -p ditch_cli
DITCH_TEST_CODEX_BINARY=/absolute/path/to/codex \
DITCH_TEST_DESKTOP_CLI="$PWD/target/debug/ditch_cli" \
  cargo test -p ditchd --lib live_codex_discovery_preserves_production_execution_owner \
  -- --ignored --nocapture
```

The companion Python probe reproduces the old strict discovery rejection, then
verifies the corrected contract, including real secondary-CLI tool-call rejection
and revocation. It makes no desktop calls or external inference requests:

```sh
python3 scripts/check-peekaboo-codex.py --adapter-startup \
  --server /absolute/path/to/ditch_cli --codex /absolute/path/to/codex \
  --build-root target/adapter-probe
```

In a Commercial checkout, prefix the script path with `community/`. Use a short
build-root path because macOS bounds the length of Unix socket paths. The probe
writes only disposable homes and diagnostic evidence below that directory and
removes its per-run socket credentials when it finishes.

Physical keyboard/mouse input from the signed Ditch host has **not** yet been
qualified with macOS permissions. Before release, use a disposable text document:

1. Start the built Ditch app, select the custom model and Full Access, and complete
   OS permission setup for the displayed runtime host.
2. Ask the agent to list windows, observe the disposable document, type a short
   marker and re-observe it. Confirm actual tool activity, not the model's claimed
   identity in prose.
3. Stop during a long input sequence. Confirm no later input and no automatic
   retry. Repeat while changing away from Full Access.
4. Test a stale snapshot after moving/closing the window; another Ditch session
   contending for the desktop; screen lock/sleep; permission revocation; and
   restarting after interrupted input. A new observation/turn must be required.
5. Qualify Retina/external displays, Intel, macOS 15, and older-OS launch without
   loading the optional module. Test capture contention with standalone Peekaboo.
6. Validate Developer ID release signatures, notarization, clean installation and
   permission continuity across upgrades. Debug builds are not release sign-off.

Do not run these checks against a document or application where unintended input
would have consequential effects.

## Fresh/resumed inference contract and Arc verification

The opt-in Rust regression test
`actual_inference_request_preserves_current_policy_on_fresh_and_resumed_threads`
exports the production instructions, schemas and diagnostic restrictions, then
runs `scripts/check-oss-turn-contract.py` against installed Codex and a loopback
Responses fixture. It checks the actual outgoing developer messages and tool
catalogue for a fresh thread, a resumed current thread, and a resumed thread with
obsolete CUA references. Resume must retain the original thread and recall marker.
The fixture never performs desktop actions or external inference.

```sh
DITCH_TEST_CODEX_BINARY=/absolute/path/to/codex \
  cargo test -p ditchd actual_inference_request_preserves_current_policy \
  --lib -- --ignored --nocapture
```

The printed evidence directory contains `policy.json`. To compare an actual OSS
model, pass that file to the same Python script with `--codex`, `--build-root`,
`--endpoint` and `--model`. This is opt-in inference against the specified custom
endpoint. The test relay rejects executable/delegation tools before forwarding,
does not follow redirects, and uses a disposable Codex home without OpenAI
credentials. The MCP server is still a fixture, not a native desktop test. An
optional `DITCH_OLLAMA_SMOKE_TOKEN` stays in the relay, not Codex's environment.

`TurnOptions.desktop_verification_target` is an optional diagnostic restriction
(`Arc` or `DitchStaging`), not an additional permission or a UI default. It requires
an eligible local Full Access OSS turn. It disables shell/delegation/image-file
tools and restricts native discovery, observation and action to that exact bundle
ID. Normal turns omit it. Native ABI 3 is required so a stale module cannot silently
ignore the target restriction. This is not complete host credential isolation;
the target application's own capabilities remain relevant.

After rebuilding and launching signed Staging, the explicit native probe is:

```sh
python3 scripts/check-signed-desktop.py \
  --support "$HOME/Library/Application Support/The Ditch Staging" \
  --project-id EXISTING_LOCAL_PROJECT_UUID \
  --endpoint-id SELECTED_OLLAMA_ENDPOINT_UUID \
  --model qwen3.5:9b --report target/arc-verification.json
```

In a Commercial checkout, prefix script paths with `community/`. The probe refuses
production and builds lacking `desktop_verification_v1` or `desktop_automation_v2`. It creates a diagnostic
agent, opens one new Arc tab at example.com with a unique URL marker, and enables
the existing visualizer. It does not create Kanban tasks or invoke GitHub. It
requires native action receipts and a subsequent fresh observation containing
that marker; model claims alone cannot pass. A timeout requests cancellation.
The report contains IDs, states and verification booleans, not browser contents.
It requires visual observations and foreground action receipts. Actual overlay appearance and pointer travel still need visual verification on the signed application.

Qualify ordinary Arc operation before running repeated browser/Kanban workflows.
Record native failures separately from model mistakes, false completion claims,
latency and retries. Do not generalize a fixture pass to autonomous desktop
reliability or delete an old conversation to conceal a resume failure.

### Validation recorded on 2026-11-16

The installed Codex 0.166.0 passed the loopback request-boundary checks for fresh,
resumed current, and resumed legacy-CUA threads. Current developer instructions,
the four desktop schemas, and retained history reached the inference request;
diagnostic shell/delegation tools were absent.

Live Qwen3.5:9b checks against the selected Ollama endpoint selected the correct
desktop tool in all three cases. One run passed the recall checks; a repeat
inserted an extra character in the synthetic recall marker on both resumed turns.
The original marker was present in each outgoing request. These are recorded
model transcription failures, not evidence of dropped history, and the test
continues to fail them. Fixture success does not establish native Arc reliability.

Targeted Rust checks passed for both editions. A direct build of the current
Swift module and target-policy tests passed (four tests, with the GUI-host-only
self-PID test skipped). The signed Staging rebuild failed during Xcode package
resolution/result-bundle writes because of filesystem permissions. The running
older build was correctly refused by the native probe before creating an agent.
Actual signed-host Arc input and visualizer visibility remain unverified until
Staging is rebuilt and the native probe completes.

### Visual input implementation and validation — 2026-11-17

The native visual route, turn-scoped target registry, cursor feedback, separate
Accessibility-only observation, runtime recovery limits, and v3 developer
instructions are implemented in Community and shared by Commercial. No vendor
source or inference provider configuration was changed by this increment.

The native module compiles against the pinned Peekaboo dependencies. Native
regressions cover coordinate scaling, same-host occlusion, turn ownership,
Accessibility snapshot publication without images, cancellation and overlay
input isolation. Community and Commercial runtime tests cover scope, receipts,
revocation and bounded recovery.

The installed Codex 0.166.0 passed the actual inference-request checks on fresh
and resumed threads with the new schemas. A live Qwen3.5:9b image fixture at the
selected Ollama endpoint passed: the exact MCP PNG reached the inference request,
and Qwen identified the randomly placed red half correctly. This is image
transport/perception evidence, not proof of successful desktop automation.

Signed Staging build was attempted using the existing Developer ID identity.
Xcode package resolution failed on sandbox-denied cache/result writes outside the
workspace. No new signed app was launched, so physical clicks, visible cursor
travel, Ditch task creation and clean-install behavior remain unqualified.
Rebuild and launch signed Staging, confirm desktop_automation_v2, then run the Arc
probe above and a Ditch Staging workflow before treating this as release-ready.
Status/menu-bar integration and absolute host-account inference isolation remain
separate unresolved work; this increment does not claim to implement either.

### Discovery repair and validation — 2026-11-17

The morning signed Staging run exposed a native registration bug: a missing
optional LaunchServices timestamp caused `applicationGone`, and one registration
failure aborted the complete list. Discovery failures also bypassed the failure
counter. The general repair above applies to all applications; Staging is the
regression scenario. OSS instruction policy v4 prioritizes current tool errors
over stale conversation summaries and prohibits substituting production or shell
launches for failed target discovery. These instructions are not a guarantee of
model compliance.

Validation of this repair:

- Native module compiled against the existing pinned dependency objects; the
  direct XCTest runner executed 28 tests, 27 passed and one GUI-host test skipped.
  New regressions cover kernel identity, missing identity, PID reuse, bundle and
  session mismatches, partial/empty discovery, and malformed argument diagnostics.
- Community and Commercial: nine tool-integration tests passed in each edition;
  one opt-in live transport test was not run in each suite. Five OSS-policy tests
  passed in Commercial. The real installed Codex request-boundary test also passed
  for fresh, resumed-current and resumed-legacy threads with policy v4. That test
  uses a loopback fixture, not live Qwen inference or desktop input.
- Four offline signed-probe regressions passed. They reject nonexistent,
  duplicate, wrong-project, non-Backlog and GitHub-linked tasks.
- Full SwiftPM testing was blocked writing its existing build directory. The
  signed Staging build was attempted separately and failed during Xcode package
  resolution with filesystem permission errors. No repaired signed application
  was launched. Real cursor visibility and Qwen task creation remain unverified.

The signed probe now supports `--target DitchStaging`. This explicitly creates
one uniquely titled diagnostic Backlog task through the UI in the supplied
project. It requires native foreground click receipts, a later visual observation,
and a read-only database check for exactly one task in that project, Backlog state,
and no GitHub source. It leaves the task in place for inspection. No task API or
database write is used by the probe to create it. Visual cursor appearance still
requires observation by the tester; a dispatch receipt cannot prove it was visible.

After rebuilding and launching signed Staging, from the Commercial root:

```sh
python3 community/scripts/check-signed-desktop.py \
  --support "$HOME/Library/Application Support/The Ditch Staging" \
  --project-id EXISTING_LOCAL_PROJECT_UUID \
  --endpoint-id SELECTED_OLLAMA_ENDPOINT_UUID \
  --model qwen3.5:9b --target DitchStaging \
  --expected-build "$(./scripts/build-identifier)" \
  --timeout 300 --report target/staging-desktop-verification.json
```

`--expected-build` rejects a mismatched Rust runtime build before starting an
agent; it does not independently attest native module contents. Use the normal
complete signed-app build, not a manually replaced dylib. In Community, omit the
`community/` script prefix. To exercise an existing failing conversation, add
`--resume-thread THREAD_UUID` and use a separate report path. Resume requires an
inactive thread with the same project, endpoint and model. Verification reads
only new transcript bytes and operation receipts and requires the same thread ID;
old successful actions cannot satisfy the new run. Each probe run creates its own
uniquely named task if it succeeds. OpenAI inference is never requested by this
probe; provider isolation beyond the existing diagnostic restrictions is separate.
