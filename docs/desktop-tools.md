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
and `desktop_act`. Actions support one click, text entry, key combination, scroll,
or window focus against a fresh exact-window snapshot. Other Peekaboo features
are outside this initial tool surface.

## OSS tool routing

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

Native feedback briefly outlines an observed target, pulses a click location, or
shows a fixed typing/shortcut/scroll icon. It never displays typed text and never
means an action was verified successful. Fresh observations still verify results.
The current tools use background delivery: there is no travelling physical cursor.
Hidden, moved, minimized, or partly covered windows receive no overlay.

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
Snapshots are single-use, expire after 30 seconds, and are discarded on revocation.
Window/process generation and outcome validation reuse Peekaboo's exact-target
APIs. Partial or uncertain dispatch returns `outcome_unknown`; it is not replayed.

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
