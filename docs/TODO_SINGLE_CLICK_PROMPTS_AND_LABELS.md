Implement one bounded vertical feature in the current The Ditch codebase: prompt-control actions for the existing Codex chat composer.

Do not redesign the application, replace the composer, introduce a second state architecture, or create a parallel message model. First inspect the repository and read all applicable `AGENTS.md`, product, architecture, design-system, state-management, persistence, daemon/IPC, session, and testing documentation. Locate the current chat composer, message submission flow, Codex input boundary, transcript rendering, session persistence, theme tokens, reusable buttons/chips, and existing tests.

Before editing, briefly report:

1. The current path from typed text to the Codex runtime.
2. The existing state-management and message models involved.
3. The design-system components and tokens that should be reused.
4. The exact files you expect to change.
5. Any conflict between this requirement and an existing architectural contract.

If no material conflict exists, proceed with implementation without waiting for another routine confirmation. If implementing this feature would require changing a frozen contract, bypassing `ditchd`, replacing the current state-management approach, or creating a competing message pipeline, stop and explain the conflict instead.

## Goal

Add seven prompt-control options associated with the existing chat composer:

Primary actions:

* Understand
* Propose
* Plan
* Double-check
* Repeat
* Implement

Modifier:

* Do not edit

The user writes the substantive prompt normally. Selecting an action adds a visible chip/token to the composer, but does not insert or display the predefined expansion inside the editable text field.

When the user submits, The Ditch constructs the expanded outbound prompt internally and sends it through the existing Codex submission path. The normal chat transcript shows the user’s original text and selected action metadata, not the hidden expansion.

## Architectural constraints

Preserve the current architecture.

* Use the existing state-management system.
* Use the existing composer and message-submission boundary.
* Route submission through the existing application/daemon/client abstraction.
* Do not let Flutter or UI code bypass `ditchd` or write directly to a PTY if the current architecture assigns runtime ownership to `ditchd`.
* Do not introduce a new database, event bus, dependency-injection framework, navigation system, state library, design system, or generic prompt-engineering framework.
* Do not create provider-specific logic in shared UI components unless the existing architecture already places it there.
* Keep prompt expansion as a small, testable domain/application concern rather than concatenation scattered through widgets.
* Preserve current session continuity, runtime ownership, error handling, and message ordering.
* Preserve unrelated behavior and user changes.
* Do not modify the old Sentinel reference prompt or use it as architectural authority.
* Do not perform unrelated refactors or visual redesigns.
* Do not create commits unless explicitly authorized.

If there is already an appropriate message metadata or command model, extend it. If not, introduce the smallest model consistent with current conventions.

## Interaction model

### Toolbar

Place the prompt-control buttons in the existing composer area, directly above the editable prompt field or in the closest location consistent with the current layout.

The controls must feel native to the existing application rather than attached as a visually separate feature.

Use existing:

* typography;
* colors;
* corner radii;
* spacing;
* borders;
* hover states;
* focus states;
* icon style;
* light/dark theme tokens;
* animation conventions.

Do not hardcode an unrelated color palette.

The toolbar must support narrower window sizes without breaking the composer. Follow the application’s current responsive behavior. Use wrapping, horizontal scrolling, or an overflow menu only if consistent with existing UI patterns.

### Selection rules

Exactly zero or one primary action may be active:

* Understand
* Propose
* Plan
* Double-check
* Repeat
* Implement

Clicking an inactive primary action selects it.

Clicking another primary action replaces the current primary selection.

Clicking the active primary action again deselects it.

`Do not edit` is an independent modifier that may be active with:

* no primary action;
* Understand;
* Propose;
* Plan;
* Double-check;
* Repeat.

`Do not edit` cannot coexist with `Implement`.

Selecting `Implement` automatically clears `Do not edit`.

Selecting `Do not edit` while `Implement` is active clears `Implement`.

Do not silently choose one based on button ordering.

### Composer representation

Selecting a control must not populate, prefix, suffix, replace, or otherwise mutate the editable user text.

Represent selected controls as visually distinct chips/tokens inside the composer container but outside the editable text value.

Examples:

* `Plan ×`
* `Double-check ×`
* `Understand ×  Do not edit ×`

The chips must:

* use a deliberate accent treatment consistent with the current theme;
* remain visually distinct from typed text;
* have accessible labels;
* have keyboard focus;
* expose a remove action;
* support mouse and keyboard removal;
* not become selectable or editable text;
* not be copied when the user copies the prompt-field text;
* not affect cursor position, undo history, selection, or text editing;
* remain visible while the user types;
* not consume excessive vertical space.

