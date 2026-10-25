import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/task_board.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

class PlanningFixture {
  Map<String, dynamic> task = {
    'id': 'task',
    'project_id': 'project',
    'title': 'Make the map clearer',
    'description': 'Original GitHub description',
    'state': 'Backlog',
    'condition': 'Idle',
    'priority': 'Normal',
    'acceptance_criteria': <String>[],
    'revision': 1,
    'order_key': 1,
    'archived': false,
    'planning': {'text': ''},
  };
  final calls = <Map>[];
  late final controller = TaskBoardController(
    request: (request) async {
      final p = (request as Map)['TaskRequest'] as Map;
      final op = p['operation'] as Map;
      calls.add(op);
      if (op.containsKey('List')) {
        return {
          'TaskResponse': {
            'Tasks': [task],
          },
        };
      }
      if (op.containsKey('Get')) {
        return {
          'TaskResponse': {
            'Detail': {'task': task, 'history': []},
          },
        };
      }
      if (op['UpdateDescription'] case final Map update) {
        if (update['expected_description'] != task['description'] ||
            update['expected_plan'] != (task['planning'] as Map)['text']) {
          return {
            'TaskResponse': {
              'Error': {
                'code': 'RevisionConflict',
                'message': 'Changed elsewhere',
              },
            },
          };
        }
        task = {
          ...task,
          'description': update['description'],
          'planning': {...task['planning'] as Map, 'text': update['plan']},
        };
      }
      if (op.containsKey('GeneratePlan')) {
        task = {
          ...task,
          'planning': {
            ...task['planning'] as Map,
            'job': {
              'id': 'job',
              'status': 'running',
              'title': task['title'],
              'description': task['description'],
              'previous_plan': (task['planning'] as Map)['text'],
            },
          },
        };
      }
      if (op.containsKey('CancelPlan')) {
        task = {
          ...task,
          'planning': {
            ...task['planning'] as Map,
            'job': {'id': 'job', 'status': 'canceled'},
          },
        };
      }
      task = {...task, 'revision': (task['revision'] as int) + 1};
      return {
        'TaskResponse': {'Changed': task},
      };
    },
  );
  void complete() {
    task = {
      ...task,
      'revision': (task['revision'] as int) + 1,
      'planning': {
        'text': 'Inspect the code, implement the change, and test.',
        'job': {
          ...(task['planning'] as Map)['job'] as Map,
          'status': 'completed',
          'result': 'Inspect the code, implement the change, and test.',
        },
      },
    };
    controller.applyEvent({'TaskChanged': task});
  }
}

