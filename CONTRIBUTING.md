# Contributing to Ditch Community Edition

Bug reports, reproducible test cases, documentation corrections, and focused pull requests are useful. Small fixes should be easy to submit. Discuss large, architectural, or behavior-changing work before investing in an implementation.

## Before You Start

1. Search [existing issues](https://github.com/DitchNow/Ditch/issues) and [Discussions](https://github.com/DitchNow/Ditch/discussions) for the same problem or proposal.
2. Reproduce the problem where applicable and reduce it to the smallest reliable case.
3. Read the [How It Works](README.md#how-it-works) section and inspect the relevant implementation before changing cross-component behavior.
4. Start a Discussion before large features, architectural changes, schema or protocol changes, or behavior that affects several components.
5. Keep each pull request focused on one coherent concern.

Unsolicited wholesale rewrites may not be accepted when they conflict with existing runtime ownership or application boundaries. This should not delay a small bug fix, test, or documentation correction.

## Ways to Contribute

- Report a reproducible bug.
- Add a focused regression test or reduced test case.
- Correct inaccurate or unclear documentation.
- Fix a small, well-understood defect.
- Improve performance, reliability, or accessibility in an existing workflow.
- Propose a carefully scoped feature that belongs in the current local product.
- Open an issue to discuss an architectural change before implementation.

## Reporting Bugs

Use the bug report issue form and include:

- macOS version;
- Ditch version or commit;
- Codex version when the problem involves Codex;
- exact reproduction steps;
- expected and actual behavior;
- relevant logs or error output; and
- whether the problem occurs consistently.

Ditch runtime writes logs under `~/Library/Application Support/The Ditch/logs/`. Logs, screenshots, transcripts, and diagnostics may contain repository paths, prompts, commands, or source text. Inspect and redact them before posting.

Do not include secrets, API credentials, authentication tokens, private source code, personal data, or other sensitive information in a public issue.

## Feature Requests

Lead with the problem rather than a proposed implementation. Describe the current workaround, the concrete behavior you would like, and why that behavior belongs in Ditch Community Edition.

Comparisons can provide context, but “add X because another product has X” is not enough to evaluate a change. Explain the workflow and outcome the feature would improve.

## Architecture Changes

Start a [GitHub Discussion](https://github.com/DitchNow/Ditch/discussions) before implementing a change that affects:

- `ditchd` process or runtime ownership;
- Codex launch, resume, interruption, or execution behavior;
- local IPC or protocol contracts;
- SQLite persistence, migrations, or durable state;
- session lifecycle or state authority;
- PTY creation, I/O, resizing, or process groups;
- project filesystem boundaries or file-write safety;
- provider abstractions;
- major UI/runtime responsibility boundaries; or
- security-sensitive execution paths.

These interfaces affect reliability, recovery, and compatibility across the application. A short issue describing the problem, constraints, proposed boundary, and migration impact is usually enough to begin discussion.

Use these principles when shaping a change:

### Runtime authority

Execution and process truth belongs in the runtime layer. Do not duplicate authoritative lifecycle state or process control in Flutter presentation state.

### Clear interfaces

Prefer explicit request, response, and event boundaries over implicit coupling between UI, runtime, persistence, and agent-provider code.

### Local-first core

Keep the Community Edition local runtime self-contained. Do not introduce mandatory external hosted services, remote state, authentication systems, or control planes into core local execution paths.

### Provider isolation

Keep provider-specific behavior at the existing provider/runtime boundary where practical. Avoid spreading Codex command details into otherwise provider-neutral state or UI code without a concrete reason.

### Backward compatibility

Treat persisted data and the versioned local protocol as compatibility boundaries. Schema and protocol changes need explicit migration, fallback, or version-handling decisions rather than silent reinterpretation of stored state.

## Development Setup

Development requires macOS 11 or later, Git, Flutter with macOS desktop support and a Dart SDK compatible with `^3.10.4`, Rust 1.85 or later, and Xcode with its macOS command-line build tools. Codex-related testing also requires a compatible Codex CLI installation signed in with `codex login`.

Clone the repository and fetch Flutter dependencies:

```sh
git clone https://github.com/DitchNow/Ditch.git
cd Ditch/apps/macos
flutter pub get
```

Run the macOS application:

```sh
flutter run -d macos
```

The macOS build phase builds and packages the Rust runtime and CLI. To build those components directly from the repository root:

```sh
cargo build --locked -p ditchd -p ditch_cli
```

You do not need to start a separate daemon for the normal Flutter development path; the app bundle includes the menu-bar runtime helper.

## Pull Requests

A pull request should:

- address one coherent concern;
- explain what changed and why;
- link the relevant issue when one exists;
- include tests for behavior changes where practical;
- include screenshots or a short recording for meaningful UI changes;
- update documentation when user-visible behavior changes;
- avoid unrelated formatting, renaming, and refactoring;
- avoid generated files unless the build requires them; and
- justify any new dependency and its effect on the local product.

Small fixes do not need a design document. Describe the failure, the fix, and how you checked it.

## PR Size

Prefer small changes that can be understood and reviewed independently. Split preparatory refactors from behavior changes when doing so produces meaningful, working increments.

For major changes, open an issue and agree on direction before investing heavily in implementation. A large pull request is not a substitute for prior agreement on architecture or scope.

## Testing

GitHub Actions runs the standard checks, but contributors should run relevant checks locally before opening a pull request.

From the repository root, validate Rust code with:

```sh
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
```

From `apps/macos`, validate Flutter code with:

```sh
flutter analyze
flutter test
```

For changes to the macOS application, also run `flutter run -d macos` and manually exercise the affected workflow. Explain any check you could not run in the pull request.

## UI Changes

Preserve the application's existing visual language and interaction patterns. Reuse established theme tokens, controls, layouts, and native integration patterns instead of adding an unrelated design system.

Account for loading, empty, error, disabled, focus, and narrow-window states where relevant. Preserve keyboard behavior and existing macOS conventions. Include before/after screenshots or a short recording for visible changes.

## Security

Do not publish exploit details, credentials, tokens, private prompts, repository contents, or other sensitive data in an issue, pull request, or Discussion. Follow [SECURITY.md](SECURITY.md) for vulnerability-reporting guidance.

## Contribution Licensing

Community Edition remains licensed exclusively as [GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`). This policy does not relicense the public project. Community functionality accepted into an official Community release remains available in Community Edition; a Commercial subscription does not make that functionality paid-only.

The project does not currently ask contributors to sign a separate Contributor License Agreement. DitchNow has not yet adopted final inbound terms that would permit third-party Community source to be included in the statically composed proprietary Commercial application. Until qualified counsel approves and maintainers publish that workflow, maintainers must not merge external source contributions into release branches. Issues, design discussion, reproducible test cases, and suggested patches remain welcome for evaluation, but this document does not grant DitchNow additional proprietary rights in submitted code.

A Developer Certificate of Origin records a contributor's certification about a contribution; by itself it is not permission for proprietary distribution. If a future no-separate-CLA workflow combines DCO sign-off with published inbound licensing terms, those terms and the exact submission process require legal approval before use.

Do not submit proprietary Commercial implementations—including iPhone Remote Control, Relay connectivity, pairing, mobile command transport, mobile projections, or paid capability enforcement—to the Community repository. Proposals for hosted or proprietary capabilities should be discussed separately with maintainers.

## What Not to Submit

- Unrelated large refactors or mass formatting mixed with functional work.
- Generated code dumps or speculative abstractions without a current use case.
- Mandatory hosted or external-service dependencies for core local behavior.
- Embedded credentials, tokens, private source, or user data.
- Proprietary Commercial implementation or protocol contracts.
- License changes.
- Wholesale architecture rewrites without prior discussion.

## Review and Merge

Maintainers review pull requests and may request revisions, narrower scope, additional tests, or a different architectural boundary. Changes that conflict with the current product scope, local-first core, security requirements, or established responsibilities may be closed.

Approval does not guarantee an immediate merge, and silence should not be interpreted as acceptance. There is no promised review or merge timeline.