Clicking the chip’s remove control deselects that action or modifier.

### Repeat availability

`Repeat` means: perform the immediately preceding reasoning task again as a fresh independent pass.

If the current session contains no preceding user/Codex turn to repeat, disable `Repeat` and explain why through the existing tooltip/help pattern:

`Repeat becomes available after Codex has answered a prompt.`

Do not send a meaningless Repeat expansion without preceding context.

### Submission requirements

Use the existing send behavior and validation.

Do not allow an action chip by itself to send an empty user request unless the current product already supports empty commands intentionally. Normally, the visible user prompt must contain non-whitespace text.

At submission time, take an immutable snapshot of:

* visible user prompt;
* selected primary action, if any;
* selected modifiers;
* exact expansion versions used;
* final assembled outbound prompt.

Do not read mutable composer state after asynchronous submission begins.

Assemble the outbound prompt exactly as:

```text
<trimmed visible user prompt>

<primary action expansion, when selected>

<Do not edit expansion, when selected>
```

Use exactly one blank line between sections.

Do not prepend internal labels such as `SYSTEM`, `HIDDEN`, `INSTRUCTION`, or `THE DITCH`.

These expansions are appended user guidance. Do not represent them as system or developer messages and do not attempt to override higher-priority instructions.

If no action or modifier is selected, send the original prompt through the existing path without behavioral change.

After successful submission:

* clear the editable prompt using existing behavior;
* clear selected action and modifier state;
* preserve the submitted action metadata on the turn.

If submission fails before the runtime accepts it:

* preserve the user’s typed text;
* preserve the selected chips;
* show the existing error/retry behavior;
* do not create duplicate turns.

If the current submission pipeline has an acknowledged intermediate state, follow its existing definition of accepted rather than inventing one.

## Transcript behavior

The normal user-message bubble must show only:

* the visible user-authored prompt;
* a small action badge for the selected primary action;
* a small modifier badge for `Do not edit`, when selected.

Do not show the expanded paragraphs inline in the ordinary transcript.

Use the application’s existing badge/chip visual language.

Examples:

```text
[Plan]
Add persistent runtime reconnection.
```

```text
[Double-check] [Do not edit]
Inspect the shutdown proposal for race conditions.
```

Do not misrepresent the hidden expansion as text manually written by the user.

### Full-prompt transparency

Add a secondary `View full prompt sent` action in the existing turn-details, overflow, inspection, or diagnostics surface.

Do not add a prominent button to every chat bubble if the current design uses contextual menus.

The full-prompt view must display:

* original user prompt;
* selected action;
* selected modifiers;
* exact assembled outbound prompt;
* expansion version identifiers when stored.

Make it read-only and copyable.

Clearly label it:

`Full prompt sent to Codex`

This view exists for trust and debugging. It must show what was actually sent for that turn, not regenerate the prompt from the latest templates.

If the current product does not yet have a turn-details or overflow surface, implement the smallest context-menu/dialog solution consistent with the app’s current design.

## Data model

Use names consistent with the current domain. Conceptually, each submitted turn needs:

```text
user_prompt
primary_action_id?
modifier_ids[]
prompt_expansion_version
expanded_prompt
```

Do not force these exact field names if the repository has established naming conventions.

Requirements:

* `user_prompt` is the text displayed in the normal transcript.
* `expanded_prompt` is the immutable text actually sent.
* Action identity is structured metadata, not inferred by parsing text.
* Submitted turns preserve the exact expansion used even if templates change later.
* Draft selection state is separate from submitted-turn metadata.
* Unknown future action IDs must not crash historical transcript rendering.
* Schema changes require a migration if the current persistence layer stores turns durably.
* Do not store duplicate expanded content in several projections without architectural justification.
* Keep everything local according to the current product privacy boundary.

If the current product does not yet persist chat turns, do not build a large persistence subsystem for this task. Extend the existing in-memory/event model cleanly and document the persistence limitation.

## Prompt definitions

Define the expansions centrally as immutable, versioned application/domain data. Do not duplicate paragraph strings across widgets, controllers, tests, and daemon code.

Use stable IDs:

```text
understand
propose
plan
double_check
repeat
implement
do_not_edit
```

Use an explicit template version, initially:

```text
prompt_controls_v1
```

Use these exact expansions.

### Understand

