# Ollama inference through Codex

Ditch's existing Codex execution provider can use registered Ollama endpoints.
The project host runs Codex, file tools and shell commands. The selected Ollama
host supplies inference only. Community owns this implementation; Commercial
imports the same runtime and Flutter components and mirrors the protocol wrapper.

## UI and persistence

The existing composer and new-agent dialog share one model dropdown, grouped
into OpenAI and OSS sections. An OSS option is identified by endpoint UUID plus
exact model ID, so equal model names on two servers remain distinct. The Add OSS
model dialog accepts a name, root URL, optional token and discovered or manual
model ID. OpenAI and OSS catalogs load independently. Endpoint lists are cached;
loading the dropdown does not contact the GPU hosts.

Agent profiles persist `inference_endpoint_id` alongside `model`. Its absence
preserves the existing default/OpenAI behavior. Existing sessions cannot change
endpoint; a different inference server requires a new agent. Endpoint URLs are
immutable: registration always creates a new ID. Disabling an endpoint stops
future turns using that endpoint without redirecting saved sessions.

## Host ownership and credentials

`Ollama` requests contain a project ID and a typed operation: `List`, `Test`,
`Add`, `Refresh`, or `Disable`. They route through the existing project SSH
connection. The execution owner keeps endpoint metadata in its canonical SQLite
database (`ollama_endpoints`, Community migration 7). Credentials are separate:
local macOS uses Keychain, and headless execution uses files with mode 0600 in a
0700 directory. Endpoint responses expose only credential presence and auth mode.

The `ollama_inference_v2` capability gates endpoint RPCs and OSS execution with
verified context handling. The runtime also advertises v1 for older clients.
Older runtimes receive a status query before any unsupported operation. Add,
refresh and disable requests use the existing operation-receipt mechanism.

HTTP is accepted only when explicitly enabled for private/loopback destinations.
Public endpoints require HTTPS. URL credentials, queries, fragments, unexpected
paths, metadata addresses and link-local addresses are rejected. Discovery pins
validated DNS addresses and does not follow redirects. DNS and HTTP operations
are bounded; error responses never include the gateway's response body or token.

## Codex launches

The default `Client::open` path still launches plain `codex app-server`. OSS uses
`open_with_provider` to supply a stable `ditch_ollama_<uuid>` provider through
child-process configuration and an optional credential through a child-only
environment variable. No user Codex config is rewritten. New threads specify
`modelProvider`; resumes reload the same provider and validate the saved endpoint.
Returned provider fields are checked before sending the model turn.

OpenAI-hosted web search, Apps and inherited MCP servers are disabled for OSS
turns. Automatic skills are limited to the project and explicitly selected
skills, with a 1,024-token catalog budget. Tool results have a 4,096-token budget.
Existing sandbox, permission and approval handling remains in the shared
execution path. ChatGPT
login remains required by the OpenAI setup path; OSS setup only needs the binary
and compatible runtime. Remote setup exposes a separate `oss_execution` check.

New OSS sessions initially show a temporary prompt-derived title. After the user
turn finishes, the execution owner generates a concise title using the session's
exact OSS model and endpoint. This runs in a separate ephemeral Codex thread,
with tools and skills disabled, and changes only the original thread's name.
It does not append messages to the conversation or create another Ditch agent.
There is no OpenAI fallback or OpenAI catalog request for OSS naming.

Naming uses the same endpoint verification and context configuration as normal
OSS turns. Generation has a 45-second response deadline; setup and verification
have their own bounded deadlines. Two attempts are allowed, with a 60-second
retry delay and durable attempt/result storage. A failed metadata save reuses the
generated title. On failure, the temporary title remains; manual names always
take priority. Existing titles are retained because their origin was not tracked.
To explicitly generate or retry a title, click the session name, clear the field,
and save. This also works for older OSS sessions. A new generation request or
model/endpoint change invalidates an older job's result. Remote SSH projects name
sessions on the execution owner, using that owner's endpoint credentials.

Skill discovery does not submit inference.
Automatic board planning for an OSS-selected operation is explicitly unsupported
in this iteration; enter the plan manually. Execution and acceptance retries
retain the selected inference profile.

## Conversation context and endpoint compatibility

