import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/data/session_title.dart';
import 'package:the_ditch/main.dart';

void main() {
  test('session name waits for Codex and respects manual edits', () {
    final session = AgentSession(
      localId: 'naming',
      provider: AgentProvider.codex,
      status: AgentStatus.working,
      messages: [],
      currentPrompt: 'Full prompt',
    );
    expect(session.displayName, sessionTitlePending);
    session.namingFailed = true;
    expect(session.displayName, sessionTitleFailed);
    session.codexTitle = ' Fix\n login ';
    expect(session.displayName, 'Fix login');
    session.userTitle = 'My name';
    session.codexTitle = 'Later Codex name';
    expect(session.displayName, 'My name');
    session.userTitle = null;
    expect(session.displayName, 'Later Codex name');
  });

  test('names fit Relay UTF-16 limit without splitting emoji', () {
    expect(normalizeSessionTitle('🙂' * 200), '🙂' * 120);
    expect(normalizeSessionTitle('${'a' * 239}🙂'), 'a' * 239);
    expect(normalizeSessionTitle(' \n '), isNull);
  });

  testWidgets('failed naming offers manual rename or an empty-name retry', (
    tester,
  ) async {
    String? renamed = 'unset';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: EditableAgentTitle(
            title: sessionTitleFailed,
            hasOverride: false,
            onRename: (value) => renamed = value,
          ),
        ),
      ),
    );
    expect(
      tester.widget<Tooltip>(find.byType(Tooltip)).message,
      contains('save an empty name to retry'),
    );
    await tester.tap(find.text(sessionTitleFailed));
    await tester.pump();
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('agent-title-editor')))
          .controller!
          .text,
      isEmpty,
    );
    await tester.testTextInput.receiveAction(TextInputAction.done);
    expect(renamed, isNull);
  });

  testWidgets('pending title updates and rename never saves the placeholder', (
    tester,
  ) async {
    String? renamed = 'unset';
    Widget title(String name) => MaterialApp(
      home: Scaffold(
        body: EditableAgentTitle(
          title: name,
          hasOverride: false,
          onRename: (value) => renamed = value,
        ),
      ),
    );
    await tester.pumpWidget(title(sessionTitlePending));
    await tester.tap(find.text(sessionTitlePending));
    await tester.pump();
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('agent-title-editor')))
          .controller!
          .text,
      isEmpty,
    );
    await tester.testTextInput.receiveAction(TextInputAction.done);
    expect(renamed, isNull);
    await tester.pumpWidget(title('Fix login'));
    expect(find.text('Fix login'), findsOneWidget);
    await tester.tap(find.text('Fix login'));
    await tester.pump();
    await tester.enterText(
      find.byKey(const Key('agent-title-editor')),
      '🙂' * 200,
    );
    await tester.testTextInput.receiveAction(TextInputAction.done);
    expect(renamed, '🙂' * 120);
  });
}
