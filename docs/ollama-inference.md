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

The `ollama_inference_v1` capability gates endpoint RPCs and OSS execution.
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

OpenAI-hosted web search is disabled for OSS turns. Existing sandbox, skill,
permission and approval handling remains in the shared execution path. ChatGPT
login remains required by the OpenAI setup path; OSS setup only needs the binary
and compatible runtime. Remote setup exposes a separate `oss_execution` check.

OSS session titles use a deterministic prompt-derived title. Ditch does not
submit an OpenAI naming request. Skill discovery does not submit inference.
Automatic board planning for an OSS-selected operation is explicitly unsupported
in this iteration; enter the plan manually. Execution and acceptance retries
retain the selected inference profile.

## Verification and release gate

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
It declines elevated approvals; interactive approval grants require a separate
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
