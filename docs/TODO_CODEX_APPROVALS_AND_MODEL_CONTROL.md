# TODO: Codex approvals, native macOS notifications, and model control

## Outcome

Ditch must provide a complete Codex approval experience equivalent in capability to the Codex VS Code extension:

1. The composer lets the user select a Codex model and approval preset.
2. Selecting `Ask for approval` makes Codex pause when it needs approval.
3. Ditch receives each live approval request and presents it in the in-app attention surface.
4. macOS also displays a native notification in the usual top-right notification area.
5. The native notification identifies the project, agent, and requested command or action and offers `Approve` and `Deny` actions.
6. Selecting an action from the app or notification sends the matching live decision back to Codex.
7. The relevant Codex turn resumes or is declined, and Ditch records the result.

The product language must be provider-neutral so a future Claude Code adapter can expose the same product-level controls when its native capabilities permit them.

## Current state and why `Ask for approval` is unavailable

The current runtime launches every Codex request through non-interactive `codex exec` processes. That transport supports launch flags such as `--model` and `--approve-for-me`, but it does not provide Ditch with a bidirectional live protocol for receiving and resolving user approval requests.

The codebase already contains partial permission protocol types:

- `PermissionRequest`
- `ServerEvent::PermissionRequested`
- `ClientRequest::ApprovePermission`
- `ClientRequest::DenyPermission`

However, the runtime currently returns `unsupported_request` for the approve and deny requests. The disabled composer menu item prevents implying that a user approval can currently be delivered back to Codex.

## Required architecture: Codex App Server

Replace Codex's process-per-prompt `codex exec` integration with a long-lived `codex app-server --stdio` child owned by `ditchd`.

App Server is Codex's JSON-RPC integration protocol for rich clients. It provides:

- authenticated thread creation and resumption;
- model discovery through `model/list`;
- streamed agent, tool, command, and file-change events;
- per-turn model, effort, approval, reviewer, and sandbox overrides;
- server-initiated command, file-change, permission, MCP, and tool approval requests;
- client decisions that resume or decline the exact pending request.

Use one managed App Server connection per Ditch runtime and multiplex its Codex threads. Do not spawn one short-lived App Server process per prompt: a long-lived connection is required to receive and answer server-initiated approval requests reliably.

### App Server initialization

On runtime startup or first Codex use:

1. Locate the Codex binary as today.
2. Launch `codex app-server --stdio` with piped stdin/stdout/stderr.
3. Send JSON-RPC `initialize` with client metadata identifying Ditch.
4. Send `initialized`.
5. Start a dedicated stdout reader that parses every JSON-RPC response, notification, and server-initiated request.
6. Serialize outbound JSON-RPC writes behind a mutex/queue and allocate monotonically increasing request IDs.
7. Treat a broken pipe or exited App Server as a runtime incident: mark active Codex turns interrupted/stale, clear actionable approvals, raise attention, and restart lazily on the next use.

## Provider-neutral domain model

Do not model this as Codex-only state. Add provider-neutral concepts in `ditch_core`.

```text
AgentExecutionProfile
  model: Option<String>
  reasoning_effort: Option<String>
  approval_preset: Ask | ApproveForMe | FullAccess
  approval_policy: effective provider-native policy
  approvals_reviewer: User | AutoReview
  sandbox_policy: ReadOnly | WorkspaceWrite | DangerFullAccess
  network_access: bool

AgentCapabilitySet
  models
  supported_reasoning_efforts
  supports_interactive_approvals
  supports_auto_review
  supports_full_access
  supports_per_turn_model_change
  approval_decision_kinds

AgentTurn
  ditch_turn_id
  agent_id
  provider_thread_id
  provider_turn_id
  user_prompt
  effective_execution_profile
  status
  started_at
  finished_at
  error

PendingApproval
  ditch_request_id
  provider_request_id
  provider_thread_id
  provider_turn_id
  provider_item_id
  agent_id
  project_id
  kind
  summary
  command
  cwd
  target
  network_context
  available_decisions
  created_at
```

Store the requested preset and the fully resolved effective profile. A user can select one preset while organization policy or provider capability resolves it to more constrained native settings.

## Codex preset mapping

The initial product presets map to App Server settings as follows:

| Ditch selection | `approvalPolicy` | `approvalsReviewer` | `sandboxPolicy` |
| --- | --- | --- | --- |
| Ask for approval | `on-request` | `user` | `workspaceWrite` |
| Approve for me | `on-request` | `auto_review` | `workspaceWrite` |
| Full Access | `never` | not applicable | `dangerFullAccess` |

`Full Access` must require explicit confirmation before changing the next-turn/session profile. The runtime must independently validate that the selected policy is allowed; Flutter input is not an authorization boundary.

Before rendering these controls, query `configRequirements/read` and disable unavailable modes according to Codex managed policy. Never show a selectable mode that the current Codex installation, account, or managed configuration rejects.

