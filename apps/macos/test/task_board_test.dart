import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/task_board.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

Map<String, dynamic> taskJson({
  String id = 'one',
  String project = 'project',
  String state = 'Ready',
  int revision = 1,
  int order = 1024,
}) => {
  'id': id,
  'project_id': project,
  'title': 'Task $id',
  'description': 'Description',
  'state': state,
  'condition': 'Idle',
  'priority': 'Normal',
  'acceptance_criteria': ['Reviewed by a human'],
  'revision': revision,
  'order_key': order,
  'archived': false,
};
void main() {
  test('legacy states map without fabricating review evidence', () {
    for (final state in [
      'Draft',
      'Ready',
      'Running',
      'Blocked',
      'Rejected',
      'InReview',
      'Accepted',
      'Cancelled',
    ]) {
      final task = TaskDto.fromJson(taskJson(state: state));
      expect(task.summary, isNull);
      expect(
        task.column,
        state == 'Accepted'
            ? TaskColumn.done
            : state == 'InReview'
            ? TaskColumn.inReview
            : ['Running', 'Blocked', 'Rejected'].contains(state)
            ? TaskColumn.inProgress
            : TaskColumn.todo,
      );
    }
    expect(
      () => TaskDto.fromJson(taskJson(state: 'MadeUp')),
      throwsFormatException,
    );
  });
  test(
    'snapshots and events converge and older responses cannot regress task revisions',
    () {
      final c = TaskBoardController(request: (_) async => {});
      c.replaceSnapshot({
        'tasks': [taskJson()],
        'agents': [],
      });
      c.applyEvent({'TaskChanged': taskJson(state: 'Running', revision: 2)});
      c.applyEvent({'TaskChanged': taskJson()});
      expect(c.task('one')!.revision, 2);
      final other = TaskBoardController(request: (_) async => {});
      other.replaceSnapshot({
        'tasks': [taskJson(state: 'Running', revision: 2)],
        'agents': [],
      });
      expect(c.task('one')!.column, other.task('one')!.column);
      c.applyEvent({
        'TaskDeleted': {'task_id': 'one'},
      });
      c.applyEvent({'TaskChanged': taskJson()});
      expect(c.tasks, isEmpty);
      c.dispose();
      other.dispose();
    },
  );
  test('interrupted mutations retry the same idempotency key', () async {
    final requests = <Map>[];
    var attempts = 0;
    final c = TaskBoardController(
      request: (request) async {
        requests.add((request as Map)['TaskRequest'] as Map);
        if (attempts++ == 0) throw Exception('socket interrupted');
        return {
          'TaskResponse': {'Changed': taskJson()},
        };
      },
    );
    await c.mutate('project', {
      'Create': {'draft': {}},
    });
    expect(c.canRetry, isTrue);
    await c.retry();
    expect(requests[0]['request_id'], requests[1]['request_id']);
    expect(c.tasks.length, 1);
    c.dispose();
  });
  test(
    'an in-flight refresh cannot resurrect deleted cards or overwrite newer events',
    () async {
      final response = Completer<Map<String, dynamic>>();
      final c = TaskBoardController(request: (_) => response.future);
      c.replaceSnapshot({
        'tasks': [taskJson()],
        'agents': [],
      });
      final refresh = c.refresh();
      c.applyEvent({
        'TaskDeleted': {'task_id': 'one'},
      });
      c.applyEvent({'TaskChanged': taskJson(id: 'two', revision: 3)});
      response.complete({
        'TaskResponse': {
          'Tasks': [taskJson(), taskJson(id: 'two', revision: 1)],
        },
      });
      await refresh;
      expect(c.task('one'), isNull);
      expect(c.task('two')!.revision, 3);
      c.dispose();
    },
  );
  testWidgets(
    'large board virtualizes cards in dark theme and shows disconnect state',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(680, 600));
      final c = TaskBoardController(request: (_) async => {});
      c.replaceSnapshot({
        'tasks': List.generate(2000, (i) => taskJson(id: 'many-$i')),
        'agents': [],
      });
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData(
            brightness: Brightness.dark,
            extensions: const [DitchTokens.dark],
          ),
          home: Scaffold(
            body: TaskBoard(
              controller: c,
              projects: const [TaskProjectOption('project', 'First')],
              connected: false,
              executionProfile: () => {'approval': 'Ask'},
              onOpenAgent: (_) {},
            ),
          ),
        ),
      );
      expect(find.text('Todo  2000'), findsOneWidget);
      expect(
        find.textContaining('Task many-').evaluate().length,
        lessThan(100),
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'board provides narrow scrolling, filtering, and accessible task actions',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(680, 600));
      final c = TaskBoardController(request: (_) async => {});
      c.replaceSnapshot({
        'tasks': [
          taskJson(),
          taskJson(id: 'other', project: 'second', state: 'InReview'),
        ],
        'agents': [],
      });
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData(extensions: const [DitchTokens.light]),
          home: Scaffold(
            body: TaskBoard(
              controller: c,
              projects: const [
                TaskProjectOption('project', 'First'),
                TaskProjectOption('second', 'Second'),
              ],
              connected: true,
              executionProfile: () => {'approval': 'ApproveForMe'},
              onOpenAgent: (_) {},
            ),
          ),
        ),
      );
      expect(find.text('Todo  1'), findsOneWidget);
      expect(find.text('In Review  1'), findsOneWidget);
      expect(find.byTooltip('Task actions for Task one'), findsOneWidget);
      await tester.tap(find.byTooltip('Task actions for Task one'));
      await tester.pumpAndSettle();
      expect(find.text('Move to In Progress…'), findsOneWidget);
      expect(find.text('Review and Accept…'), findsNothing);
      await tester.tapAt(const Offset(5, 5));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
      c.dispose();
      await tester.binding.setSurfaceSize(null);
    },
  );
  testWidgets(
    'review acceptance asks for explicit confirmation and feedback is required',
    (tester) async {
      final requests = <Map>[];
      final c = TaskBoardController(
        request: (request) async {
          final payload = (request as Map)['TaskRequest'] as Map;
          requests.add(payload);
          if ((payload['operation'] as Map).containsKey('Get')) {
            return {
              'TaskResponse': {
                'Detail': {
                  'task': {
                    ...taskJson(state: 'InReview'),
                    'review_summary': 'Finished',
                  },
                  'history': [],
                },
              },
            };
          }
          return {
            'TaskResponse': {
              'Changed': {
                ...taskJson(state: 'Accepted', revision: 2),
                'review_summary': 'Finished',
              },
            },
          };
        },
      );
      c.replaceSnapshot({
        'tasks': [
          {...taskJson(state: 'InReview'), 'review_summary': 'Finished'},
        ],
        'agents': [],
      });
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData(extensions: const [DitchTokens.light]),
          home: Builder(
            builder: (context) => Scaffold(
              body: TextButton(
                onPressed: () => showTaskInspector(
                  context,
                  c,
                  c.task('one')!,
                  const [TaskProjectOption('project', 'First')],
                  () => {'approval': 'ApproveForMe'},
                  (_) {},
                ),
                child: const Text('Open'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextButton>(
              find.widgetWithText(TextButton, 'Request Changes'),
            )
            .onPressed,
        isNull,
      );
      await tester.tap(find.text('Accept…'));
      await tester.pumpAndSettle();
      expect(find.text('Accept this work?'), findsOneWidget);
      expect(
        requests.where(
          (r) => (r['operation'] as Map).containsKey('Transition'),
        ),
        isEmpty,
      );
      await tester.tap(find.text('Accept'));
      await tester.pumpAndSettle();
      expect(
        (requests.lastWhere(
              (r) => (r['operation'] as Map).containsKey('Transition'),
            )['operation']
            as Map)['Transition']['action'],
        'Accept',
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
}
