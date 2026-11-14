# Ditch desktop MCP feasibility fixture

This is a Phase 1 test server, **not the production Ditch tool adapter**. It has
no desktop APIs and is never linked into or packaged with Ditch. The SDK is
pinned to rmcp 3.10.0; Cargo.lock records the fixture's complete dependency set.

From the Community repository:

```sh
cargo build --locked --manifest-path scripts/fixtures/desktop-mcp-probe/Cargo.toml --target-dir target/peekaboo-prototype/rust-build
python3 -B scripts/check-peekaboo-codex.py --server target/peekaboo-prototype/rust-build/debug/ditch-desktop-mcp-probe --build-root target/peekaboo-prototype
```

The default check verifies SDK initialization, tool discovery/invocation over
stdio, and the installed Codex app-server's discovery of a server injected in
thread/start configuration. It makes no inference request. It uses an isolated
CODEX_HOME and does not read or change installed Ditch settings or user Codex
configuration. Reports explicitly mark untested resume/model/vision checks as
false. A discovery-only pass is not a complete Phase 1 pass.

To test an accessible Ollama endpoint, add these arguments to the Python command:

```text
--live --endpoint https://your-ollama-server --model qwen3.5:9b --vision
```

When needed, supply its existing credential through DITCH_OLLAMA_SMOKE_TOKEN,
not command arguments. Live mode creates a conversation without the adapter,
restarts Codex, resumes that same thread with the adapter, checks conversation
recall, and requires an actual MCP call. Vision mode supplies a randomly ordered
four-color PNG through MCP image content, with no accessibility text describing
the colors. Shell/file tool use invalidates the test. The harness currently uses
the previously tested 65,536-token serving budget; run live mode only against a
model configured for that budget.

All temporary homes, logs, and reports are placed below the specified build root.
No project output/ folder or real screen captures are created. This fixture does
not establish macOS permission attribution, native input cancellation, Full Access
gating in Ditch, or packaging compatibility. Those remain separate native gates.

## November 13, 2026 local evidence

- Built successfully with Rust 1.97.1 and rmcp 3.10.0.
- SDK stdio initialization/list/call passed.
- Codex 0.166.0 discovered desktop_status from thread/start configuration.
- After full workspace access, the native package and staging app build passed.
- Codex 0.166.0 resumed an existing thread, recalled prior context, called the fixture with Qwen3.5:9b, and interpreted the synthetic image through the onprem-mtn2 endpoint.
- The signed status-only native prototype reported no macOS permissions. Physical input was not tested or authorized through this fixture.

The fixture remains non-shipping. Production integration is described in `docs/desktop-tools.md`. Native permission/input and clean-machine release qualification remain separate gates.