Every launch and resume verifies the selected model before sending the user's
turn. `/api/show` must expose an explicit model `num_ctx` (8,192–262,144), and
`/api/ps` must confirm that same allocation after a short Responses request.
Training-context metadata is never used as a serving budget. A missing budget,
different allocation, failed developer-instruction/recall check, or silent
overflow fails the turn visibly without replacing the saved thread.

Successful checks are stored in the endpoint JSON's optional `model_status` map,
keyed by exact model ID. Old endpoint records deserialize with an empty map.
The overflow check is cached for at most 24 hours and invalidated by changes to
the model description/configuration or server version. The short instruction,
recall and allocation checks run on every launch/resume. No credentials or
conversation content are included in this metadata. Checks have a shared
75-second deadline, within the UI's 100-second RPC timeout.

Codex receives `model_context_window=num_ctx` and
`model_auto_compact_token_limit=num_ctx*3/4` with `total` accounting as child-only
overrides. For 65,536
tokens, compaction is configured at 49,152. Codex can report a slightly smaller
usable window after its own reserve (62,259 in CLI 0.166.0). Both initial and
resumed turns use the same configuration and original CODEX_HOME.

For Qwen3.5 9B, 65,536 was validated on a 24 GiB RTX 3090 Ti. Configure the model
using Ollama's model configuration, not a transient chat `options.num_ctx`:

```text
FROM qwen3.5:9b
PARAMETER num_ctx 65536
```

Apply this Modelfile with `ollama create qwen3.5:9b -f /path/to/Modelfile` on the
inference host. Preserve existing parameters and choose an allocation suitable
for each model; do not apply this size blindly to all models on the host.

Ollama 0.40.16 fixes Responses `truncation: disabled`, but its Qwen3.5 renderer
still drops developer messages. `scripts/ollama-qwen35-developer.patch` applies
to source tag `v0.40.16` (commit `d63b702609c4300a2ca07be2bdb0adb9834725e5`).
It reuses the existing instruction normalization for Qwen3.5 and adds tests with
and without tools. To build an audited replacement in an Ollama source checkout:

```sh
git apply /path/to/Ditch/community/scripts/ollama-qwen35-developer.patch
go test ./model/renderers ./openai
go build -ldflags '-X github.com/ollama/ollama/version.Version=0.40.16-ditch1' -o ollama .
```

Use the source's required Go version and retain compatible runner libraries.
Keep a backup of the installed executable before replacing it and restarting
the service. This patch must be rechecked on future Ollama upgrades; a plain
upstream upgrade must not be assumed to preserve the developer-role fix.

The live Ditch regression exercises the actual Rust launch/resume path and checks
conversation-only marker recall after a new child and explicit compaction:

```sh
DITCH_OLLAMA_TEST_URL=https://your-gpu-host.example \
DITCH_OLLAMA_TEST_MODEL=qwen3.5:9b \
cargo test -p ditchd --lib live_ollama_recall_resume_and_compaction -- --ignored --nocapture
```

Optional `DITCH_OLLAMA_TEST_ROLLOUT` tests recovery from a **copy** of a saved OSS
rollout. It never edits the original transcript. Use only the original inference
endpoint when replaying real conversation content. Temporary test homes use the
test process's temporary directory; no project `output/` directory is required.

## Verification and release gate

OSS naming validation on November 12, 2026:

- The live Qwen3.5 9B regression generated “Fix login validation to reject empty
  passwords” through the configured HTTPS Ollama endpoint, without OpenAI auth.
  The original thread's turns were unchanged, its name was saved, and no extra
  persistent thread or Ditch agent was created.
- Fixture tests cover provider/model binding, execution-owner routing, waiting
  for turn completion, retries, cached results after save failure, manual edits,
  explicit regeneration of legacy titles, and backward-compatible title data.
- Community core/store/daemon library tests: 278 passed, 10 ignored. The final
  naming suite passed in both editions: 15 passed, 2 opt-in live tests ignored
  (the OSS live test was run separately and passed).
- Eighteen affected Flutter tests passed; analysis of the changed UI files
  reported no issues.

Run the opt-in naming regression with a disposable Codex home:

```sh
DITCH_OLLAMA_TEST_URL=https://your-gpu-host.example \
DITCH_OLLAMA_TEST_MODEL=qwen3.5:9b \
cargo test -p ditchd --lib live_oss_session_naming_preserves_main_conversation -- --ignored --nocapture
```

The test creates and removes its own temporary home and sends two synthetic
turns, in addition to compatibility probes. Rebuild/relaunch the app and update
remote execution runtimes to activate naming; this does not require another
Ollama server change.

Context-fix validation on November 11, 2026:

- Built `0.40.16-ditch1`, passed Ollama renderer/Responses tests, and installed
  it on the inference host after operator sudo approval. The original server
  executable is preserved as `/usr/local/bin/ollama.pre-ditch-context-fix`.
- A 49,263-token Responses request recalled its marker at 65,536 context;
  oversized requests with disabled truncation now fail explicitly.
- The real Ditch Rust path passed recall after restart and explicit compaction
  through the normal HTTPS endpoint. A copied affected rollout also passed.
  A fresh test used about 6.8K input tokens; the copied old thread used about
  19.6K before compaction. Historical incorrect answers are not rewritten.
- Community daemon library: 245 passed, 9 ignored. Commercial daemon library:
  317 passed, 10 ignored, the previously documented
  `oldest_eligible_plan_claims_before_newer_work` failure remains.
- Twelve affected Flutter tests passed; analysis of the changed UI files found
  no issues. Focused core/store/Ollama tests passed after the final changes.
- The fixture-coding harness reached actual tool calls, but the enclosing macOS
  environment denied nested `sandbox-exec`. It deliberately declined escalation;
  end-to-end file editing must be rerun from a normal development terminal.
- A native Staging debug build was attempted, but Xcode package resolution failed
  with `permissionDenied` in this environment. No new app bundle was deployed.
- Recall was tested across **explicit** compaction. A prolonged workload that
  naturally reaches the automatic threshold remains a separate soak test.

The server is updated independently of the application. Rebuild/relaunch Ditch
and update any remote execution runtimes to activate the v2 launch safeguards.
Do not count a server-only upgrade as an application deployment.

Working-tree validation on November 11, 2026:

- Community core/store/daemon library tests: 268 passed, 8 ignored. The final
  persistence/reopen and OSS daemon checks also passed after the last changes.
- Commercial library tests: 335 passed, 9 ignored, one existing scheduler failure
  (`oldest_eligible_plan_claims_before_newer_work`). The same failure reproduced
  in an untouched checkout of the original revision. All seven final Commercial
  OSS daemon tests passed.
- Affected Community Flutter tests and all 91 Commercial Flutter tests passed;
  analysis of the changed Dart files reported no issues.
- Native macOS packaging remains unverified: Xcode package resolution failed
  with sandbox permission errors. Run that build in an unrestricted development
  environment before release.

The installed Codex CLI 0.166.0 accepted the custom provider in a fresh Codex home
without OpenAI credentials and returned the expected provider ID. This was a
configuration-only check, not a successful Ollama coding turn.

Run the isolated real-endpoint harness from the actual project execution host:

```sh
python3 scripts/check-ollama-codex.py \
  --endpoint https://your-gpu-host.example \
  --model your-exact-model-id \
  --output /path/to/disposable-evidence
```

For gateway authentication, supply `DITCH_OLLAMA_SMOKE_TOKEN` through the
environment. Do not put it on the command line. `--config-only` verifies thread
startup without inference. The full harness requests a fixture correction and
test run, checks the result independently, then resumes using a new child.
It also checks a conversation-only marker after restart and compaction; reading
the fixture cannot satisfy that check. `--context-window` must match the serving
configuration and defaults to 65,536. It declines elevated approvals;
interactive approval grants require a separate
Ditch UI test.

Before release, verify real coding, approval grants, stop/reconnect/restart and
resume on local and SSH projects, with two distinct GPU endpoints advertising
the same model. Confirm inference in the GPU-server logs. A model-list response
or the dialog's bounded Responses/tool smoke test does not establish reliable
autonomous coding. Verify Keychain behavior in a packaged macOS app and remote
credential handling on the chosen execution host.

Commercial must pin the committed Community revision before release packaging.
The working-tree integration alone does not create that immutable revision.

Reference: [OpenAI's provider-scoped app-server configuration](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server).
