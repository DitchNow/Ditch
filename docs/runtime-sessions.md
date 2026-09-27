# Local and SSH Codex sessions

The desktop uses the same app-server adapter for both execution targets:

- Local: Desktop → Mac runtime → Codex app-server.
- SSH: Desktop → Mac runtime → SSH → remote runtime → Codex app-server.

Mobile continues to reach SSH projects through the Mac. Commercial Mobile follow-ups preserve the conversation’s applied model, reasoning effort, and approval policy. This change introduces no remote enrollment, relay, or key ownership changes.

## Controls

| Selection | Codex approval policy | Sandbox |
| --- | --- | --- |
| Ask for approval | `on-request` | Workspace write, network enabled |
| Approve for me | `never` | Workspace write, network enabled |
| Full Access | `never` | Unrestricted |

Approve for me does not grant unrestricted filesystem access. A command forbidden by the workspace sandbox can fail; it must not silently switch to Full Access. User questions remain answerable in every mode.

Profiles belong to a conversation. The new-agent composer has a separate draft per project. The model list comes from the project's runtime, and an unavailable catalog does not replace a saved model. Model and policy changes apply to the next turn; an already waiting request must be answered or the turn stopped first. The runtime records the effective model returned by Codex.

Approve once, session approval, and Cancel are protocol responses, not chat prompts. Session approval forwards Codex's native `acceptForSession` or permission `scope: session`, retaining Codex's grant scope; it is not blanket approval of unrelated actions. The adapter currently runs one app-server process per turn, so native in-memory grants are not guaranteed across subsequent resumed turns. Choose Approve for me for a persistent no-approval policy on the conversation.

## Confirmation and recovery

An accepted Ditch command means its response was sent to Codex. The request remains visible with `response_pending: true` until `serverRequest/resolved`, completion of its corresponding item, or turn termination. Repeated identical responses while pending do not write twice; conflicting responses are rejected. A response still unconfirmed after 30 seconds fails and closes that turn instead of leaving an indefinite approval wait.

Snapshots and rejoin include pending responses. Missing requests from an offline SSH host are not treated as confirmed resolutions; a recovered live request can replace a stale UI resolution. The inline card can refresh an uncertain outcome: a resolved request stays closed; an unsent request becomes actionable; a sent response stays disabled until confirmation. A second approval has its own ID and remains actionable even when it immediately follows the first one.

Stop first sends `turn/interrupt` with the native thread and turn IDs. If that does not finalize within the grace period, existing process-group termination handles cleanup. Stop, cancellation, failure, and completion dismiss pending permission attention. Later prompts resume the saved thread with the selected profile. An immediate follow-up waits briefly for the completed process to release its thread writer; late events from older runs cannot clear the new run’s controls.

SSH loss does not intentionally stop the remote daemon or its agent. Existing operation receipts, event replay, and paginated rejoin continue to handle bridge recovery. A daemon restart cannot preserve its in-memory app-server connection: active runs become stale, their old approvals are dismissed, and the saved thread can be resumed explicitly.

## Updating

Build and ship the desktop and its runtime together. Rebuild the bundled remote runtime artifacts from the same source. New remote turns require both `remote_runtime_protocol_v4` and `app_server_sessions_v2`; use Remote Runtime Setup to update an older host. Existing approval and Stop controls continue to work against protocol 4 hosts while preparing that update. No lingering/service change is required by this patch.

Existing running local CLI turns are not converted in place. They can finish or be stopped through their existing process handle; the next prompt uses app-server. Do not replace a running remote daemon just to deploy this fix while it still has work that must be preserved.

## Validation

Run both editions' runtime tests and the shared desktop tests. `local_and_remote_app_server_approval_rejoin_model_and_stop_lifecycle` covers both runtime modes, immediate consecutive approvals, pending-response rejoin, duplicate responses, model/policy changes on the same thread, Stop and Cancel. The existing SSH receipt/replay tests cover transport recovery.

The opt-in live test uses a disposable project and separate Codex home. It copies the existing authentication file only on its own machine, then deletes the copy and temporary project. It makes real model requests. Defaults require access to `gpt-6-astra` and `gpt-5.6-sol`.

```sh
# From the Community checkout:
python3 scripts/test-app-server-live.py
python3 scripts/test-app-server-live.py --ssh YOUR_SSH_ALIAS
```

The live test drives the actual shared adapter: automatic execution without a permission request, Ask/approve/confirmation, same-thread model change, and interruption. Its SSH wrapper tests app-server across a real SSH connection; it does not install a new remote ditchd or replace the running desktop. A packaged release still needs the normal desktop → Mac daemon → SSH daemon acceptance check, including disconnecting and reconnecting during a pending approval.
