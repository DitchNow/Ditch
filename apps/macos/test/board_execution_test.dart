import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'task_queue_test.dart' as queue;

void main() {
  testWidgets(
    'master pause and play preserve cards, order and agent selection',
    (tester) async {
      final f = queue.QueueFixture([
        {
          ...queue.task('a', position: 1),
          'continue_agent_id': 'existing-agent',
        },
        queue.task('b', position: 2),
      ]);
      final original = f.rows.map((r) => Map.of(r)).toList();
      await f.mount(tester);
      final toggle = find.byKey(const ValueKey('board-execution-toggle'));
      expect(find.text('Pause'), findsOneWidget);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(f.calls.single['BoardExecution'], {
        'paused': true,
        'expected_revision': 0,
      });
      expect(find.text('Play'), findsOneWidget);
      expect(find.text('Board paused'), findsNWidgets(2));
      expect(f.rows, original);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(f.calls.last['BoardExecution'], {
        'paused': false,
        'expected_revision': 1,
      });
      expect(find.text('Board paused'), findsNothing);
      expect(f.rows, original);
      expect(f.coordinatorCalls, 0);
    },
  );

  testWidgets(
    'paused status loads when board is opened and all-project view has no master',
    (tester) async {
      final f = queue.QueueFixture([queue.task('a', position: 1)])
        ..paused = true;
      await f.mount(tester);
      expect(find.text('Play'), findsOneWidget);
      expect(find.text('Board paused'), findsOneWidget);
      expect(f.calls, isEmpty);
    },
  );

  testWidgets('all-project view does not imply cross-project control', (
    tester,
  ) async {
    final f = queue.QueueFixture([queue.task('a', position: 1)]);
    await f.mount(tester, all: true);
    expect(find.byKey(const ValueKey('board-execution-toggle')), findsNothing);
  });

  test(
    'late reads and replay responses cannot overwrite newer execution events',
    () async {
      final pending = Completer<Map<String, dynamic>>();
      final c = TaskBoardController(request: (_) => pending.future);
      addTearDown(c.dispose);
      final load = c.loadBoardExecution('project');
      c.applyEvent({
        'BoardExecutionChanged': {
          'project_id': 'project',
          'paused': true,
          'revision': 3,
        },
      });
      pending.complete({
        'TaskResponse': {
          'BoardExecution': {
            'project_id': 'project',
            'paused': false,
            'revision': 2,
          },
        },
      });
      await load;
      expect(c.boardExecutions['project']!.paused, isTrue);
      expect(c.boardExecutions['project']!.revision, 3);
      c.applyEvent({
        'BoardExecutionChanged': {
          'project_id': 'other',
          'paused': false,
          'revision': 10,
        },
      });
      expect(c.boardExecutions['project']!.paused, isTrue);
    },
  );
}