## Thread and turn lifecycle

### New agent

1. Use `model/list` to populate the picker from the active account's actual model catalog.
2. On first submit, call `thread/start` with cwd, selected model, initial approval settings, and client service name.
3. Persist the returned `thread.id` on the agent.
4. Call `turn/start` with the user's prompt and the selected execution profile.
5. Persist the returned turn ID and the effective profile before accepting streamed output.

### Follow-up

1. Ensure the saved thread is resumed with `thread/resume` after runtime reconnection.
2. Call `turn/start` with the follow-up prompt and any selected profile override.
3. Record the turn's actual model, effort, approval configuration, and result.

Model or approval changes during an active turn apply only to the next turn. The UI must say `Applies next turn` and must not attempt to alter an active turn or use `turn/steer` for this purpose.

## Approval event routing

Handle every App Server approval surface explicitly.

| App Server request | Ditch approval kind | Required decision choices |
| --- | --- | --- |
| `item/commandExecution/requestApproval` | Command | Approve once, approve for session when offered, deny, cancel; optionally exec-policy amendment when offered |
| `item/fileChange/requestApproval` | File change | Approve once, approve for session when offered, deny, cancel |
| `item/permissions/requestApproval` | Additional permission | Grant the requested subset with turn/session scope, or deny |
| `mcpServer/elicitation/request` | MCP elicitation | Accept, decline, or cancel; collect required form/url result when applicable |
| `tool/requestUserInput` | Tool input/approval | Render the provider-supplied choices and resolve the request |

Do not collapse these into a boolean `ApprovePermission` API. Ditch protocol must preserve the server's `availableDecisions` and all IDs necessary to resolve the exact pending request.

### Protocol changes

Replace the old binary request API with a structured resolution request:

```text
ResolveApproval {
  request_id
  decision
  scope: once | session
  granted_permissions: optional subset
  execpolicy_amendment: optional argv rule
  user_input: optional structured value
}
```

`PermissionRequested` must contain enough display data for both in-app UI and native notifications without requiring Flutter to understand Codex-specific JSON.

## macOS notification behavior

### Delivery

Use `UNUserNotificationCenter` in the macOS Runner/AppDelegate layer.

Request notification authorization once, lazily, when the first approval request is received. If authorization is denied, keep the in-app attention card fully functional and show a one-time settings explanation.

Post a local notification for every newly pending approval with:

```text
Title: Approval needed — <agent name>
Subtitle: <project name>
Body: <command or concise action summary>
```

Keep the body compact and redact secrets. If a command is long, display a safe truncated preview in the notification and show the complete command only in the Ditch UI after explicit user navigation.

### Notification actions

Register category actions:

```text
DITCH_APPROVAL_APPROVE
DITCH_APPROVAL_DENY
DITCH_APPROVAL_OPEN
```

For each notification, include the Ditch approval request ID in `userInfo`.

- `Approve` resolves the request with the safest offered affirmative decision: normally approve once.
- `Deny` resolves with decline.
- Opening the notification activates Ditch, selects the relevant project/agent, and opens the in-app approval details where any richer decision can be chosen.

Do not expose `Approve for session` from the native notification unless the notification text makes the persistence scope unmistakable. Keep it available in the full in-app approval card.

### Notification process bridge

The native Swift notification delegate must send an event through the existing Flutter method channel. Flutter forwards it to `ditchd` using `ResolveApproval`.

The runtime owns final validation:

1. Confirm the request still exists and belongs to the active runtime instance.
2. Confirm the requested decision remains available.
3. Send the JSON-RPC response to App Server.
4. Remove or update the in-app attention card only after App Server emits `serverRequest/resolved` or a terminal item event.
5. Remove delivered notifications for the resolved request.

Notification actions can arrive after an approval has already been resolved in-app. Treat that as a harmless no-op and remove the stale notification.

## In-app approval UI

Add a pending-approval card to the expanded agent view and existing attention panel.

It must show:

- Agent display name and project name.
- Approval type.
- Command, file-change, network destination, or permission request as appropriate.
- Working directory and reason when supplied.
- Which options are one-time versus session-wide.
- Approve, deny, cancel, and any provider-offered richer decisions.
- A link/action to open the agent conversation.

When an approval is pending:

- agent state becomes `AwaitingApproval`;
- the composer remains visible but does not start a competing turn for that agent;
- stop/interruption remains available;
- the card is removed only after resolution confirmation;
- native notification and in-app card stay synchronized.

## macOS composer controls

Place compact controls above the prompt field:

```text
[ Approval preset v ] [ Model v ] [ reasoning effort v, when applicable ]
<prompt editor>                                      [ Send ]
```

Requirements:

