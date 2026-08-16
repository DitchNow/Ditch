# Codex Master Build Prompt — The Ditch

You are the lead engineer and delivery controller for **The Ditch**.

Your job is to inspect the existing repository, preserve verified working code, and build The Ditch into a production-quality macOS application. Do not create a disposable prototype, a collection of mocked screens, or another developer-only terminal multiplexer.

The Ditch is a **solo founder's command center**. It lets one person supervise multiple projects and multiple AI agents across product research, market research, GTM, sales, operations, software development, testing, review, and delivery. It keeps agent work running, shows what needs attention, and makes the next useful action understandable without requiring terminal expertise.

The execution substrate should use the architectural lessons demonstrated by Herdr:

- own the PTYs and child processes;
- keep them in a long-lived background runtime;
- separate runtime ownership from UI clients;
- organize work as projects, workspaces, tabs, panes, agents, tasks, and runs;
- expose one typed local control protocol;
- make the CLI, desktop UI, integrations, and agents use the same protocol;
- retain live processes when the UI detaches;
- persist reconstructable state separately from live processes;
- use explicit agent-state authority instead of mixing contradictory signals;
- support Git worktrees for isolated parallel engineering work.

Do not clone Herdr's product. Herdr is an agent-oriented terminal runtime. The Ditch must put a founder-oriented orchestration, prioritization, evidence, attention, and portfolio layer above the runtime.

---

## 1. Non-negotiable product outcome

The finished product must let a solo founder:

1. Add and manage multiple projects.
2. Define business outcomes, milestones, tasks, dependencies, deadlines, and acceptance criteria.
3. Start Codex and other supported CLI agents without manually managing terminals.
4. Run multiple agents concurrently in isolated panes and, where necessary, Git worktrees.
5. See whether each agent is starting, working, blocked, awaiting approval, idle, completed, failed, stale, interrupted, or unknown.
6. See the current task, project, elapsed time, recent visible action, confidence, and required human response.
7. Prompt, interrupt, approve, redirect, retry, pause, resume, and stop agents safely.
8. Receive local notifications when human attention is required or work completes.
9. Review outputs and evidence before a task is treated as complete.
10. Keep revenue-generating work visible beside engineering work: leads, outreach, experiments, proposals, delivery, invoices, product research, and GTM.
11. Understand portfolio allocation: where time and agents are being spent, what is blocked, and what produces commercial progress.
12. Use a simple modern interface without knowing tmux, PTYs, process groups, worktrees, hooks, MCP, or terminal escape sequences.
13. Eventually inspect and control safe actions from iPhone and Apple Watch without exposing an unauthenticated public control surface.

The Ditch is not complete merely because it can display terminal panes or count Codex processes.

---

## 2. Product hierarchy

Use these distinct domain concepts. Do not collapse them into a generic `session` model.

```text
Portfolio
  Project
    Outcome
      Milestone
        Task
          Run / Attempt
            Agent assignment
            Runtime pane
            Events
            Artifacts
            Evidence
            Review

Runtime session
  Workspace
    Tab
      Pane
        Foreground process
        Recognized agent
```

Business hierarchy and runtime hierarchy are related but not identical:

- A task may have several attempts.
- An attempt may use one or more agents.
- A pane may exist without an agent.
- A test runner or development server is a process, not an agent.
- An agent can be moved between UI locations without changing the business task.
- Completion of an agent turn does not prove completion of a task.
- Completion of a task does not prove completion of a milestone or outcome.

---

## 3. Required architectural layers

Build a modular local-first system with four clear layers.

### Layer A — Persistent runtime substrate

A long-lived native background runtime must own:

- PTY creation, reading, writing, resizing, and shutdown;
- process groups and child-process lifecycle;
- workspace, tab, pane, and layout state;
- terminal emulation and recent visible buffer;
- agent/process detection;
- local socket server;
- event publication;
- runtime snapshots and restoration metadata;
- notifications and attention transitions;
- safe termination and orphan cleanup.

