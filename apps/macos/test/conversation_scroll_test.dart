import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/widgets/inline_approval.dart';

AgentChatMessage message(
  String id,
  ChatMessageRole role, {
  String? text,
  ChatMessageArrival arrival = ChatMessageArrival.history,
}) => AgentChatMessage(
  identity: id,
  role: role,
  text: text ?? 'Message $id with enough text to read.',
  createdAt: DateTime(2026),
  arrival: arrival,
);

void main() {
  test(
    'partial tool turns retain identity when history and live tools join',
    () {
      final late = message('late', ChatMessageRole.tool);
      final first = buildConversationItems([late], isWorking: true);
      final merged = buildConversationItems(
        [
          message('user', ChatMessageRole.user),
          message('early', ChatMessageRole.tool),
          late,
          message(
            'new',
            ChatMessageRole.tool,
            arrival: ChatMessageArrival.live,
          ),
        ],
        isWorking: true,
        previousItems: first,
      );
      expect(merged.last.identity, first.single.identity);
      expect(merged.last.toolMessages.map((m) => m.identity), [
        'early',
        'late',
        'new',
      ]);
    },
  );

  testWidgets('older page across a tool group preserves the reading position', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    addTearDown(viewport.dispose);
    final messages = [
      message('late-tool', ChatMessageRole.tool),
      for (var i = 0; i < 30; i++)
        message('answer-$i', ChatMessageRole.assistant),
    ];
    late StateSetter rebuild;
    var loads = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return ConversationTranscript(
                messages: messages,
                viewport: viewport,
                hasOlderMessages: true,
                onLoadOlder: () => loads++,
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, 400));
    await tester.pumpAndSettle();
    viewport.scrollController.jumpTo(
      viewport.scrollController.position.maxScrollExtent,
    );
    await tester.pumpAndSettle();
    final anchor = find.text('Message answer-0 with enough text to read.');
    final before = tester.getTopLeft(anchor).dy;
    final baselineLoads = loads;
    rebuild(() {
      messages.insertAll(0, [
        for (var i = 0; i < 20; i++) ...[
          message('older-user-$i', ChatMessageRole.user),
          message('older-answer-$i', ChatMessageRole.assistant),
        ],
        message('turn-user', ChatMessageRole.user),
        message('early-tool', ChatMessageRole.tool),
      ]);
    });
    await tester.pumpAndSettle();
    expect(viewport.unseenCount, 0);
    expect(tester.getTopLeft(anchor).dy, closeTo(before, 1));
    expect(loads, baselineLoads);
    expect(tester.takeException(), isNull);
  });

  testWidgets('one mouse wheel event detaches before the next update', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    addTearDown(viewport.dispose);
    final messages = [
      for (var i = 0; i < 30; i++)
        message('answer-$i', ChatMessageRole.assistant),
    ];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ConversationTranscript(messages: messages, viewport: viewport),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.sendEventToBinding(
      PointerScrollEvent(
        position: tester.getCenter(find.byType(ListView)),
        scrollDelta: const Offset(0, -40),
      ),
    );
    await tester.pump();
    expect(viewport.mode, ConversationViewportMode.detached);
    expect(viewport.scrollController.offset, greaterThan(0));
  });

  testWidgets('mixed history and live arrivals preserve a visible message', (
    tester,
  ) async {
    final harness = _TranscriptHarness();
    await harness.mount(tester);
    await harness.readOlder(tester);
    final anchor = harness.visibleAnchor(tester);
    final y = tester.getTopLeft(anchor).dy;
    harness.rebuild(() {
      harness.messages.insertAll(0, [
        for (var i = 0; i < 100; i++)
          message('history-$i', ChatMessageRole.assistant),
      ]);
      harness.messages.add(
        message(
          'live',
          ChatMessageRole.assistant,
          text: List.filled(15, 'A new live response.').join('\n'),
          arrival: ChatMessageArrival.live,
        ),
      );
    });
    await tester.pumpAndSettle();
    expect(harness.viewport.unseenCount, 1);
    expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
    expect(harness.loads, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'regrouping relocates a tool row without moving the answer being read',
    (tester) async {
      final harness = _TranscriptHarness();
      harness.messages.insert(1, message('late-tool', ChatMessageRole.tool));
      await harness.mount(tester);
      await harness.readOlder(tester);
      harness.viewport.scrollController.jumpTo(
        harness.viewport.scrollController.position.maxScrollExtent,
      );
      await tester.pumpAndSettle();
      final anchor = find.byKey(const ValueKey('answer-0'));
      final y = tester.getTopLeft(anchor).dy;
      harness.rebuild(
        () => harness.messages.insertAll(0, [
          message('old-user', ChatMessageRole.user),
          message('early-tool', ChatMessageRole.tool),
        ]),
      );
      await tester.pumpAndSettle();
      expect(harness.viewport.unseenCount, 0);
      expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
      expect(harness.loads, 0);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('loading state and response do not chain pagination requests', (
    tester,
  ) async {
    final harness = _TranscriptHarness();
    await harness.mount(tester);
    await harness.readOlder(tester);
    await harness.nearOldest(tester);
    expect(harness.loads, 0); // A programmatic jump is not a history gesture.
    await tester.sendEventToBinding(
      PointerScrollEvent(
        position: tester.getCenter(find.byType(ListView)),
        scrollDelta: const Offset(0, -20),
      ),
    );
    await tester.pump();
    expect(harness.loads, 1);
    harness.rebuild(() => harness.loading = true);
    await tester.pump();
    harness.rebuild(() {
      harness.loading = false;
      harness.messages.insertAll(0, [
        for (var i = 0; i < 100; i++)
          message('older-$i', ChatMessageRole.assistant),
      ]);
    });
    await tester.pumpAndSettle();
    expect(harness.loads, 1);
    expect(harness.viewport.unseenCount, 0);
    await harness.nearOldest(tester);
    await tester.sendEventToBinding(
      PointerScrollEvent(
        position: tester.getCenter(find.byType(ListView)),
        scrollDelta: const Offset(0, -20),
      ),
    );
    await tester.pump();
    expect(harness.loads, 2); // A fresh user movement can load the next page.
  });

  testWidgets(
    'a pending response preserves the users latest reading position',
    (tester) async {
      final harness = _TranscriptHarness();
      await harness.mount(tester);
      await harness.readOlder(tester);
      harness.rebuild(() => harness.loading = true);
      await tester.pump();
      await tester.drag(find.byType(ListView), const Offset(0, 180));
      await tester.pumpAndSettle();
      final anchor = harness.visibleAnchor(tester);
      final y = tester.getTopLeft(anchor).dy;
      harness.rebuild(() {
        harness.loading = false;
        harness.messages.insert(0, message('older', ChatMessageRole.assistant));
      });
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
      expect(harness.viewport.unseenCount, 0);
    },
  );

  testWidgets('streaming growth preserves the anchor and counts the row once', (
    tester,
  ) async {
    final harness = _TranscriptHarness();
    await harness.mount(tester);
    await harness.readOlder(tester);
    final anchor = harness.visibleAnchor(tester);
    final y = tester.getTopLeft(anchor).dy;
    for (var i = 1; i <= 4; i++) {
      harness.rebuild(() {
        harness.messages.removeWhere((m) => m.identity == 'stream');
        harness.messages.add(
          message(
            'stream',
            ChatMessageRole.assistant,
            text: List.filled(i * 5, 'Streaming text.').join('\n'),
            arrival: ChatMessageArrival.live,
          ),
        );
      });
      await tester.pumpAndSettle();
      expect(harness.viewport.unseenCount, 1);
      expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
    }
    harness.rebuild(() {
      harness.messages.removeLast();
      harness.messages.add(
        message(
          'final',
          ChatMessageRole.assistant,
          arrival: ChatMessageArrival.live,
        ),
      );
    });
    await tester.pumpAndSettle();
    expect(harness.viewport.unseenCount, 1);
    expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
    await tester.tap(find.byKey(const Key('conversation-new-messages')));
    await tester.pumpAndSettle();
    expect(harness.viewport.scrollController.offset, 0);
    expect(harness.viewport.unseenCount, 0);
  });

  testWidgets('a live update during a drag does not cancel the drag', (
    tester,
  ) async {
    final harness = _TranscriptHarness();
    await harness.mount(tester);
    final gesture = await tester.startGesture(
      tester.getCenter(find.byType(ListView)),
    );
    await gesture.moveBy(const Offset(0, 25));
    await tester.pump();
    await gesture.moveBy(const Offset(0, 160));
    await tester.pump();
    expect(harness.viewport.isDetached, isTrue);
    final anchor = harness.visibleAnchor(tester);
    final y = tester.getTopLeft(anchor).dy;
    harness.rebuild(
      () => harness.messages.add(
        message(
          'live',
          ChatMessageRole.assistant,
          text: 'A live message\nwith extra height\nwhile dragging.',
          arrival: ChatMessageArrival.live,
        ),
      ),
    );
    await tester.pump();
    expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
    expect(
      harness.viewport.scrollController.position.isScrollingNotifier.value,
      isTrue,
    );
    await gesture.moveBy(const Offset(0, 50));
    await tester.pump();
    expect(tester.getTopLeft(anchor).dy, closeTo(y + 50, 1));
    await gesture.up();
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'expanded tool history retains the visible action and expanded state',
    (tester) async {
      final harness = _TranscriptHarness();
      harness.working = true;
      harness.messages.insertAll(0, [
        for (var i = 0; i < 15; i++)
          message(
            'tool-$i',
            ChatMessageRole.tool,
            text: 'Action $i\nOutput line one\nOutput line two',
          ),
      ]);
      await harness.mount(tester);
      await harness.readOlder(tester);
      await harness.nearOldest(tester);
      // Move down into the expanded group, leaving its heading above the viewport.
      harness.viewport.scrollController.jumpTo(
        harness.viewport.scrollController.offset - 400,
      );
      await tester.pumpAndSettle();
      final bounds = tester.getRect(find.byType(ListView));
      final anchor =
          List.generate(
            15,
            (i) => find.text('Action $i\nOutput line one\nOutput line two'),
          ).firstWhere(
            (finder) =>
                finder.evaluate().isNotEmpty &&
                bounds.contains(tester.getCenter(finder)),
          );
      final y = tester.getTopLeft(anchor).dy;
      harness.rebuild(
        () => harness.messages.insertAll(0, [
          message('user', ChatMessageRole.user),
          for (var i = 0; i < 10; i++)
            message(
              'older-tool-$i',
              ChatMessageRole.tool,
              text: 'Earlier action $i\nOlder output',
            ),
        ]),
      );
      await tester.pumpAndSettle();
      expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
      expect(harness.viewport.unseenCount, 0);
      expect(harness.loads, 0);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('finishing a tool group preserves a visible answer', (
    tester,
  ) async {
    final harness = _TranscriptHarness();
    harness.working = true;
    harness.messages.insert(
      0,
      message(
        'tool',
        ChatMessageRole.tool,
        text: List.filled(20, 'Tool output').join('\n'),
      ),
    );
    await harness.mount(tester);
    await harness.readOlder(tester);
    final anchor = harness.visibleAnchor(tester);
    final y = tester.getTopLeft(anchor).dy;
    harness.rebuild(() => harness.working = false);
    await tester.pumpAndSettle();
    expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
    expect(harness.viewport.unseenCount, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('approval insertion preserves the reading position', (
    tester,
  ) async {
    final harness = _TranscriptHarness();
    await harness.mount(tester);
    await harness.readOlder(tester);
    final anchor = harness.visibleAnchor(tester);
    final y = tester.getTopLeft(anchor).dy;
    harness.rebuild(
      () => harness.approvals.add(
        InlineApproval(
          request: {
            'id': 'approval',
            'summary': 'Run tests?',
            'created_at': '2026-02-01T00:00:00Z',
          },
          respond: (_) async {},
          refresh: () async {},
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
    expect(harness.viewport.unseenCount, 1);
    expect(harness.loads, 0);
  });

  testWidgets('queued follow is cancelled when the reader scrolls away', (
    tester,
  ) async {
    final harness = _TranscriptHarness();
    await harness.mount(tester);
    harness.viewport.attach(); // Queues a follow callback for the next frame.
    await tester.sendEventToBinding(
      PointerScrollEvent(
        position: tester.getCenter(find.byType(ListView)),
        scrollDelta: const Offset(0, -40),
      ),
    );
    await tester.pump();
    expect(harness.viewport.isDetached, isTrue);
    expect(harness.viewport.scrollController.offset, 40);
  });

  testWidgets(
    'reopening before the positioning frame does not lose initialization',
    (tester) async {
      final harness = _TranscriptHarness();
      await harness.mount(tester);
      harness.viewport.beginOpening();
      harness.viewport.beginOpening();
      await tester.pump();
      expect(harness.viewport.mode, ConversationViewportMode.following);
      expect(harness.viewport.scrollController.offset, 0);
    },
  );

  testWidgets(
    'switching conversations drops old anchors and pending arrivals',
    (tester) async {
      final harness = _TranscriptHarness();
      await harness.mount(tester);
      await harness.readOlder(tester);
      final old = harness.viewport;
      harness.rebuild(() {
        harness.messages.add(
          message(
            'unseen',
            ChatMessageRole.assistant,
            arrival: ChatMessageArrival.live,
          ),
        );
        harness.viewport = ConversationViewportController()..beginOpening();
        harness.messages
          ..clear()
          ..addAll([
            for (var i = 0; i < 20; i++)
              message('other-$i', ChatMessageRole.assistant),
          ]);
      });
      addTearDown(harness.viewport.dispose);
      await tester.pumpAndSettle();
      expect(old.scrollController.hasClients, isFalse);
      expect(harness.viewport.unseenCount, 0);
      expect(harness.viewport.scrollController.offset, 0);
      expect(harness.viewport.mode, ConversationViewportMode.following);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('macOS momentum continues after a live message changes layout', (
    tester,
  ) async {
    final harness = _TranscriptHarness()..platform = TargetPlatform.macOS;
    await harness.mount(tester);
    await tester.fling(find.byType(ListView), const Offset(0, 240), 900);
    await tester.pump(const Duration(milliseconds: 16));
    expect(harness.viewport.isDetached, isTrue);
    expect(
      harness.viewport.scrollController.position.isScrollingNotifier.value,
      isTrue,
    );
    final anchor = harness.visibleAnchor(tester);
    final y = tester.getTopLeft(anchor).dy;
    harness.rebuild(
      () => harness.messages.add(
        message(
          'live-fling',
          ChatMessageRole.assistant,
          text: 'New output\nwhile scrolling\nwith momentum',
          arrival: ChatMessageArrival.live,
        ),
      ),
    );
    await tester.pump();
    expect(tester.getTopLeft(anchor).dy, closeTo(y, 1));
    expect(
      harness.viewport.scrollController.position.isScrollingNotifier.value,
      isTrue,
    );
    await tester.pump(const Duration(milliseconds: 20));
    expect(tester.getTopLeft(anchor).dy, greaterThan(y));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });
}

class _TranscriptHarness {
  var viewport = ConversationViewportController();
  final messages = [
    for (var i = 0; i < 30; i++)
      message('answer-$i', ChatMessageRole.assistant),
  ];
  late StateSetter rebuild;
  bool loading = false;
  bool working = false;
  final approvals = <InlineApproval>[];
  int loads = 0;
  TargetPlatform platform = TargetPlatform.android;

  Future<void> mount(WidgetTester tester) async {
    addTearDown(viewport.dispose);
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(platform: platform),
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return ConversationTranscript(
                messages: messages,
                viewport: viewport,
                isWorking: working,
                approvals: approvals,
                loadingOlder: loading,
                hasOlderMessages: true,
                onLoadOlder: () => loads++,
              );
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Future<void> readOlder(WidgetTester tester) async {
    await tester.drag(find.byType(ListView), const Offset(0, 400));
    await tester.pumpAndSettle();
    expect(viewport.isDetached, isTrue);
  }

  Finder visibleAnchor(WidgetTester tester) {
    final bounds = tester.getRect(find.byType(ListView));
    for (final element in find.byType(AgentChatBubble).evaluate()) {
      final bubble = element.widget as AgentChatBubble;
      final finder = find.byKey(ValueKey(bubble.message.identity));
      if (bounds.contains(tester.getCenter(finder))) return finder;
    }
    throw StateError('No visible message');
  }

  Future<void> nearOldest(WidgetTester tester) async {
    // Variable-height lazy rows refine the estimated extent after a large jump.
    for (var i = 0; i < 3; i++) {
      viewport.scrollController.jumpTo(
        viewport.scrollController.position.maxScrollExtent - 30,
      );
      await tester.pumpAndSettle();
    }
    expect(viewport.isNearOldest, isTrue);
  }
}
