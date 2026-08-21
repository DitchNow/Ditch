Implement one bounded, production-quality vertical slice in the current The Ditch codebase: rebuild the per-agent chat viewport and scroll lifecycle so it behaves like a modern ChatGPT-style conversation while preserving The Ditch’s existing architecture, brand, message model, runtime boundary, and overall visual language.

The current behavior is unacceptable: opening an agent can begin at old messages, scrolling jumps unpredictably, message updates disturb the viewport, the agent-list and transcript scroll domains interfere, and long conversations do not behave like a stable modern chat.

Do not redesign the entire application. Do not replace the daemon, runtime, session model, navigation, composer, state-management framework, or design system. Fix the chat viewport comprehensively within the existing product.

## Start by inspecting, not editing

Read all applicable:

* `AGENTS.md`
* product and architecture documents
* design-system documentation
* `CHAT_UI_STRUCTURE.md`
* current Flutter chat implementation
* current message/session models
* message-pagination implementation
* runtime event handling
* native macOS composer implementation
* existing tests

At minimum, inspect the current equivalents of:

* `apps/macos/lib/main.dart`
* `apps/macos/lib/design_system/ditch_theme.dart`
* `apps/macos/macos/Runner/MainFlutterWindow.swift`

Identify:

1. Every scrollable ancestor and descendant around the agent list, expanded agent panel, transcript, and native composer.
2. Every `ScrollController`, its owner, lifetime, and associated agent.
3. Every `ensureVisible`, `jumpTo`, `animateTo`, reversed-list assumption, post-frame callback, and scroll listener.
4. How an agent changes between collapsed, expanded, focused, deleted, and reopened states.
5. How initial messages, live messages, streaming updates, system/tool messages, and older pages enter the transcript.
6. Whether message identity is stable across hydration, deduplication, pagination, and runtime updates.
7. Which existing design tokens and reusable components must be retained.
8. Which existing tests cover scrolling and where coverage is absent.

Before modifying code, write a short implementation plan explaining the root causes found and the smallest architecture that will satisfy every acceptance criterion below.

Proceed automatically after the plan unless a requirement conflicts with a frozen architectural contract. If there is a material conflict, stop and explain it rather than silently rewriting the architecture.

## Non-negotiable architectural contract

There must be exactly one conversation-scroll owner for an expanded or focused agent.

The intended hierarchy is:

```text
Agent chat surface
  fixed agent/chat header
  Expanded
    bounded transcript viewport
      one conversation scrollable
      viewport-relative new-messages control
  fixed status/composer footer
```

Requirements:

* The transcript is the only scrollable responsible for conversation messages.
* The composer stays pinned below the transcript.
* The chat header stays pinned above the transcript.
* The native prompt text view may scroll its own text internally after overflow, but it must not become the conversation-scroll owner.
* The collapsed-agent list may have its own list controller only while displaying multiple collapsed agents.
* An expanded or focused chat must not remain embedded inside the collapsed-agent `ListView`.
* Do not lay a long conversation out as an unbounded `Column`.
* Do not rely on an ancestor page scroll to move through chat messages.
* Do not use `Scrollable.ensureVisible` in a way that can select the transcript scrollable instead of the agent-list scrollable.
* Do not create nested vertical transcript scroll views.
* Do not allow rebuilding the composer, header, inspector, or agent list to recreate the active transcript controller.
* Do not let UI scrolling changes bypass or modify the `ditchd` runtime boundary.

Remove or isolate the current embedded conversation path if it can render an active or long transcript as a plain `Column`. Static previews may use a separate non-interactive representation, but every active conversation must use the bounded transcript viewport.

## Choose one clear scroll coordinate model

The current implementation uses a reversed `ListView`, where offset `0` is the visual bottom, and manually reverses message indices. That is error-prone and has already caused confusing behavior.

Prefer changing the active transcript to a normal chronological scroll model unless repository inspection proves that retaining the reversed model is materially safer:

