import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/task_board.dart';

Map<String, dynamic> task(String id, {int? position, bool running = false}) => {
  'id': id,
  'project_id': 'project',
  'title': 'Task $id',
  'description': '',
  'state': position == null ? 'Ready' : 'Running',
  'condition': position == null
      ? 'Idle'
      : running
      ? 'Running'
      : 'Queued',
  'assigned_agent_id': running ? 'agent' : null,
  'priority': 'Normal',
  'revision': 1,
  'order_key': id == 'a' ? 1 : 2,
  'archived': false,
  'planning': {'text': 'Implementation plan'},
  if (position != null)
    'execution_queue': {'position': position, 'profile': {}},
};

class QueueFixture {
  QueueFixture(this.rows, {this.agents = const []});
  List<Map<String, dynamic>> rows;
  List<Map<String, dynamic>> agents;
  final calls = <Map>[];
  int coordinatorCalls = 0;
  bool paused = false;
  int executionRevision = 0;
  late final TaskBoardController controller = TaskBoardController(
    request: (request) async {
      if ((request as Map)['TaskRequest'] case final Map body) {
        final operation = body['operation'] as Map;
        if (operation['BoardExecution'] case final Map execution) {
          if (execution['paused'] is bool) {
            calls.add(operation);
            paused = execution['paused'] as bool;
            executionRevision++;
          }
          return {
            'TaskResponse': {
              'BoardExecution': {
                'project_id': body['project_id'],
                'paused': paused,
                'revision': executionRevision,
              },
            },
          };
        }
        if (operation.containsKey('BoardProjects')) {
          return {
            'TaskResponse': {'BoardProjects': []},
          };
        }
        if (operation.containsKey('List')) {
          return {
            'TaskResponse': {'Tasks': rows},
          };
        }
        if (operation['Get'] case final Map get) {
          return {
            'TaskResponse': {
              'Detail': {
                'task': rows.firstWhere((t) => t['id'] == get['task_id']),
                'history': [],
              },
            },
          };
        }
        calls.add(operation);
        final value = operation.values.first as Map;
        final index = rows.indexWhere((t) => t['id'] == value['task_id']);
        if (operation.containsKey('SelectAgent')) {
          final selected = value['agent_id'];
          rows[index] = {
            ...rows[index],
            'assigned_agent_id': selected,
            'continue_agent_id': selected,
          };
          for (var i = 0; i < agents.length; i++) {
            if (agents[i]['id'] == selected ||
                agents[i]['task_id'] == rows[index]['id']) {
              agents[i] = {
                ...agents[i],
                'task_id': agents[i]['id'] == selected
                    ? rows[index]['id']
                    : null,
              };
              controller.applyEvent({'AgentChanged': agents[i]});
            }
          }
        } else if (operation.containsKey('Enqueue')) {
          rows[index] = {
            ...rows[index],
            'state': 'Running',
            'condition': 'Queued',
            'assigned_agent_id': null,
            'last_reason': null,
            'execution_queue': {
              'agent_id':
                  rows[index]['continue_agent_id'] ??
                  rows[index]['assigned_agent_id'],
              'position': 1,
              'profile': value['execution_profile'],
            },
          };
        } else if (operation.containsKey('Move')) {
          rows[index] = {
            ...rows[index],
            'state': 'Ready',
            'condition': 'Idle',
            'execution_queue': null,
          };
        } else if (operation.containsKey('ReorderQueue')) {
          for (var i = 0; i < rows.length; i++) {
            rows[i] = {
              ...rows[i],
              'execution_queue': {
                'position': i == index ? 1 : 2,
                'profile': {},
              },
            };
          }
        }
        rows[index] = {
          ...rows[index],
          'revision': (rows[index]['revision'] as int) + 1,
        };
        return {
          'TaskResponse': {'Changed': rows[index]},
        };
      }
      return {
        'GitHub': {'connected': false},
      };
    },
  );
  Future<void> mount(WidgetTester tester, {bool all = false}) async {
    tester.view.physicalSize = const Size(1700, 950);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(controller.dispose);
    controller.replaceSnapshot({'tasks': rows, 'agents': agents});
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: Scaffold(
          body: TaskBoard(
            controller: controller,
            projects: const [TaskProjectOption('project', 'Project')],
            initialProjectId: all ? null : 'project',
            connected: true,
            executionProfile: () => {
              'model': 'chosen-model',
              'approval': 'Ask',
            },
            onOpenAgent: (_) {},
            onRunSelected: (_, _) async {
              coordinatorCalls++;
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }
}

Future<void> drag(WidgetTester tester, Finder from, Finder to) async {
  final gesture = await tester.startGesture(tester.getCenter(from));
  await gesture.moveBy(const Offset(10, 0));
  await tester.pump();
  await gesture.moveTo(tester.getCenter(to));
  await tester.pump(const Duration(milliseconds: 150));
  await gesture.up();
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'blocked startup has an explicit retry and no phantom agent or lock',
    (tester) async {
      final f = QueueFixture([
        {
          ...task('a', position: 1),
          'condition': 'Blocked',
          // Old persisted failures may still contain a reserved ID, but no agent.
          'assigned_agent_id': 'never-created',
          'last_reason': 'Workspace snapshot failed',
          'planning': {
            'text': 'Keep this plan',
            'job': {'status': 'canceled'},
          },
        },
      ]);
      await f.mount(tester);
      expect(find.text('Plan ready'), findsOneWidget);
      expect(find.text('Plan canceled'), findsNothing);
      expect(find.text('Workspace snapshot failed'), findsOneWidget);
      expect(find.byIcon(Icons.lock_outline), findsNothing);
      expect(find.byIcon(Icons.smart_toy_outlined), findsNothing);
      await tester.tap(find.text('Review & retry'));
      await tester.pumpAndSettle();
      expect(find.text('Use local App Server'), findsNothing);
      expect(find.byIcon(Icons.chat_bubble_outline), findsNothing);
      expect(find.text('Submit for Review'), findsNothing);
      expect(find.text('Submission summary'), findsNothing);
      await tester.tap(find.text('Retry task'));
      await tester.pumpAndSettle();
      expect(f.calls.single.containsKey('Enqueue'), isTrue);
      expect(
        f.calls.single['Enqueue']['execution_profile']['transport'],
        'AppServer',
      );
      expect(f.rows.single['planning']['text'], 'Keep this plan');
      expect(f.rows.single['assigned_agent_id'], isNull);
      expect(f.coordinatorCalls, 0);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'Todo to In Progress enqueues with user profile without Ditchmaster',
    (tester) async {
      final f = QueueFixture([task('a')]);
      await f.mount(tester);
      await drag(
        tester,
        find.text('Task a'),
        find.byKey(const ValueKey('board-column-inProgress')),
      );
      expect(f.calls.single.containsKey('Enqueue'), isTrue);
      expect((f.calls.single['Enqueue'] as Map)['execution_profile'], {
        'model': 'chosen-model',
        'approval': 'Ask',
        'transport': 'AppServer',
      });
      expect(f.coordinatorCalls, 0);
      expect(find.text('Queued'), findsOneWidget);
      expect(find.byIcon(Icons.drag_indicator), findsOneWidget);
      expect(find.byType(AlertDialog), findsNothing);
    },
  );

  testWidgets('project queue uses position and can reorder waiting cards', (
    tester,
  ) async {
    final f = QueueFixture([task('a', position: 2), task('z', position: 1)]);
    await f.mount(tester);
    expect(
      tester.getTopLeft(find.text('Task z')).dy,
      lessThan(tester.getTopLeft(find.text('Task a')).dy),
    );
    await drag(tester, find.text('Task a'), find.text('Task z'));
    expect(f.calls.single['ReorderQueue'], {
      'task_id': 'a',
      'expected_revision': 1,
      'before_id': 'z',
    });
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'All Projects has no ordering controls and rejects same-column reorder',
    (tester) async {
      final f = QueueFixture([task('a', position: 2), task('z', position: 1)]);
      await f.mount(tester, all: true);
      expect(find.byIcon(Icons.drag_indicator), findsNothing);
      await drag(tester, find.text('Task a'), find.text('Task z'));
      expect(f.calls, isEmpty);
      await tester.tap(find.byTooltip('Task actions for Task a'));
      await tester.pumpAndSettle();
      expect(find.text('Move up'), findsNothing);
    },
  );

  testWidgets('running task is fixed and a waiting task can leave the queue', (
    tester,
  ) async {
    final f = QueueFixture([
      task('a', position: 1, running: true),
      task('z', position: 2),
    ]);
    await f.mount(tester);
    expect(find.byIcon(Icons.lock_outline), findsOneWidget);
    await drag(
      tester,
      find.text('Task z'),
      find.byKey(const ValueKey('board-column-todo')),
    );
    expect(f.calls.single.containsKey('Move'), isTrue);
    expect(TaskDto.fromJson(f.rows.last).executionQueue, isNull);
    expect(tester.takeException(), isNull);
  });
}
