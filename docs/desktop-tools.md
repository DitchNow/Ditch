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

The staging debug bundle builds. Native cancellation/ownership tests, runtime
policy/dedup tests, focused Flutter regressions, Codex session/approval regressions
and nested signing tests pass. The isolated live Qwen3.5:9b probe passed tool
discovery, thread resumption, recall and synthetic image interpretation.

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