```text
oldest message at index 0
latest message at final index
visual top = minScrollExtent
visual bottom = maxScrollExtent
distance from bottom = extentAfter
older-history loading near visual top = extentBefore threshold
```

Do not retain a reversed list merely to minimize the diff.

If you retain `reverse: true`, centralize all bottom/top calculations behind named helpers and prove every behavior with tests. No code outside that abstraction may directly assume whether the bottom is `minScrollExtent` or `maxScrollExtent`.

Do not mix normal-list and reversed-list assumptions.

## Explicit viewport state machine

Implement a small, testable conversation viewport state machine. Adapt naming to current conventions, but model these states explicitly:

```text
initializing
following
detached
```

### Initializing

The transcript is hydrating and has not yet established its first stable viewport.

During initialization:

* do not animate through old messages;
* do not briefly render the top and visibly fly to the bottom;
* wait until the initial message page and first valid layout dimensions are available;
* position directly at the latest message;
* perform this initial positioning exactly once for that opening lifecycle;
* do not count initial hydration as unseen messages.

### Following

The user is at or close to the latest message.

While following:

* appended messages remain visible;
* streaming content growth remains pinned to the latest content;
* discrete new messages may use a short smooth arrival animation;
* token-by-token or rapid updates must be coalesced rather than queueing hundreds of animations;
* layout changes in the footer, window, status strip, or inspector must preserve the bottom relationship;
* the new-message counter is zero and its control is hidden.

### Detached

The user deliberately scrolled away from the latest message.

While detached:

* never pull them back to the bottom;
* preserve their visible reading position;
* appended messages accumulate an unseen-message count;
* streaming updates must not cause repeated viewport jumps;
* older-page loading must preserve the exact visible anchor;
* window resizing should preserve the logical reading location rather than resetting to either edge.

Transition from `detached` to `following` only when:

* the user scrolls within the near-bottom threshold; or
* the user clicks the new-messages control; or
* an explicit existing product action requests the latest message.

Do not infer following merely because a rebuild occurred.

## Opening, closing, focusing, and switching agents

Implement these exact semantics.

### Opening an agent row

Whenever a collapsed agent row is expanded into its chat:

* show the latest message immediately;
* start at the visual bottom;
* do not show the first/oldest message first;
* do not animate from top to bottom;
* do not allow the agent-list controller to fight the transcript controller;
* focus the existing composer only after the viewport is established and only if current UX already expects composer focus.

### First hydration

If messages must load asynchronously:

* render a stable loading state;
* hydrate the initial page;
* lay it out;
* jump directly to the latest content before exposing a misleading top position where practical;
* do not create a replacement `ScrollController` when hydration completes.

### Focus/enlarge mode

Switching the same already-open agent between normal expanded and focused/enlarged modes must preserve its current logical transcript position.

Do not reset to the top or bottom merely because the layout mode changed.

If the layout reconstruction makes direct controller preservation unsafe, capture and restore a stable anchor using message identity and relative viewport offset.

### Collapse and reopen

Collapsing an agent and later opening it again should intentionally open at the latest message, as requested.

This is different from switching the same open conversation between normal and focused modes.

### Switching agents

Each agent must have isolated viewport state.

* No controller may drive two transcripts.
* Agent A’s position must not be applied to Agent B.
* Opening Agent B starts at Agent B’s latest message.
* Returning to a still-open/focused Agent A may preserve its position according to the current navigation lifecycle.
* Reopening Agent A after a deliberate collapse starts at the latest message.
* Deleting a session disposes its viewport resources.
* Removed agents must not leave listeners, controllers, timers, or post-frame callbacks alive.

## Near-bottom detection

Use actual scroll metrics rather than assumptions about pixel direction.

For a normal chronological list, near-bottom should be based on:

```text
position.extentAfter <= threshold
```

Use a centralized threshold appropriate to the interface, initially around 72 logical pixels unless testing supports another value.

Near-bottom behavior must remain correct when:

* the window is resized;
* the inspector opens or closes;
* the composer controls wrap;
* the thinking strip appears or disappears;
* the transcript contains large code blocks;
* message height changes after rendering;
* the scrollbar is dragged;
* the user uses mouse wheel, trackpad, Page Up/Down, Home/End, or keyboard scrolling.

## New-messages control

When the user is detached and one or more new conversation items arrive, show a floating viewport-relative control:

```text
↓ 1 new message
↓ 3 new messages
```

Use correct singular/plural wording.

The count should represent new conversation items, not rebuilds, token chunks, frames, or changes to an already-counted message.

Define counting behavior clearly:

* a newly appended user or assistant message counts once;
* a newly appended grouped tool/activity item counts according to the final visible conversation-item model, not every raw event;
* additional streaming text added to an already-counted assistant message does not repeatedly increment the count;
* initial hydration does not count;
* loading older history does not count;
* deduplicated/replayed runtime events do not count;
* replacing a message with the same stable identity does not count.

Place the control inside the transcript viewport’s `Stack`, anchored relative to the transcript itself—not with a hard-coded offset based on an assumed footer height.

Recommended position:

```text
horizontal center
16 logical pixels above the transcript viewport’s bottom edge
```

Adapt to the current visual system and avoid covering selected text or critical content.

The control must:

* use The Ditch’s existing accent color and surface/border/shadow tokens;
* work in light and dark themes;
* include a downward arrow;
* use compact modern typography;
* have hover, pressed, focus, and disabled states consistent with existing controls;
* be keyboard accessible;
* expose an accessibility label containing the count;
* not rely on color alone;
* animate into and out of view subtly;
* not shift transcript layout when appearing.

Clicking it must:

1. enter `following`;
2. clear the unseen count;
3. smoothly move to the latest content;
4. remain pinned as content finishes rendering;
5. avoid producing a second jump after animation completion.

If the distance is extremely large, use a bounded-duration strategy rather than animating slowly through hundreds of screens. The interaction should feel immediate but spatially understandable.

Use an appropriate curve such as the application’s existing motion token or an ease-out curve. Do not scatter duration and curve constants.

## New-message arrival behavior

When following:

* keep the latest message visible;
* do not queue overlapping `animateTo` operations;
* coalesce rapid runtime events into at most one scheduled viewport correction per frame;
* cancel or supersede stale scheduled movement;
* use direct pinned correction for high-frequency streaming when animation would cause oscillation;
* use short animation for discrete message arrival only when it improves perception.

When detached:

* do not call `animateTo` or `jumpTo` because a new message arrived;
* preserve reading position;
* update the unseen count and button only.

If the user begins scrolling during an automatic animation:

* user input wins;
* cancel or stop automatic following;
* transition to detached if they move away from the bottom.

Do not fight the user.

## Stable message identity

Every rendered conversation item must have a stable identity.

Use the existing persisted sequence or message ID where available.

For live unsequenced events, use an existing stable runtime/event identity. If none exists, introduce the smallest local identity mechanism consistent with the current model.

Do not key message rows by:

* list index;
* current text;
* role plus text;
* build timestamp;
* regenerated random key during rendering.

Grouped system/tool activity must also receive stable identity.

Stable identity must survive:

* runtime event updates;
* streaming content updates;
* initial hydration;
* pagination;
* deduplication;
* focus/enlarge transitions;
* rebuilds.

Do not broadly redesign persistence unless current message identity makes correctness impossible. If a schema change is required, add an explicit migration and preserve existing data.

## Anchored older-history loading

Older history is loaded at the visual top.

Trigger older-page loading only when:

* the viewport is near the visual top;
* `hasOlderMessages` is true;
* no older-page request is already active.

Prevent duplicate concurrent requests.

When older items are prepended:

* preserve the currently visible message and its relative pixel offset;
* do not jump to the newly inserted oldest message;
* do not jump to the latest message;
* do not change following/detached state incorrectly;
* do not increment unseen-message count.

Use one of these robust strategies:

1. Stable visible-item anchor plus relative offset restoration.
2. Carefully measured pre/post content-extent delta with tests proving variable-height messages.
3. An existing repository abstraction that provides equivalent anchored insertion guarantees.

Do not use a guessed fixed row height. Messages have variable height.

Show the older-history loading affordance at the actual visual top. Do not implement its location using reversed-list assumptions after moving to a normal chronology model.

If pagination fails:

* preserve the viewport;
* show a small retry affordance at the top;
* do not destroy already loaded messages;
* do not reset the transcript.

## Layout stability

The transcript must remain stable when:

* the main window changes size;
* the project sidebar changes width;
* the inspector opens, closes, or resizes;
* the agent switches between normal and focused modes;
* the composer’s control rows wrap;
* the thinking/status strip appears or disappears;
* an error/disabled message appears above the composer;
* the native composer’s internal text scroll changes;
* a message expands due to streaming;
* code blocks or long lines render;
* a scrollbar appears or disappears.

The chat header and composer must remain pinned.

Do not position transcript overlays using the total footer’s assumed height.

Avoid unnecessary relayout of the full transcript when only the composer or status changes.

## Native composer interaction

Preserve the native macOS composer behavior:

* fixed Flutter height around the existing 72-pixel contract;
* internal native text scrolling after overflow;
* Enter submits;
* Shift+Enter inserts a newline;
* current focus/enlarge/escape behavior;
* current colors and font.

Do not rewrite the native composer as part of this task unless it is proven to be the root cause of the transcript problem.

Investigate trackpad/wheel events over the native `NSScrollView`.

Expected behavior:

* when the pointer is over overflowing composer text, the native composer may scroll its own text;
* transcript scrolling outside the composer remains smooth;
* event handling must not cause both composer and transcript to jump;
* do not use unsupported event interception hacks without real macOS verification.

Document any unavoidable platform-view limitation.

## Modern visual treatment

Keep The Ditch’s existing brand colors and design language intact.

Do not make the chat look like a generic clone of ChatGPT, Claude, Slack, or Discord. Copy interaction quality, not branding.

Preserve and reuse:

* current background and surface colors;
* existing user/assistant/tool role colors;
* existing radii;
* spacing scale;
* typography;
* border and divider tokens;
* current icon family;
* light/dark behavior.

Improve only the active chat surface where necessary:

* consistent readable message width;
* clear but restrained separation between user, assistant, system, and activity content;
* stable vertical rhythm;
* no unnecessary boxes around every element;
* comfortable code-block and long-response reading;
* subtle scrollbar;
* pinned composer;
* unobtrusive new-message control;
* clean loading, empty, error, and history-boundary states.

Do not change product-wide navigation, agent headers, inspector styling, or toolbar branding unless a tiny adjustment is required to make the bounded chat viewport work.

Do not hardcode new colors when an equivalent theme token exists.

If a new semantic token is genuinely needed, add it to the existing design system and use it consistently.

## Performance requirements

The transcript must remain responsive with:

* 1,000 loaded conversation items;
* long Markdown responses;
* large code blocks;
* rapid tool events;
* token-like streaming updates;
* multiple agent sessions;
* frequent rebuilds elsewhere in the command center.

Requirements:

* use lazy list/sliver construction;
* do not render all messages in a `Column`;
* do not perform expensive grouping or Markdown transformation unnecessarily on every scroll frame;
* avoid broad `setState` calls that rebuild the entire command center for scroll-position changes;
* isolate transient viewport state from unrelated application state;
* dispose listeners and controllers correctly;
* do not schedule unbounded post-frame callbacks;
* do not queue animations;
* avoid logging per-pixel scroll events in production;
* do not load every historical page merely to reach the latest message.

Do not add a third-party scrolling dependency unless Flutter primitives and existing project dependencies cannot meet the requirements. If proposing a dependency, document why it is necessary, its maintenance status, platform implications, and what complexity it removes. Prefer no new dependency.

## Suggested separation of responsibility

Adapt this to existing conventions rather than copying names blindly:

```text
ConversationViewportController
  owns:
    ScrollController
    follow mode
    unseen visible-item identities
    initial-position lifecycle
    scheduled/coalesced follow correction
    pagination anchor
    per-agent disposal

ConversationTranscript
  owns:
    lazy rendering
    stable item keys
    scroll notifications
    top history loader
    viewport-relative new-message control

AgentChatPanel
  owns:
    fixed toolbar
    bounded transcript slot
    fixed footer/composer
```

Do not put all new scrolling logic back into the monolithic command-center build method if the repository supports focused extraction.

Do not perform a broad main.dart refactor unrelated to the chat viewport.

## Remove conflicting behavior

Find and remove or constrain behavior that fights the new contract, including:

* opening an agent and remaining at the oldest message;
* `ensureVisible` calls that act on the wrong scrollable;
* automatic scroll on every rebuild;
* multiple initial-bottom callbacks;
* message-count logic that treats pagination as new messages;
* hard-coded `bottom: 112` positioning for the new-items control;
* reversed-list calculations mixed with normal chronology;
* controllers recreated during focus/layout changes;
* scroll listeners attached more than once;
* embedded active chats rendered in an unbounded `Column`;
* auto-follow continuing after user scroll input;
* older-page insertion changing the reading position;
* scroll state from one agent leaking into another;
* delayed callbacks operating on disposed sessions.

Do not leave the old and new scroll lifecycle active simultaneously.

## Automated tests

Add meaningful tests. Compilation is not evidence that scrolling works.

### Pure controller/state tests

Cover:

* initializing transitions to following after first stable layout;
* initial hydration does not increment unseen count;
* appended message while following remains following;
* appended message while detached increments unseen count once;
* streaming updates to the same identity do not repeatedly increment;
* grouped activity counts according to rendered-item identity;
* reaching the near-bottom threshold clears unseen count;
* clicking new messages enters following and clears count;
* pagination insertion does not increment unseen count;
* focus-mode change preserves state;
* deliberate collapse/reopen requests latest-message initialization;
* deleted agent disposes state;
* stale callbacks are ignored after disposal.

### Widget tests

Create deterministic variable-height messages.

Cover:

* opening an agent places the viewport at the latest message;
* oldest message is not initially shown for a long conversation;
* the composer remains visible while the transcript scrolls;
* header remains visible while the transcript scrolls;
* user scroll upward enters detached state;
* new messages do not change detached reading position;
* `↓ X new messages` appears with correct count;
* clicking it smoothly reaches the latest content;
* the control disappears at the bottom;
* older-page insertion preserves the visible anchor;
* pagination failure preserves position and exposes retry;
* window resize while following remains at bottom;
* window resize while detached preserves reading location;
* focused/normal transition preserves current position;
* collapse/reopen starts at latest;
* switching Agent A to Agent B does not leak position;
* rapid updates do not queue animations or throw exceptions;
* removal during a scheduled callback does not use a disposed controller;
* no overflow exceptions occur at narrow and short window sizes.

### Integration tests

Use the existing fake runtime/daemon boundary.

Cover:

1. Hydrate an agent with enough messages to exceed several screens.
2. Open it and verify the latest message is visible.
3. Stream several updates while following.
4. Scroll up.
5. Append three visible conversation items.
6. Verify the reading position remains stable and the button shows three.
7. Click the button and verify arrival at latest.
8. Load an older page and verify anchor stability.
9. Switch agents and verify isolated viewport behavior.
10. Change focus mode and window dimensions.
11. Confirm no duplicate message events or scroll exceptions.

### Golden/design tests

Use existing golden-test infrastructure where available.

Cover:

* light theme;
* dark theme;
* no unseen messages;
* one unseen message;
* multiple unseen messages;
* older-history loading;
* pagination error;
* long assistant response;
* narrow supported window.

Do not create a new golden framework if the repository has none; document manual visual verification instead.

## Performance verification

Build a development fixture containing:

* at least 1,000 variable-height conversation items;
* long code blocks;
* grouped tool activity;
* rapid message updates.

