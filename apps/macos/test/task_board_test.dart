import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/task_board.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'task_agent_attachment_test.dart' show agent;
import 'task_queue_test.dart' as queue;

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
Future<void> mountAgentBoard(
  WidgetTester tester,
  queue.QueueFixture fixture,
  List<String> opened, {
  bool dark = false,
  String? projectId,
  Size size = const Size(1700, 950),
}) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(fixture.controller.dispose);
  fixture.controller.replaceSnapshot({
    'tasks': fixture.rows,
    'agents': fixture.agents,
  });
  await tester.pumpWidget(
    MaterialApp(
      theme: dark ? DitchTheme.dark() : DitchTheme.light(),
      home: Scaffold(
        body: TaskBoard(
          controller: fixture.controller,
          projects: const [
            TaskProjectOption('project', 'First'),
            TaskProjectOption('second', 'Second'),
          ],
          initialProjectId: projectId,
          connected: true,
          executionProfile: () => {},
          onOpenAgent: opened.add,
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  for (final state in ['Running', 'InReview', 'Accepted', 'Ready']) {
    for (final dark in [false, true]) {
      testWidgets('robot opens agent directly in $state (dark: $dark)', (
        tester,
      ) async {
        final f = queue.QueueFixture(
          [
            {
              ...taskJson(state: state),
              'condition': state == 'Running' ? 'Running' : 'Idle',
              'assigned_agent_id': 'alpha',
            },
          ],
          agents: [agent('alpha', taskId: 'one')],
        );
        final opened = <String>[];
        await mountAgentBoard(tester, f, opened, dark: dark);
        final before = f.controller.task('one');
        final button = find.byKey(const ValueKey('task-open-agent-one'));
        expect(button.hitTestable(), findsOneWidget);
        expect(find.byTooltip('Go to agent'), findsOneWidget);
        await tester.tap(button);
        await tester.pump(const Duration(milliseconds: 350));
        await tester.pumpAndSettle();
        expect(opened, ['alpha']);
        expect(find.byType(AlertDialog), findsNothing);
        expect(f.calls, isEmpty);
        expect(f.controller.task('one'), same(before));
        await tester.tap(find.text('Task one'));
        if (state == 'InReview' || state == 'Accepted') {
          await tester.pump(const Duration(milliseconds: 50));
          await tester.tap(find.text('Task one'));
        }
        await tester.pumpAndSettle();
        expect(find.byType(AlertDialog), findsOneWidget);
        expect(opened, ['alpha']);
        expect(tester.takeException(), isNull);
      });
    }
  }

  for (final attachment in [
    {
      'continue_agent_id': 'continuation',
      'assigned_agent_id': 'assigned',
      'execution_queue': {'agent_id': 'queued'},
    },
    {
      'assigned_agent_id': 'assigned',
      'execution_queue': {'agent_id': 'queued'},
    },
    {
      'execution_queue': {'agent_id': 'queued'},
    },
  ]) {
    final expected =
        attachment['continue_agent_id'] ??
        attachment['assigned_agent_id'] ??
        'queued';
    testWidgets('robot resolves $expected with attachment precedence', (
      tester,
    ) async {
      final f = queue.QueueFixture(
        [
          {...taskJson(), ...attachment},
        ],
        agents: [agent('continuation'), agent('assigned'), agent('queued')],
      );
      final opened = <String>[];
      await mountAgentBoard(tester, f, opened);
      await tester.tap(find.byKey(const ValueKey('task-open-agent-one')));
      await tester.pumpAndSettle();
      expect(opened, [expected]);
      expect(f.calls, isEmpty);
    });
  }

  testWidgets('robot is absent without a resolvable selected agent', (
    tester,
  ) async {
    final f = queue.QueueFixture(
      [
        taskJson(id: 'unassigned'),
        {...taskJson(id: 'missing'), 'assigned_agent_id': 'missing'},
        {
          ...taskJson(id: 'missing-continuation'),
          'continue_agent_id': 'missing',
          'assigned_agent_id': 'alpha',
        },
      ],
      agents: [agent('alpha')],
    );
    await mountAgentBoard(tester, f, []);
    expect(find.byType(TaskCard), findsNWidgets(3));
    expect(find.byTooltip('Go to agent'), findsNothing);
    expect(find.byIcon(Icons.smart_toy_outlined), findsNothing);
  });

  testWidgets('robot retains agent after dragging stopped work to Todo', (
    tester,
  ) async {
    final f = queue.QueueFixture(
      [
        {
          ...queue.task('a', position: 1),
          'condition': 'Blocked',
          'assigned_agent_id': 'alpha',
        },
      ],
      agents: [agent('alpha', taskId: 'a')],
    );
    final opened = <String>[];
    await mountAgentBoard(tester, f, opened, projectId: 'project');
    await queue.drag(
      tester,
      find.text('Task a'),
      find.byKey(const ValueKey('board-column-todo')),
    );
    expect(f.controller.task('a')!.column, TaskColumn.todo);
    expect(f.calls.single['Move']['column'], 'Todo');
    await tester.tap(find.byKey(const ValueKey('task-open-agent-a')));
    await tester.pumpAndSettle();
    expect(opened, ['alpha']);
    expect(f.calls, hasLength(1));
    expect(find.byType(AlertDialog), findsNothing);
  });

  testWidgets(
    'combined board routes each project and rechecks changed attachments',
    (tester) async {
      final first = {...taskJson(), 'assigned_agent_id': 'alpha'};
      final second = {
        ...taskJson(id: 'two', project: 'second'),
        'assigned_agent_id': 'beta',
      };
      final f = queue.QueueFixture(
        [first, second],
        agents: [
          agent('alpha'),
          {...agent('beta'), 'project_id': 'second'},
        ],
      );
      final opened = <String>[];
      await mountAgentBoard(tester, f, opened);
      final firstButton = find.byKey(const ValueKey('task-open-agent-one'));
      await tester.tap(firstButton);
      await tester.tap(find.byKey(const ValueKey('task-open-agent-two')));
      expect(opened, ['alpha', 'beta']);
      final oldCallback = tester.widget<IconButton>(firstButton).onPressed!;
      f.controller.applyEvent({
        'TaskChanged': {...first, 'revision': 2, 'assigned_agent_id': 'beta'},
      });
      // Activate before a rebuild to exercise a stale card callback.
      oldCallback();
      expect(opened, ['alpha', 'beta', 'beta']);
      await tester.pumpAndSettle();
      await tester.tap(firstButton);
      expect(opened.last, 'beta');
      expect(opened, hasLength(4));
      f.controller.replaceSnapshot({
        'tasks': [
          {...first, 'revision': 3, 'assigned_agent_id': 'beta'},
          second,
        ],
        'agents': [agent('alpha')],
      });
      oldCallback();
      expect(opened, hasLength(4));
      await tester.pumpAndSettle();
      expect(find.byTooltip('Go to agent'), findsNothing);
      f.controller.applyEvent({
        'TaskDeleted': {'task_id': 'one'},
      });
      oldCallback();
      expect(opened, hasLength(4));
      expect(f.calls, isEmpty);
      expect(find.byType(AlertDialog), findsNothing);
    },
  );

  testWidgets('robot is accessible and keyboard activated on a narrow board', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final f = queue.QueueFixture(
      [
        {...taskJson(), 'assigned_agent_id': 'alpha'},
      ],
      agents: [agent('alpha')],
    );
    final opened = <String>[];
    await mountAgentBoard(tester, f, opened, size: const Size(680, 600));
    final button = find.byKey(const ValueKey('task-open-agent-one'));
    await tester.ensureVisible(button);
    await tester.pumpAndSettle();
    expect(
      tester.getSemantics(button),
      matchesSemantics(
        label: 'Go to agent',
        isButton: true,
        hasEnabledState: true,
        isEnabled: true,
        isFocusable: true,
        hasTapAction: true,
        hasFocusAction: true,
      ),
    );
    expect(tester.getSize(button).width, greaterThanOrEqualTo(40));
    expect(tester.getSize(button).height, greaterThanOrEqualTo(40));
    Focus.of(
      tester.element(find.byIcon(Icons.smart_toy_outlined)),
    ).requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    semantics.dispose();
    expect(opened, ['alpha']);
    expect(find.byType(AlertDialog), findsNothing);
    expect(f.calls, isEmpty);
    expect(tester.takeException(), isNull);
  });

  test(
    'transition history excludes metadata refresh without removing workflow events',
    () async {
      final json = taskJson();
      final c = TaskBoardController(
        request: (_) async => {
          'TaskResponse': {
            'Detail': {
              'task': json,
              'history': [
                for (final action in [
                  'github_import',
                  'github_refresh',
                  'prioritize',
                  'remote_materialized',
                ])
                  {
                    'action': action,
                    'actor': 'User',
                    'created_at': '2026-10-29T12:00:00Z',
                  },
              ],
            },
          },
        },
      );
      addTearDown(c.dispose);
      final history = await c.history(TaskDto.fromJson(json));
      expect(history.map((event) => event.action), [
        'github_import',
        'prioritize',
        'remote_materialized',
      ]);
    },
  );
  testWidgets('board keeps secondary controls in its menu', (tester) async {
    await tester.binding.setSurfaceSize(const Size(1400, 900));
    final c = TaskBoardController(
      request: (_) async => {
        'TaskResponse': {'Tasks': []},
      },
    );
    c.replaceSnapshot({'tasks': [], 'agents': []});
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(extensions: const [DitchTokens.light]),
        home: Scaffold(
          body: TaskBoard(
            controller: c,
            projects: const [TaskProjectOption('project', 'Project')],
            connected: true,
            executionProfile: () => {},
            onOpenAgent: (_) {},
            onRunSelected: (_, _) async {},
            onPause: () async {},
          ),
        ),
      ),
    );
    expect(find.text('Run selected (0)'), findsNothing);
    expect(find.text('Pause queued work'), findsNothing);
    expect(find.text('Show archived'), findsNothing);
    expect(find.text('New task'), findsOneWidget);
    await tester.tap(find.byTooltip('Board options'));
    await tester.pumpAndSettle();
    expect(find.text('Pause queued work'), findsNothing);
    await tester.tap(
      find.widgetWithText(CheckedPopupMenuItem<String>, 'Show archived'),
    );
    await tester.pumpAndSettle();
    expect(find.text('Archived included'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
    c.dispose();
    await tester.binding.setSurfaceSize(null);
  });
  test('backlog remains distinct from ready work', () {
    expect(
      TaskDto.fromJson(taskJson(state: 'Backlog')).column,
      TaskColumn.backlog,
    );
    expect(TaskDto.fromJson(taskJson()).column, TaskColumn.todo);
  });
  testWidgets('legacy run controls stay hidden even with callbacks installed', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1400, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final c = TaskBoardController(
      request: (_) async => {
        'TaskResponse': {
          'Tasks': [taskJson()],
        },
      },
    );
    addTearDown(c.dispose);
    c.replaceSnapshot({
      'tasks': [taskJson()],
      'agents': [],
    });
    var calls = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(extensions: const [DitchTokens.light]),
        home: Scaffold(
          body: TaskBoard(
            controller: c,
            projects: const [TaskProjectOption('project', 'Project')],
            connected: true,
            executionProfile: () => {'approval': 'Ask'},
            onOpenAgent: (_) {},
            onRunSelected: (_, _) async {
              calls++;
            },
            onPause: () async {
              calls++;
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Select for Run'), findsNothing);
    expect(find.byType(CheckboxListTile), findsNothing);
    await tester.tap(find.byTooltip('Board options'));
    await tester.pumpAndSettle();
    expect(find.text('Pause queued work'), findsNothing);
    expect(calls, 0);
  });
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
          if (payload['operation'] == 'ListReleases') {
            return {
              'TaskResponse': {'Releases': []},
            };
          }
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
              'ReleaseBatch': {
                'tasks': [
                  {
                    ...taskJson(state: 'Accepted', revision: 2),
                    'review_summary': 'Finished',
                  },
                ],
                'releases': [],
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
      expect(find.text('Accept and release work'), findsOneWidget);
      expect(
        requests.where(
          (r) =>
              r['operation'] is Map &&
              (r['operation'] as Map).containsKey('AcceptRelease'),
        ),
        isEmpty,
      );
      await tester.tap(find.text('Accept and move'));
      await tester.pumpAndSettle();
      expect(
        (requests.lastWhere(
              (r) =>
                  r['operation'] is Map &&
                  (r['operation'] as Map).containsKey('AcceptRelease'),
            )['operation']
            as Map)['AcceptRelease']['destination'],
        'Other',
      );
      await tester.pumpWidget(const SizedBox());
      c.dispose();
    },
  );
}
