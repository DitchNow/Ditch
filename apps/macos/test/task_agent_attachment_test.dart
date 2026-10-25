import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'task_queue_test.dart' as queue;

Map<String, dynamic> agent(String id, {String? taskId}) => {
  'id': id,
  'project_id': 'project',
  'task_id': taskId,
  'state': 'Interrupted',
  'can_stop': false,
  'native_session_id': 'thread-$id',
  'codex_title': 'Agent $id',
};

void main() {
  testWidgets(
    'stopped same-task agent is selectable in Todo and locked until return',
    (tester) async {
      final f = queue.QueueFixture(
        [
          {...queue.task('a'), 'assigned_agent_id': 'alpha'},
        ],
        agents: [
          agent('alpha', taskId: 'a'),
          agent('beta'),
        ],
      );
      await f.mount(tester);
      await tester.tap(find.text('Task a'));
      await tester.pumpAndSettle();
      final selector = find.byKey(const ValueKey('task-agent-selector'));
      expect(
        tester.widget<DropdownButton<String>>(selector).onChanged,
        isNotNull,
      );
      expect(tester.widget<DropdownButton<String>>(selector).value, 'alpha');
      await tester.tap(selector);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Agent beta').last);
      await tester.pumpAndSettle();
      expect(f.calls.single['SelectAgent']['agent_id'], 'beta');
      expect(f.rows.single['state'], 'Ready');
      await tester.tap(find.text('Close'));
      await tester.pumpAndSettle();
      await queue.drag(
        tester,
        find.text('Task a'),
        find.byKey(const ValueKey('board-column-inProgress')),
      );
      await tester.tap(find.text('Task a'));
      await tester.pumpAndSettle();
      expect(selector, findsNothing);
      expect(find.text('Move to Todo to change the agent.'), findsOneWidget);
      expect(find.text('Agent beta'), findsWidgets);
      await tester.tap(find.text('Close'));
      await tester.pumpAndSettle();
      await queue.drag(
        tester,
        find.text('Task a'),
        find.byKey(const ValueKey('board-column-todo')),
      );
      await tester.tap(find.text('Task a'));
      await tester.pumpAndSettle();
      expect(
        tester.widget<DropdownButton<String>>(selector).onChanged,
        isNotNull,
      );
      await tester.tap(selector);
      await tester.pumpAndSettle();
      await tester.tap(find.text('New agent').last);
      await tester.pumpAndSettle();
      expect(f.calls.last['SelectAgent']['agent_id'], isNull);
      expect(f.coordinatorCalls, 0);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'other Todo agent requires reassignment confirmation; locked agents excluded',
    (tester) async {
      final f = queue.QueueFixture(
        [
          queue.task('a'),
          queue.task('b'),
          {...queue.task('c', position: 1), 'condition': 'Blocked'},
        ],
        agents: [
          agent('transfer', taskId: 'b'),
          agent('locked', taskId: 'c'),
        ],
      );
      await f.mount(tester);
      await tester.tap(find.text('Task a'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('task-agent-selector')));
      await tester.pumpAndSettle();
      expect(find.text('Agent locked'), findsNothing);
      await tester.tap(find.text('Agent transfer').last);
      await tester.pumpAndSettle();
      expect(find.text('Reassign agent?'), findsOneWidget);
      expect(f.calls, isEmpty);
      await tester.tap(find.text('Reassign'));
      await tester.pumpAndSettle();
      expect(f.calls.single['SelectAgent'], {
        'task_id': 'a',
        'expected_revision': 1,
        'agent_id': 'transfer',
        'replace_task_id': 'b',
        'replace_task_revision': 1,
      });
    },
  );

  testWidgets('empty selector explains absence and retains New agent option', (
    tester,
  ) async {
    final f = queue.QueueFixture([queue.task('a')]);
    await f.mount(tester);
    await tester.tap(find.text('Task a'));
    await tester.pumpAndSettle();
    expect(
      find.text('No stopped conversations available in this project.'),
      findsOneWidget,
    );
    expect(
      tester
          .widget<DropdownButton<String>>(
            find.byKey(const ValueKey('task-agent-selector')),
          )
          .onChanged,
      isNotNull,
    );
  });
}
