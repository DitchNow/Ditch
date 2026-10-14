# Task and skill execution threat model

Community retains one runtime/process owner and one SQLite connection owner. The local socket is mode 0600 and assumes a trusted local user. A target SSH runtime assumes a trusted SSH account; it does not claim confinement to a project directory when the installed target provider cannot enforce it.

| Threat | Enforced control | Evidence / remaining boundary |
|---|---|---|
| Two tasks write overlapping workspaces | Canonical host-aware `WorkspaceLease`, persisted reservation before launch; unconfined runs exclusive | Barrier-synchronized claims, nested roots, restart with surviving process group, independent-project tests |
| Old provider frames modify a new attempt | Owned run/thread/turn identifiers, monotonic task revision checks | App Server event gate and stale remote projection tests |
| Model declares its own work accepted | Task state machine requires explicit user Accept and current review evidence | Review, stale snapshot, and invalid-transition tests; preserved legacy Accepted rows are historical imports |
| File/context path traversal or symlink substitution | Validate relative components; directory-fd `openat` and `O_NOFOLLOW` reads for context, content predicates, hashes and skill trees | Link/FIFO/size fixtures; privileged replacement of a root or ancestor remains outside a complete adversarial filesystem proof |
| Unbounded input or notification flood | Bounded IPC/App Server frames, RPC absolute deadline, catalog and evidence limits | Expired-deadline/oversized-file fixtures; blocking OS writes and process startup are not exhaustively fault-injected |
| Hostile managed skill archive/source | Read bounded data, validate paths/hashes, stage before promotion, no source scripts/hooks/submodules | Install/update/rollback/pinned revision/remote checksum fixtures; model interpretation of instructions is not proven prompt-injection safe |
| Changed skill between assignment and execution | Durable selected revision/hash; verify again before execution | Changed-binding rejection and native recognition fixture |
| Validator command injection | Confirmed argv arrays, no implicit shell interpolation, selected profile and bounded deadline | A user can explicitly approve a shell command; this is then executable user-approved code |
| Secrets in output/context | Known secret-file exclusion and heuristic output redaction | Bearer/private-key marker regression; arbitrary secrets and authorized direct project reads are not universally filtered |
| Cancellation releases ownership before child exits | Owned process groups and persistent reservations; validator cancellation before release | Native installed-Codex cancellation and force-stop tests |
| Corrupt extension data damages core state | Unknown tables/settings preserved; incompatible authoritative records fail closed | Intermediate migration fixtures preserve 1,000 tasks and opaque extension data |

No skill import should mutate global Codex configuration or copy authentication material. Installation and synchronization operate on explicit Ditch-managed revisions. Provider-owned Codex history is separate from Ditch's bounded UI projections.

The practical concurrency tests do not replace ThreadSanitizer or formal verification. Real SSH trust behavior, historical provider versions, native accessibility, and sustained multi-project load require environment-specific validation before release.

## GitHub integration boundary (2026-09-29)

GitHub operations retain the native-peer check before runtime dispatch; the shared helper is also used by Commercial Ditchmaster. Worker tools and the typed mobile command surface do not expose GitHub connection/import operations. No caller-provided actor flag authorizes them.

The integration executes only the pinned managed CLI and internally constructed GET requests. It removes inherited tokens and browser/editor/pager overrides. Authentication uses noninteractive pipes with stdin closed and prompts disabled; only the expected device code and fixed github.com device URL reach the native panel. Authentication output is not published as events or agent scrollback.

The explicit Connect action records acceptance of the inline disclosure. Consent and a stable GitHub user ID, configuration path, and connection generation are stored without credentials. Active-account/storage verification runs before and after each private GET, and the generation is checked under the store lock before publishing or committing. External shared-account changes require a new browser connection. These checks detect practical drift but cannot transactionally isolate the account from another same-user process.

Disconnect never runs logout, deletes shared config, or revokes authorization. It invalidates pending work and clears transient browsing views; imported task/source snapshots are deliberate retained local copies. An observed plaintext fallback rejects the connection and reports that shared credentials were not erased. Ditch does not attempt unsafe cleanup of a shared configuration file.

The broker is not OS isolation from unrestricted same-user shell processes. Agent/project permissions are unchanged. Imported issue content is task data and is explicitly identified as untrusted in execution prompts; it cannot authorize tools or override user/project instructions. Bodies/comments are displayed as literal Markdown text, with no executable HTML or automatic external-resource fetches.
