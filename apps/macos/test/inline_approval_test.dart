import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/widgets/inline_approval.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

InlineApproval approval({
  String id = 'request-1',
  Future<void> Function(ApprovalChoice)? send,
}) => InlineApproval(
  request: {
    'id': id,
    'summary': 'Run tests?',
    'command': 'cargo test',
    'target': '/project',
    'created_at': '2026-01-01T00:00:00Z',
  },
  respond: send ?? (_) async {},
  refresh: () async {},
);

void main() {
  for (final choice in ApprovalChoice.values) {
    testWidgets('${choice.label} resolves only after acknowledgement', (
      tester,
    ) async {
      final response = Completer<void>();
      final choices = <ApprovalChoice>[];
      final request = approval(
        send: (value) {
          choices.add(value);
          return response.future;
        },
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(body: InlineApprovalCard(approval: request)),
        ),
      );
      expect(find.byType(AlertDialog), findsNothing);
      await tester.tap(find.text(choice.label));
      await tester.pump();
      await request.choose(choice);
      expect(choices, [choice]);
      expect(find.text(choice.result), findsNothing);
      response.complete();
      await tester.pump();
      expect(find.text(choice.result), findsOneWidget);
      expect(find.text('Approve once'), findsNothing);
    });
  }
  test(
    'offline approval cannot submit and becomes available after reconnect',
    () async {
      var online = false;
      var sends = 0;
      final request = InlineApproval(
        request: {'id': 'request'},
        isAvailable: () => online,
        respond: (_) async {
          sends++;
        },
        refresh: () async {},
      );
      await request.choose(ApprovalChoice.once);
      expect(sends, 0);
      online = true;
      await request.choose(ApprovalChoice.once);
      expect(sends, 1);
    },
  );
  test('expired request cannot submit', () async {
    var sends = 0;
    final request = InlineApproval(
      request: {'id': 'expired', 'expires_at': '2000-01-01T00:00:00Z'},
      respond: (_) async {
        sends++;
      },
      refresh: () async {},
    );
    await request.choose(ApprovalChoice.once);
    expect(sends, 0);
    expect(request.expired, isTrue);
  });
  testWidgets('approval remains actionable when transcript loading fails', (
    tester,
  ) async {
    final request = approval();
    final viewport = ConversationViewportController();
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: Scaffold(
          body: ConversationTranscript(
            messages: const [],
            approvals: [request],
            viewport: viewport,
            ready: false,
            historyError: 'History unavailable',
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Run tests?'), findsOneWidget);
    await tester.tap(find.text('Approve once'));
    await tester.pump();
    expect(request.resolution, 'Approved once');
    await tester.pumpWidget(const SizedBox());
    viewport.dispose();
  });
  test('lost acknowledgement never replays a decision', () async {
    var sends = 0;
    final request = approval(
      send: (_) async {
        sends++;
        throw TimeoutException('lost');
      },
    );
    await request.choose(ApprovalChoice.once);
    await request.choose(ApprovalChoice.session);
    await request.checkStatus();
    expect(sends, 1);
    expect(request.uncertain, isTrue);
    expect(request.resolution, isNull);
    request.resolve();
    expect(request.actionable, isFalse);
  });
  test('resolution from another client disables the request', () async {
    final request = approval();
    request.resolve();
    await request.choose(ApprovalChoice.cancel);
    expect(request.resolution, 'Resolved');
  });
  testWidgets(
    'approval is visible in an empty chat and independent of other agents',
    (tester) async {
      final first = approval();
      final second = approval(id: 'request-2');
      final viewport = ConversationViewportController();
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: ConversationTranscript(
              messages: const [],
              approvals: [first],
              viewport: viewport,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Run tests?'), findsOneWidget);
      expect(find.text('No messages yet.'), findsNothing);
      await tester.tap(find.text('Cancel'));
      await tester.pump();
      expect(first.resolution, 'Cancelled');
      expect(second.actionable, isTrue);
      await tester.pumpWidget(const SizedBox());
      viewport.dispose();
    },
  );
}