```text
First inspect the request and all relevant available context, including the current code, documentation, constraints, and prior decisions. Explain what you believe the user is trying to achieve, what behavior is expected, which parts of the system are affected, and what assumptions you are making.

Identify material ambiguities, contradictions, missing information, and risks. Ask questions only when the answers would materially change the result. Do not propose a detailed implementation plan or modify anything yet.
```

### Propose

```text
Analyze the request against the current codebase and product requirements, then propose the best solution. Explain the intended behavior, architectural approach, important design decisions, tradeoffs, risks, and why this approach is preferable to the realistic alternatives.

Keep the proposal concrete and proportionate to the problem. Do not produce a step-by-step implementation plan and do not edit or create files yet. Clearly separate verified facts about the current system from assumptions and recommendations.
```

### Plan

```text
Inspect the current implementation and turn the agreed outcome into a concrete, dependency-aware implementation plan. Identify the components and files likely to change, contracts that must remain stable, migrations or compatibility concerns, tests required, failure and recovery behavior, and the order in which work should proceed.

Break the work into bounded, verifiable steps with acceptance criteria for each step. Challenge any part of the proposed solution that conflicts with the existing architecture or introduces unnecessary complexity. Do not edit or create anything yet.
```

### Double-check

```text
Critically audit the latest understanding, proposal, or implementation plan rather than merely restating or defending it. Reinspect the relevant code and requirements, then look specifically for incorrect assumptions, missing dependencies, architectural contradictions, race conditions, unsafe lifecycle behavior, migration problems, security issues, unhandled failure states, redundant work, premature abstraction, weak tests, and requirements that cannot actually be verified.

Report every material issue, explain its consequence, and revise the affected parts of the proposal or plan. If the original result remains correct, justify that conclusion with concrete evidence. Do not edit or create anything yet.
```

### Repeat

```text
Perform the immediately preceding reasoning task again as a fresh, independent pass. Return to the underlying request and relevant source material instead of treating the previous answer as correct. Actively search for a different interpretation, overlooked evidence, unjustified confidence, and a simpler or safer solution.

Compare the new conclusion with the previous one. State what changed, what remained stable, and why. Preserve the preceding step’s restriction against implementation unless the user has explicitly authorized changes.
```

### Implement

```text
Implement the agreed solution in the current codebase. Inspect the affected code before editing, preserve unrelated work, follow applicable repository instructions, and keep the changes limited to the approved scope. Resolve routine implementation details using the existing architecture; stop only when a missing decision would materially change product behavior, safety, or scope.

Add or update meaningful tests, run every applicable formatting, analysis, build, and test command, and fix failures caused by the change. Then review the final diff against the request and acceptance criteria. Report files changed, behavior implemented, commands and exact results, remaining limitations, and anything requiring real-environment verification. Do not claim completion without evidence.
```

### Do not edit

```text
This turn is strictly read-only. You may inspect files, search the repository, run safe non-mutating diagnostic commands, reason about the system, identify problems, and recommend next steps. Do not edit, create, delete, move, format, generate, patch, commit, install, or otherwise mutate files, configuration, dependencies, services, repositories, or external resources.

Do not treat phrases such as “fix,” “build,” or “implement” elsewhere in the request as authorization to make changes during this turn. Return findings and recommendations only, then wait for explicit implementation authorization.
```

## Suggested internal boundary

Adapt this concept to the current architecture rather than copying it blindly:

```text
PromptControlDefinition
- id
- label
- kind: primary | modifier
- templateVersion
- expansion

PromptDraftState
- userText
- primaryActionId?
- modifierIds

SubmittedPrompt
- userPrompt
- primaryActionId?
- modifierIds
- templateVersion
- expandedPrompt
```

Use one pure prompt-assembly function that can be unit tested without Flutter, Rust, PTY, or Codex.

The UI selects structured IDs. The application layer resolves the versioned definitions and constructs the outbound text. The runtime layer receives the already assembled prompt through the established command contract unless the existing architecture has a stronger reason to assemble it elsewhere.

Do not make widgets own the canonical expansion strings.

## Keyboard and accessibility

Follow current keyboard conventions.

At minimum:

* every toolbar button is reachable by keyboard;
* selected state is exposed to accessibility APIs;
* chip remove controls have descriptive labels;
* disabled Repeat explains its state;
* focus remains predictable when selecting or removing chips;
* sending and clearing selections do not strand focus;
* color is not the only indication of selected state;
* light and dark modes remain legible;
* text scaling does not clip labels.