- Use App Server `model/list`; no hard-coded model catalog.
- Show provider display names such as `GPT-5.6 Sol`.
- Filter hidden models by default.
- Show only reasoning efforts supported by the selected model.
- Preserve a historical unavailable model as read-only text rather than silently replacing it.
- Render controls responsively: wrap or use compact menu labels at narrow widths; never overflow the composer.
- Store selection per agent/session draft, not in a global singleton shared by every agent.
- Changes while an agent is working apply to its next turn only.
- Full Access has warning color, explanatory tooltip, and explicit confirmation.

## Persistence and recovery

The App Server owns canonical Codex thread/turn history. Ditch database stores presentation indexes and Ditch-owned metadata.

Persist:

- provider thread ID and session ID;
- provider turn and item IDs;
- Ditch turn record and effective profile;
- rendered messages/items with idempotency keys;
- pending-approval audit metadata;
- resolved approval decision and time.

On Ditch UI reconnect while `ditchd` and App Server are alive, rehydrate pending approvals and re-deliver any missing native notifications.

On `ditchd` or App Server restart, old server request IDs are invalid. Mark the associated turn interrupted/stale, clear actionable pending approvals, dismiss stale notifications, and explain that the user can resume the agent in a new turn. Do not render a stale approval as actionable.

## Provider abstraction for future Claude Code support

Define a provider adapter interface:

```text
AgentProviderAdapter
  discoverCapabilities()
  discoverModels()
  startThread(profile)
  resumeThread(providerThreadId)
  startTurn(prompt, profile)
  interruptTurn()
  resolveApproval(request, decision)
  streamEvents()
```

Codex App Server is the first implementation. A future Claude Code adapter maps Ditch's product-level presets to Claude's supported model, permission, and tool-confirmation controls, and reports unavailable capabilities rather than pretending parity.

## Migration plan

1. Add provider-neutral profile, capability, turn, and structured approval types.
2. Add database migrations for turns, approval records, and provider item IDs.
3. Build a tested App Server JSON-RPC client in `ditchd`.
4. Migrate new-session, resume, prompt, interruption, and event streaming from `codex exec` JSONL to App Server.
5. Implement model and managed-policy discovery.
6. Implement App Server approval request parsing and structured protocol events.
7. Implement in-app approval cards and resolution actions.
8. Add native macOS notifications and actions.
9. Implement stale-request and restart handling.
10. Replace temporary CLI approval/profile controls with per-session capability-driven state.
11. Add provider adapter contracts and leave Claude Code unsupported-but-ready until its adapter exists.

## Acceptance criteria

### Model controls

1. Opening a composer displays models returned by the currently authenticated Codex App Server.
2. Selecting a model and effort starts the next turn with those values.
3. Changing a model while Codex is working displays `Applies next turn` and does not alter the active turn.
4. Each completed turn displays/stores the effective model and execution profile used.

### Ask for approval

1. Selecting `Ask for approval` produces App Server turns with `approvalPolicy: on-request`, `approvalsReviewer: user`, and workspace-write sandboxing.
2. A command requiring approval moves the agent into `AwaitingApproval`.
3. The in-app card contains the correct agent name, project name, action/command, cwd, and offered decisions.
4. macOS displays one corresponding native notification with approve and deny actions.
5. `Approve` in-app resumes the exact Codex action.
6. `Deny` in-app declines the exact Codex action.
7. `Approve` and `Deny` from macOS notification actions perform the same resolution.
8. Resolving in one place clears the other surface.
9. An already resolved notification action is harmless and does not affect a newer request.

### Safety and recovery

1. Full Access requires confirmation and is blocked when managed policy forbids it.
2. Commands in notification previews are truncated and secret-redacted.
3. Restarting `ditchd` while an approval is pending leaves no stale actionable notification.
4. A runtime/App Server disconnect records the turn interruption and allows a safe later resume.
5. Existing persisted Codex threads can be resumed through App Server without duplicated transcript messages.

### Tests

- Rust unit tests for Codex profile mapping, App Server JSON-RPC correlation, event parsing, approval decision routing, stale requests, and idempotent persistence.
- Runtime integration tests using a fake App Server fixture for model discovery, turn streaming, command approval, file approval, deny, session approval, and restart behavior.
- Flutter widget tests for responsive composer controls, unavailable managed choices, confirmation dialogs, in-app approval cards, and next-turn semantics.
- Swift/macOS tests for notification category registration, payload construction/redaction, approve/deny action forwarding, and stale notification cleanup.
- Manual verification on macOS with a real Codex account and notification permission both granted and denied.

## Explicit non-goals for this implementation

- Implementing the Claude Code adapter itself.
- Exposing every experimental App Server feature unrelated to models or approvals.
- Persisting an actionable approval across a runtime/App Server restart.
- Allowing model or sandbox changes to mutate an already running turn.
- Making native notifications the only approval surface; the in-app card remains required for accessibility and richer decisions.