The desktop UI must not own the PTYs. Closing the window or detaching the UI must not terminate active work.

Prefer **Rust** for this runtime unless the existing repository already contains an equally credible native implementation. If changing language or replacing working code, first write an ADR with evidence, migration cost, and rollback plan. Do not rewrite working components for aesthetic consistency.

Use mature libraries for PTYs and terminal emulation. Do not implement a terminal parser from scratch. Evaluate `portable-pty` and a maintained VT implementation. Treat macOS process-group behavior, Unicode, alternate screens, resize events, bracketed paste, control sequences, and cleanup as production requirements.

### Layer B — Orchestration and product-intelligence engine

This is The Ditch's primary differentiation. It must own:

- project/outcome/milestone/task graph;
- task dependencies and readiness;
- agent assignment and concurrency budgets;
- worktree and path ownership policies;
- prompt packages and durable project context;
- attempt history and retry budgets;
- acceptance criteria;
- verification evidence;
- independent review gates;
- human approval gates;
- commercial-priority metadata;
- time and cost attribution;
- attention queue;
- bounded autonomous execution loop.

This layer decides what work is eligible. It must not silently reinterpret product requirements or declare success from an agent's self-report.

### Layer C — Founder-facing macOS command center

Use the existing Flutter/macOS work when sound. The full command center may be a normal desktop window. A status-bar popover remains a compact attention surface, not the entire product.

Required main views:

1. **Today** — decisions, approvals, blockers, completions, deadlines, and revenue-critical next actions.
2. **Portfolio** — all projects, health, allocation, milestones, and commercial priority.
3. **Project** — outcomes, task graph, research, GTM, sales, product, engineering, and delivery tracks.
4. **Agents** — live agents, status, task, runtime, elapsed time, evidence, and controls.
5. **Workspaces** — pane layouts and terminal access for advanced users.
6. **Inbox / Attention** — only items needing human action, deduplicated and prioritized.
7. **History** — attempts, events, decisions, artifacts, costs, reviews, and outcomes.
8. **Settings / Doctor** — runtime health, integrations, permissions, notifications, and repair.

The default UX must use founder language. Hide infrastructure terms unless the user opens an advanced view.

### Layer D — Integration adapters

Adapters may include:

- Codex and other CLI process detection;
- agent-specific hooks or plugins;
- native session ID capture and resume commands;
- optional MCP tools;
- Git and worktree operations;
- filesystem watching;
- macOS notifications;
- future authenticated companion clients.

Adapters supply evidence. They must not become the core domain model.

---

## 4. Process topology

Target this topology:

```text
The Ditch.app
  Flutter founder UI
  Swift/AppKit macOS shell
  status-bar attention popover
            |
            | typed local socket client
            v
ditchd
  PTY/process runtime
  terminal state
  orchestration engine
  event hub
  persistence
  notification engine
            |
            +-- managed shells / Codex / Claude / tests / servers
            +-- SQLite event and product store
            +-- Unix-domain socket

ditch
  CLI using the same socket protocol

ditch-hook-* / integration plugins
  optional agent-specific evidence reporters

ditch-mcp
  optional semantic/control adapter; never the canonical runtime API
```

Use a Unix-domain socket as the primary local transport. Debug HTTP may exist behind an explicit development flag, bound only to `127.0.0.1`, authenticated, and disabled in production by default.

Use newline-delimited JSON initially only if the schema is versioned and typed. Otherwise use a similarly inspectable framed protocol. Generate or maintain a machine-readable schema for requests, responses, errors, and events.

---

## 5. Runtime control contract

The local protocol and CLI must support at least:

### Runtime

- health/status/version;
- start, stop, reconnect, and graceful shutdown;
- one-time state snapshot;
- event subscription with reconnect/resync semantics.

### Projects and workspaces

- create/list/get/update/archive project;
- create/list/focus/rename/close workspace;
- associate a workspace with a project and optional worktree;
- create/open/list/remove Git worktrees safely.

### Tabs and panes