Do not introduce global keyboard shortcuts for all seven actions in this slice unless the application already has a shortcut-management system.

## Tests

Add tests at the lowest appropriate layers.

### Domain/application tests

Cover:

* no control returns the original prompt unchanged;
* each primary action appends the correct exact expansion;
* `Do not edit` works without a primary action;
* each permitted primary action combines with `Do not edit`;
* selecting `Implement` clears `Do not edit`;
* selecting `Do not edit` clears `Implement`;
* only one primary action remains selected;
* whitespace is normalized only at section boundaries;
* visible prompt content is otherwise preserved;
* empty/whitespace-only prompt validation;
* immutable submission snapshot;
* submitted prompt retains the template version;
* historical expanded prompt is not regenerated from changed definitions;
* unknown historical action ID renders safely.

### Widget/UI tests

Cover:

* clicking a button does not change the text controller value;
* selected action appears as a chip;
* selecting a second primary action replaces the first;
* clicking the selected action toggles it off;
* chip removal works;
* `Do not edit` appears independently;
* conflict behavior between `Implement` and `Do not edit`;
* Repeat disabled without preceding turn;
* typed-text cursor and undo state are unaffected;
* failed send preserves text and chips;
* successful send clears text and chips;
* transcript shows original text and badges only;
* transcript does not expose expansion paragraphs;
* full-prompt view shows the exact submitted expansion;
* keyboard navigation and accessibility labels exist.

### Integration tests

Using the existing fake daemon/provider/runtime:

* submit visible text with `Plan`;
* assert the runtime receives visible text plus the exact Plan expansion;
* assert the transcript displays only visible text with a Plan badge;
* assert stored turn metadata contains the immutable assembled prompt;
* simulate submission failure and verify no duplicate turn;
* submit without controls and verify existing behavior is unchanged.

Do not require a paid Codex invocation for automated tests.

## Verification

Run all applicable existing repository checks, including the repository’s documented equivalents of:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
dart format --output=none --set-exit-if-changed .
flutter analyze
flutter test
flutter build macos --debug
```

Use the project’s actual scripts and commands when they differ.

Do not claim checks passed if they could not run. Report unavailable toolchains separately and give exact manual verification steps.

## Manual acceptance journey

Verify on the running macOS app:

1. Type a prompt.
2. Select `Understand`.
3. Confirm an Understand chip appears without changing typed text.
4. Select `Plan`.
5. Confirm Plan replaces Understand.
6. Select `Do not edit`.
7. Confirm both chips appear.
8. Remove Plan.
9. Confirm Do not edit remains.
10. Select Implement.
11. Confirm Do not edit clears.
12. Submit.
13. Confirm the transcript shows the visible prompt and Implement badge.
14. Confirm the composer clears.
15. Inspect `Full prompt sent to Codex`.
16. Confirm it contains the original prompt and exact Implement expansion.
17. Start another prompt, simulate failure, and confirm text and chips remain.
18. Submit a prompt without controls and confirm legacy behavior is unchanged.

## Scope exclusions

Do not add:

* user-editable templates;
* custom shortcut creation;
* keyboard-shortcut settings;
* template synchronization;
* cloud storage;
* analytics;
* provider-specific template variants;
* AI-generated prompt rewriting;
* automatic action selection;
* sticky action selections;
* multiple simultaneous primary actions;
* new mobile or Watch UI;
* unrelated composer redesign;
* a generic workflow engine.

These may be considered separately after this vertical slice is verified.

## Done when

This feature is complete only when:

* the seven controls exist in the current composer design;
* selected controls render as separate chips, not editable text;
* typed text is never populated or mutated by a selection;
* one primary action and the optional valid modifier are enforced;
* exact versioned expansions are centrally defined;
* outbound Codex input contains the correct assembled prompt;
* ordinary transcript rendering hides expansion text;
* turn details can reveal the exact full prompt sent;
* the implementation uses existing architectural boundaries and design tokens;
* no direct runtime path or competing message pipeline is introduced;
* existing submission behavior remains unchanged without controls;
* meaningful automated tests pass;
* the macOS acceptance journey is verified or explicitly left for real-device verification;
* the final diff contains no unrelated refactor.

Finish with:

1. Current architecture inspected.
2. Implementation plan followed.
3. Files changed and why.
4. State and data-model decisions.
5. UI behavior implemented.
6. Prompt assembly behavior.
7. Tests added.
8. Commands run and exact results.
9. Manual verification results.
10. Remaining limitations.
11. Final self-review against every Done condition.