Verify in profile mode on macOS where available:

* scrolling remains responsive;
* no obvious repeated full-screen rebuilds;
* no growing queue of animations or post-frame callbacks;
* memory does not grow continuously while switching agents;
* controllers/listeners are released after agent deletion.

Report measured observations honestly. Do not invent frame-rate numbers.

## Manual macOS acceptance test

Run and record this exact journey:

1. Start The Ditch with an agent containing a long conversation.
2. Open the agent row.
3. Confirm the latest message appears immediately without visible top-to-bottom travel.
4. Scroll slowly with a trackpad.
5. Scroll rapidly with a trackpad.
6. Drag the scrollbar thumb.
7. Scroll upward several screens.
8. Allow or inject new agent messages.
9. Confirm the viewport does not move.
10. Confirm `↓ X new messages` appears and counts correctly.
11. Click it.
12. Confirm smooth, bounded movement to the latest message.
13. Scroll to the oldest loaded message.
14. Trigger older-history loading.
15. Confirm the same message remains under the pointer at approximately the same offset.
16. Resize the window vertically and horizontally.
17. Open and close the inspector.
18. Toggle focused/enlarged mode.
19. Switch between two agents.
20. Collapse and reopen the first agent.
21. Confirm reopening shows its latest message.
22. Type enough composer text to activate its internal scrolling.
23. Verify transcript scrolling still behaves correctly outside the composer.
24. Run rapid simulated message updates.
25. Confirm no jumps, overflow warnings, exceptions, or stuck new-message control.

Capture a short screen recording after implementation for comparison if repository workflow allows it.

## Verification commands

Run all applicable existing repository checks, using project scripts when available:

```text
dart format --output=none --set-exit-if-changed .
flutter analyze
flutter test
flutter build macos --debug
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Do not change Rust code merely to create a Rust diff if the feature is entirely within Flutter.

Do not claim a macOS build or manual test passed if Flutter, Xcode, or a real macOS environment is unavailable.

## Scope exclusions

Do not implement:

* daemon or IPC redesign;
* message-protocol redesign unless stable identity is impossible otherwise;
* new agent orchestration;
* prompt-shortcut changes;
* mobile or Watch UI;
* cloud synchronization;
* new Markdown editor;
* complete composer rewrite;
* product-wide redesign;
* new navigation;
* unrelated database changes;
* full accessibility redesign outside this chat surface;
* arbitrary performance “optimizations” without evidence;
* a generic reusable chat SDK.

## Done when

This task is complete only when:

* every expanded/focused agent has exactly one bounded transcript scrollable;
* opening or reopening a collapsed agent shows the latest message;
* the oldest message never appears as the unintended initial position;
* the composer and header remain pinned;
* auto-follow occurs only while near the bottom;
* user scrolling immediately defeats auto-follow;
* detached reading position survives incoming messages;
* `↓ X new messages` appears and counts visible conversation items correctly;
* clicking it smoothly reaches and follows the latest content;
* older-history loading preserves a stable visible anchor;
* focus changes, resizing, inspector changes, and agent switching do not cause arbitrary jumps;
* per-agent viewport state cannot leak;
* rapid updates do not queue scroll animations;
* active conversations never render as an unbounded message `Column`;
* the implementation preserves The Ditch’s existing brand and design system;
* existing runtime and composer behavior remain intact;
* automated tests cover the critical lifecycle;
* all available verification commands pass;
* manual macOS behavior is verified or explicitly reported as unverified;
* the final diff contains no unrelated architectural or visual refactor.

Finish with:

1. Root causes found.
2. Scroll architecture chosen and why.
3. Whether the reversed list was removed or retained, with justification.
4. Files changed.
5. State-machine behavior.
6. Initial-open and per-agent lifecycle behavior.
7. New-message counting rules.
8. Pagination anchoring method.
9. Performance considerations.
10. Automated tests and exact results.
11. Manual macOS verification results.
12. Remaining platform limitations.
13. Final self-review against every Done condition.