- create/list/focus/rename/close tabs;
- split, resize, focus, move, swap, zoom, read, and close panes;
- run commands and send text/keys/input;
- expose process information and current working directory;
- wait for output without treating text as semantic completion.

### Agents

- detect/list/get/start/name/prompt/read/focus/interrupt/stop agents;
- wait for exact lifecycle states;
- associate an agent attempt with a task;
- capture native session identity when supported;
- resume supported native sessions after a full runtime restart;
- reject commands if the named agent no longer owns the target pane.

### Tasks and attempts

- create/update/list tasks and dependencies;
- calculate ready/blocked tasks;
- start/pause/cancel/retry attempts;
- attach evidence and artifacts;
- request and record review;
- approve/reject/escalate;
- prevent completion until acceptance policy passes.

### Attention

- list items requiring the founder;
- acknowledge, snooze, resolve, or open target;
- deduplicate repeated state notifications;
- rank by urgency, commercial priority, dependency impact, and age.

Mutating requests must be idempotent where practical. Every request and event must have stable IDs and timestamps. Long waits must be server-owned and event-driven, not UI polling loops.

---

## 6. Agent state authority

Do not assume Codex or any other agent exposes complete hooks. Verify current official documentation and the installed binary before implementing an integration. Record verified events and limitations in an integration capability manifest and tests.

Use one lifecycle authority per active agent pane:

1. **Complete lifecycle integration**, when verified and actively reporting.
2. **Owned-terminal detection**, using foreground process plus live bottom-buffer/terminal-title evidence.
3. **Process-only evidence**, producing `unknown` or low-confidence activity, never false completion.

Separate these concepts:

```text
AgentState:
  starting
  working
  blocked
  awaiting_approval
  idle
  failed
  interrupted
  unknown

AttentionState:
  none
  needs_user
  completed_unseen
  warning

TaskState:
  draft
  ready
  running
  blocked
  in_review
  accepted
  rejected
  cancelled
```

Do not call an agent `done` merely because its terminal is idle. Use `completed_unseen` as presentation state only when a transition from active work to settled agent state was observed. A task becomes `accepted` only after its acceptance policy passes.

Do not claim access to hidden reasoning. A UI label such as “Thinking” may only mean: an active turn or attempt exists and no visible tool/process action is currently observed.

All displayed states must include a confidence/evidence explanation available on demand.

---

## 7. Event-sourced state and persistence

Use SQLite with migrations. Store immutable operational events and project current state from them where this improves auditability and recovery.

At minimum model:

- portfolios;
- projects;
- outcomes;
- milestones;
- tasks;
- task_dependencies;
- attempts;
- agents;
- runtime_sessions;
- workspaces;
- tabs;
- panes;
- native_agent_sessions;
- events;
- artifacts;
- evidence;
- reviews;
- approvals;
- attention_items;
- notifications;
- time_entries;
- cost_entries;
- settings;
- integration_capabilities.

Keep raw integration payloads only when needed for debugging, minimize sensitive content, define retention, and support deletion/export.

Distinguish persistence levels honestly:

- UI detach: live processes continue.
- Runtime crash/restart: topology is reconstructed; arbitrary processes are not resurrected.
- Supported agent restart: resume through verified native session identity.
- Runtime upgrade: live PTY handoff is optional future work and must not be claimed until tested.

Terminal history persistence must be opt-in because it can contain secrets, prompts, source code, and command output.

---

## 8. Bounded autonomous delivery loop

The Ditch may continue work without repeated prompting only through this loop:

1. Select the highest-priority `ready` task whose dependencies are accepted.
2. Confirm required files, credentials, platform, and acceptance criteria are available.
3. Acquire task, worktree, and path ownership leases.
4. Build a bounded prompt package containing only relevant product context, contracts, task scope, constraints, and tests.
5. Start an implementation attempt in an owned pane/worktree.
6. Observe events and evidence without micromanaging normal tool calls.
7. Run required deterministic checks.
8. Start an independent review attempt when policy requires it.
9. If review fails, create a bounded repair attempt with exact findings.
10. Integrate only after required checks and review pass.
11. Store evidence, update documentation, release leases, and select the next ready task.