Future<void> mount(WidgetTester tester, PlanningFixture f) async {
  tester.view.physicalSize = const Size(1200, 1000);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  f.controller.replaceSnapshot({
    'tasks': [f.task],
    'agents': [],
  });
  await tester.pumpWidget(
    MaterialApp(
      theme: DitchTheme.light(),
      home: Builder(
        builder: (context) => Scaffold(
          body: TextButton(
            onPressed: () => showTaskInspector(
              context,
              f.controller,
              f.controller.task('task')!,
              const [TaskProjectOption('project', 'Project')],
              () => {'model': 'chosen-model'},
              (_) {},
            ),
            child: const Text('Open task'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('Open task'));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('card shows compact live planning and result states', (
    tester,
  ) async {
    final f = PlanningFixture();
    var opened = 0;
    Future<void> card(Map<String, dynamic> planning) async {
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: SizedBox(
              width: 260,
              child: TaskCard(
                task: TaskDto.fromJson({...f.task, 'planning': planning}),
                projectName: 'Project',
                onOpen: () => opened++,
                onMove: (_) {},
              ),
            ),
          ),
        ),
      );
      await tester.pump();
      expect(tester.takeException(), isNull);
    }

    await card({});
    expect(find.text('Plan ready'), findsNothing);
    await card({
      'job': {'status': 'running'},
    });
    expect(find.text('Planning…'), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsOneWidget);
    await tester.tap(find.text('Planning…'));
    expect(opened, 1);
    await card({
      'text': 'Plan',
      'job': {
        'status': 'completed',
        'title': f.task['title'],
        'description': f.task['description'],
        'result': 'Plan',
      },
    });
    expect(find.text('Plan ready'), findsOneWidget);
    expect(find.byType(CircularProgressIndicator), findsNothing);
    await card({
      'text': 'My edit',
      'job': {
        'status': 'completed',
        'title': f.task['title'],
        'description': f.task['description'],
        'result': 'Generated plan',
      },
    });
    expect(find.text('Plan ready'), findsOneWidget);
    for (final status in ['failed', 'interrupted']) {
      await card({
        'job': {'status': status},
      });
      expect(find.text('Plan failed'), findsOneWidget);
    }
    await card({
      'job': {'status': 'canceled'},
    });
    expect(find.text('Plan canceled'), findsOneWidget);
    for (final status in ['failed', 'interrupted', 'canceled']) {
      await card({
        'text': 'Preserved plan',
        'job': {'status': status},
      });
      expect(find.text('Plan ready'), findsOneWidget);
      expect(find.text('Plan canceled'), findsNothing);
      expect(find.text('Plan failed'), findsNothing);
    }
    await card({
      'text': '',
      'job': {'status': 'completed', 'result': 'Unapplied plan'},
    });
    expect(find.text('Review plan'), findsOneWidget);

    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('unsaved plan edits survive AI completion and can be saved', (
    tester,
  ) async {
    final f = PlanningFixture();
    f.task = {
      ...f.task,
      'planning': {'text': 'Earlier plan'},
    };
    await mount(tester, f);
    await tester.tap(find.text('Generate with AI'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    await tester.enterText(
      find.byKey(const ValueKey('task-plan')),
      'My refined plan',
    );
    f.complete();
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextField>(find.byKey(const ValueKey('task-plan')))
          .controller!
          .text,
      'My refined plan',
    );
    await tester.ensureVisible(find.text('Save & close'));
    await tester.tap(find.text('Save & close'));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    expect((f.task['planning'] as Map)['text'], 'My refined plan');
    await tester.pumpWidget(const SizedBox());
    f.controller.dispose();
  });

  testWidgets('a conflicting save keeps the draft and offers discard', (
    tester,
  ) async {
    final f = PlanningFixture();
    await mount(tester, f);
    await tester.enterText(
      find.byKey(const ValueKey('task-description')),
      'My unsaved draft',
    );
    f.task = {...f.task, 'description': 'Newer edit elsewhere', 'revision': 2};
    await tester.pump();
    await tester.tap(find.text('Save & close'));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsOneWidget);
    expect(
      tester
          .widget<TextField>(find.byKey(const ValueKey('task-description')))
          .controller!
          .text,
      'My unsaved draft',
    );
    await tester.ensureVisible(find.text('Discard my edits'));
    await tester.tap(find.text('Discard my edits'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextField>(find.byKey(const ValueKey('task-description')))
          .controller!
          .text,
      'Newer edit elsewhere',
    );
    await tester.tap(find.text('Close'));
    await tester.pumpAndSettle();
    expect(find.byType(AlertDialog), findsNothing);
    await tester.pumpWidget(const SizedBox());
    f.controller.dispose();
  });

  testWidgets(
    'simple inspector saves text before planning and closing does not cancel',
    (tester) async {
      final f = PlanningFixture();
      await mount(tester, f);
      expect(find.text('Normal'), findsNothing);
      expect(find.text('Idle'), findsNothing);
      expect(find.text('Use local App Server'), findsNothing);
      expect(find.text('Submission summary'), findsNothing);
      await tester.enterText(
        find.byKey(const ValueKey('task-description')),
        'Original GitHub description\nMy extra requirement',
      );
      await tester.tap(find.text('Generate with AI'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.text('Generating plan…'), findsOneWidget);
      final mutations = f.calls.where((op) => !op.containsKey('Get')).toList();
      expect(mutations[0].containsKey('UpdateDescription'), isTrue);
      expect(mutations[1]['GeneratePlan']['model'], 'chosen-model');
      await tester.tap(find.text('Close'));
      await tester.pumpAndSettle();
      expect(f.calls.any((op) => op.containsKey('CancelPlan')), isFalse);
      f.complete();
      await tester.tap(find.text('Open task'));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextField>(find.byKey(const ValueKey('task-description')))
            .controller!
            .text,
        'Original GitHub description\nMy extra requirement',
      );
      expect(
        tester
            .widget<TextField>(find.byKey(const ValueKey('task-plan')))
            .controller!
            .text,
        contains('Inspect the code'),
      );
      expect(f.controller.agents, isEmpty);
      expect(f.task['state'], 'Backlog');
      await tester.pumpWidget(const SizedBox());
      f.controller.dispose();
    },
  );

  testWidgets(
    'generation preserves an unsaved description and close saves it',
    (tester) async {
      final f = PlanningFixture();
      await mount(tester, f);
      await tester.tap(find.text('Generate with AI'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      await tester.enterText(
        find.byKey(const ValueKey('task-description')),
        'Still typing while AI works',
      );
      f.complete();
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<TextField>(find.byKey(const ValueKey('task-description')))
            .controller!
            .text,
        'Still typing while AI works',
      );
      await tester.ensureVisible(find.text('Save & close'));
      await tester.tap(find.text('Save & close'));
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      expect(f.task['description'], 'Still typing while AI works');
      expect((f.task['planning'] as Map)['text'], contains('Inspect the code'));
      await tester.pumpWidget(const SizedBox());
      f.controller.dispose();
    },
  );

  testWidgets('cancel is explicit and failures keep the original description', (
    tester,
  ) async {
    final f = PlanningFixture();
    await mount(tester, f);
    await tester.tap(find.text('Generate with AI'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(f.calls.where((op) => op.containsKey('CancelPlan')).length, 1);
    expect(find.text('Generate with AI'), findsOneWidget);
    expect(f.task['description'], 'Original GitHub description');
    await tester.pumpWidget(const SizedBox());
    f.controller.dispose();
  });
}
