import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';

void main() {
  testWidgets('background diagrams preserve detached transcript position', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    addTearDown(viewport.dispose);
    var loads = 0;
    final messages = List.generate(
      12,
      (index) => AgentChatMessage(
        identity: 'diagram-$index',
        role: ChatMessageRole.assistant,
        text: '```mermaid\ngraph LR\n A[Response $index] --> B[Ready]\n```',
        createdAt: DateTime(2026),
      ),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 340,
            child: ConversationTranscript(
              messages: messages,
              viewport: viewport,
              hasOlderMessages: true,
              onLoadOlder: () => loads++,
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, 400));
    viewport.scrollController.jumpTo(500);
    await tester.pump();
    expect(viewport.mode, ConversationViewportMode.detached);
    final offset = viewport.scrollController.offset;
    final baselineLoads = loads;
    for (var i = 0; i < 600; i++) {
      await tester.pump(const Duration(milliseconds: 20));
      if (find.text('Rendering diagram…').evaluate().isEmpty) break;
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 10)),
      );
    }
    expect(find.text('Rendering diagram…'), findsNothing);
    expect(viewport.mode, ConversationViewportMode.detached);
    expect(viewport.scrollController.offset, moreOrLessEquals(offset));
    expect(viewport.unseenCount, 0);
    expect(loads, baselineLoads);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
}