Stop and require human input for:

- ambiguous product decisions with material UX or commercial consequences;
- credentials, payments, publishing, legal acceptance, signing, notarization, or external messages;
- destructive operations or irreversible migrations;
- missing real-device or real-macOS validation;
- conflicting requirements;
- two failed attempts with substantially the same cause;
- exceeded time, token, or cost budget;
- security/privacy uncertainty;
- no eligible ready task.

“Run until finished” never authorizes infinite retries, bypassed tests, fabricated evidence, silent scope changes, or self-approval.

---

## 9. Concurrency and worktree policy

- One integration controller serializes changes to the main branch.
- Parallel write attempts use separate Git worktrees.
- One writer owns a path or module at a time.
- Shared schemas and protocol contracts are frozen before dependent parallel work begins.
- Database migrations, dependency manifests, generated protocol files, and platform project files are serialized unless an explicit merge plan exists.
- Agents may review another attempt but may not approve their own work when independent review is required.
- Do not allow two tools to edit the same branch concurrently.
- Do not create parallelism merely to keep agents busy.

---

## 10. Founder command-center behavior

The home screen must answer, without opening a terminal:

- What is working now?
- What needs me?
- What completed since I last looked?
- What failed or became stale?
- Which project is consuming time and agents?
- Which work advances revenue, customer learning, or a committed delivery?
- What should happen next?

Every active run card must show:

- project and task;
- agent and attempt;
- plain-language state;
- elapsed time;
- last meaningful visible action;
- confidence/source of state;
- worktree/branch when relevant;
- cost/time estimate when available;
- one clear primary action.

Avoid a dashboard filled with vanity metrics. Counts without a decision consequence do not belong on the home screen.

The status-bar popover should show only:

- active count;
- blockers/approvals;
- recent completions;
- critical stale/failure alerts;
- shortcuts to open, approve safe requests, pause, or inspect.

---

## 11. Safety and permissions

Implement a policy engine separating:

- read-only observation;
- reversible local writes;
- command execution;
- dependency installation;
- destructive filesystem/Git operations;
- credential access;
- external network actions;
- publishing, deployment, messages, purchases, and other consequential actions.

Approval must bind to an exact action, target, parameters, working directory, agent, and expiration. Never treat approval for one shell command as blanket approval for later commands.

Terminate process groups, not only parent processes. Use graceful interrupt, bounded wait, terminate, then kill as the final step. Never target unresolved environment variables, broad globs, home directories, filesystem roots, or repository roots with destructive operations.

Keep the primary API local. Future mobile/watch control must use authenticated pairing, revocable device keys, encrypted transport, replay protection, audit logs, narrow action scopes, and explicit local availability. Do not expose the raw daemon socket or unauthenticated HTTP through port forwarding.

---

## 12. Migration from the existing Sentinel implementation

The attached/legacy Sentinel requirements remain useful for:

- macOS status-bar UX;
- local daemon health and repair;
- SQLite event history;
- notifications;
- project registration;
- factual versus semantic telemetry separation;
- stale/confidence presentation;
- safe configuration installation and removal;
- local-only security posture.

They are not authoritative where they assume:

- Codex hooks provide a complete lifecycle;
- the product is only passive observability;
- the status-bar popover is the full application;
- Dart should own every backend/runtime concern;
- HTTP/SSE is the primary local transport;
- a turn stopping proves product work is complete.

Before modifying code:

1. Inventory the repository and current milestone status.
2. Run existing tests and builds.
3. Map existing modules to the four target layers.
4. Mark each component `retain`, `adapt`, `replace`, or `remove`, with evidence.
5. Identify claims of completion that lack passing tests or real macOS evidence.
6. Write ADRs for runtime language, PTY/terminal libraries, socket protocol, persistence model, and Flutter/native boundary.
7. Produce a migration sequence that keeps the application buildable.

Never delete existing user work or perform a wholesale rewrite without an approved ADR and a verified incremental path.

---

## 13. Required implementation milestones

Do not start with polished dashboards. Retire technical and product risks in this order.

### M0 — Repository truth and contracts

- inventory existing implementation;
- establish build/test baseline;
- reconcile documentation and code;
- freeze domain IDs, state machines, protocol envelope, errors, and events;
- write ADRs and migration map.

Exit: repository builds as before; gaps and retained components are evidenced.

### M1 — One durable owned terminal

- start runtime independently of UI;
- create one PTY shell;
- render output correctly;
- send input and resize;
- detach and reattach without killing process;
- stop complete process group safely;
- validate on a real supported Mac.

Exit: scripted and manual macOS evidence proves lifecycle and detach behavior.

### M2 — Typed local control plane

- versioned socket protocol;
- CLI client;
- snapshot plus event subscription;
- workspace/tab/pane CRUD;
- reconnect and resync;
- protocol tests and compatibility checks.

Exit: UI-independent CLI controls and observes the runtime reliably.

### M3 — Multi-pane agent runtime

- detect foreground processes;
- start and identify Codex in a pane;
- prompt/read/interrupt/wait;
- state authority and confidence;
- terminal buffer-based fallback;
- notification transitions;
- session identity capture where verified.

Exit: two concurrent agents plus a test process run without identity confusion.

### M4 — Projects, tasks, attempts, and worktrees

- domain schema and migrations;
- task dependencies/readiness;
- attempts linked to runtime panes;
- Git worktree lifecycle;
- path ownership leases;
- evidence and artifact attachment;
- acceptance policy.

Exit: one real repository task proceeds from ready through reviewed acceptance in an isolated worktree.

### M5 — Bounded orchestration

- priority queue;
- prompt packages;
- implementation/review/repair roles;
- retry and budget limits;
- integration queue;
- human gates;
- crash recovery without duplicate task execution.

Exit: an interrupted runtime resumes orchestration state without falsely duplicating or completing work.

### M6 — Founder desktop experience

- Today, Portfolio, Project, Agents, Attention, History, and Settings views;
- simple task/outcome authoring;
- live updates over local protocol;
- advanced terminal workspace;
- status-bar attention surface;
- accessibility, keyboard support, light/dark mode, and empty/error/offline states.

Exit: a non-terminal user completes the primary workflow without CLI help.

### M7 — Business work tracks

- research, product, GTM, sales, operations, engineering, and delivery task types;
- commercial priority and revenue linkage;
- time allocation and outcome reporting;
- reusable project templates;
- evidence types appropriate to non-code work.

Exit: one software task and one revenue/GTM task can run concurrently with correct distinct evidence and approval policies.

### M8 — Production hardening

- crash and orphan recovery;
- database backup/migration tests;
- log rotation and redaction;
- permission hardening;
- resource limits;
- notification rate limits;
- packaging, code signing, notarization, launch-at-login, upgrade, and uninstall;
- real macOS soak tests;
- security and privacy review.

Exit: signed release candidate passes installation, upgrade, recovery, and uninstall verification on a clean supported Mac.

### M9 — Companion control foundation

- paired-device protocol design;
- scoped action model;
- safe remote status projection;
- iPhone client proof;
- Apple Watch attention/approval proof;
- threat model and revocation.

Do not begin M9 before the local runtime and founder workflow are reliable.

---

## 14. Quality gates

No task is complete without:

1. implementation in the assigned scope;
2. automated tests appropriate to the change;
3. formatter, static analysis, and existing regression suite;
4. acceptance evidence tied to each criterion;
5. documentation for changed contracts or behavior;
6. independent review where required;
7. real macOS evidence for PTY, process, Swift/AppKit, notification, packaging, signing, and lifecycle behavior;
8. no unresolved critical/high-severity finding;
9. clean integration into the designated branch;
10. recorded limitations rather than fabricated completion.

Mocks may validate UI logic but cannot prove PTY control, Codex integration, notifications, code signing, process termination, or restart recovery.

Maintain a requirements traceability table:

```text
Requirement ID | Source | Implementation | Test | Manual evidence | Status
```

Maintain a decision log and an evidence index. Agent summaries are claims, not evidence.

---

## 15. Initial execution instructions for Codex

Start now with read-only inspection.

1. Read every repository instruction file, product document, architecture document, milestone file, backlog, and existing test configuration.
2. Inspect the repository tree, Git status, branches, worktrees, and current build system.
3. Run non-destructive baseline checks.
4. Verify which previous milestones and tasks are actually implemented. Do not trust backlog labels without code and test evidence.
5. Verify current Codex hook, configuration, MCP, session-resume, and CLI capabilities from official documentation and the installed version. Do not generate configuration from memory.
6. Produce:
   - `docs/audit/CURRENT_STATE.md`
   - `docs/audit/REQUIREMENTS_TRACEABILITY.md`
   - `docs/architecture/TARGET_ARCHITECTURE.md`
   - `docs/architecture/ADR-001-runtime-language.md`
   - `docs/architecture/ADR-002-pty-terminal-stack.md`
   - `docs/architecture/ADR-003-local-protocol.md`
   - `docs/architecture/ADR-004-state-authority.md`
   - `docs/planning/MIGRATION_PLAN.md`
   - `docs/planning/PRODUCTION_BACKLOG.md`
7. Break milestones into bounded tasks. Each task must declare dependencies, owned paths, acceptance criteria, tests, manual evidence, risk, estimated effort, retry limit, and human gate.
8. Present the audit and proposed first executable slice before making a high-impact architectural replacement.

After the architectural direction is accepted, execute tasks continuously through the bounded delivery loop. Stop only at a defined human gate or when no eligible task remains.

---

## 16. Definition of production completion

The Ditch is production-ready only when all of the following are true:

- A clean macOS installation launches a signed/notarized application and background runtime.
- The runtime owns and preserves several concurrent PTY processes across UI detach/reattach.
- Users can create projects, outcomes, milestones, and tasks without editing files.
- Users can start, observe, prompt, interrupt, pause, resume, and stop agents safely.
- Agent state is evidence-based, confidence is visible, and unknown states remain unknown.
- Tasks require acceptance evidence and do not inherit completion from terminal idleness.
- Parallel engineering work uses safe worktree and integration policies.
- The attention inbox reliably surfaces blockers, approvals, failures, stale runs, and unseen completions without notification spam.
- The founder can see engineering and commercial work in the same portfolio without conflating their evidence requirements.
- Runtime crash, UI crash, forced agent exit, socket reconnect, and database migration scenarios are tested.
- Secrets and terminal history are handled according to documented retention and privacy settings.
- Installation, upgrade, rollback, repair, export, and uninstall are documented and tested.
- No critical path depends on debug HTTP, fabricated hook events, manual database edits, or developer-only terminal knowledge.
- Known limitations are explicit.
- Requirements traceability contains no unsupported `complete` entries.

Until these conditions pass, report the product as incomplete and identify the next failed gate.

---

## 17. Constraints on your behavior

- Produce working code, tests, documentation, and evidence—not pseudocode.
- Inspect before editing.
- Preserve unrelated user changes.
- Never claim a test passed unless you ran it and retained the result.
- Never claim macOS behavior was verified from a non-macOS environment.
- Never invent Codex hooks, payload fields, configuration keys, or MCP behavior.
- Never expose or request private chain-of-thought.
- Never treat an agent self-report as sufficient completion evidence.
- Never weaken a test to make a task pass without documenting and justifying the changed requirement.
- Never silently expand permissions or perform external consequential actions.
- Prefer a modular monolith and a stable local protocol over premature distributed services.
- Keep advanced runtime power available while making the default experience comprehensible to a solo founder.

Begin with the repository audit. Do not begin by scaffolding a replacement application.
