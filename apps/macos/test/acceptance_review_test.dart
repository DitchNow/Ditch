import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/acceptance_review.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/data/task_models.dart';

TaskDto task([Map<String, dynamic> acceptance = const {}]) => TaskDto.fromJson({
  'id': 'task',
  'project_id': 'project',
  'title': 'Review task',
  'state': 'InReview',
  'condition': 'Idle',
  'acceptance': acceptance,
});
void main() {
  testWidgets('empty review is truthful and offers explicit revalidation', (
    tester,
  ) async {
    final calls = <Object>[];
    final controller = TaskBoardController(
      request: (request) async {
        calls.add(request);
        return {
          'TaskResponse': {
            'Error': {
              'code': 'InvalidInput',
              'message': 'Revalidation requires a summary',
            },
          },
        };
      },
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: AcceptanceReview(
              task: task(),
              controller: controller,
              running: false,
            ),
          ),
        ),
      ),
    );
    expect(
      find.textContaining('No immutable review submission'),
      findsOneWidget,
    );
    await tester.tap(find.text('Revalidate evidence'));
    await tester.pumpAndSettle();
    expect(
      ((calls.single as Map)['TaskRequest']['operation'] as Map).containsKey(
        'Revalidate',
      ),
      isTrue,
    );
    expect(controller.error, contains('summary'));
  });
  testWidgets(
    'review separates human criteria and recorded validator evidence',
    (tester) async {
      final controller = TaskBoardController(request: (_) async => {});
      final value = task({
        'current_submission': 'submission',
        'submissions': [
          {
            'id': 'submission',
            'criteria': [
              {
                'criterion': {'id': 'human', 'label': 'Design approved'},
                'status': 'HumanReview',
              },
              {
                'criterion': {'id': 'test', 'label': 'Unit tests'},
                'status': 'Passed',
                'output': '42 tests passed',
              },
            ],
            'workspace': {
              'head': 'abc',
              'dirty_paths': [' M app.dart'],
              'diff_stat': 'one file changed',
              'warnings': ['Ignored files excluded'],
            },
            'preexisting_dirty_paths': [],
            'approvals': [],
          },
        ],
        'attempts': [],
      });
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: AcceptanceReview(
                task: value,
                controller: controller,
                running: false,
              ),
            ),
          ),
        ),
      );
      expect(find.text('Design approved'), findsOneWidget);
      expect(find.text('Human assessment'), findsOneWidget);
      expect(find.text('Unit tests · Passed'), findsOneWidget);
      await tester.tap(find.byType(Checkbox));
      await tester.pump();
      expect(tester.widget<Checkbox>(find.byType(Checkbox)).value, isTrue);
      expect(find.text('Ignored files excluded'), findsOneWidget);
    },
  );
  testWidgets('policy defaults are bounded and saves typed configuration', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1000, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    Map? operation;
    final controller = TaskBoardController(
      request: (request) async {
        operation = (request as Map)['TaskRequest']['operation'] as Map;
        return {
          'TaskResponse': {
            'Changed': {
              'id': 'task',
              'project_id': 'project',
              'title': 'Task',
              'state': 'Running',
            },
          },
        };
      },
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () => showDialog<void>(
                context: context,
                builder: (_) =>
                    AcceptanceEditor(task: task(), controller: controller),
              ),
              child: const Text('Configure'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Configure'));
    await tester.pumpAndSettle();
    expect(find.text('Maximum attempts (1–10)'), findsOneWidget);
    await tester.tap(find.text('Save acceptance policy'));
    await tester.pumpAndSettle();
    final config = (operation!['ConfigureAcceptance'] as Map)['config'] as Map;
    expect(config['policy']['max_attempts'], 3);
    expect(config['policy']['enabled'], false);
    expect(config['policy']['stop_on_denial'], true);
    expect(config['policy']['retry_mode'], 'Fresh');
  });
  testWidgets(
    'running loop exposes cancellation while policy editing is disabled',
    (tester) async {
      final calls = <Object>[];
      final controller = TaskBoardController(
        request: (request) async {
          calls.add(request);
          return {
            'TaskResponse': {
              'Changed': {
                'id': 'task',
                'project_id': 'project',
                'title': 'Task',
                'state': 'Running',
              },
            },
          };
        },
      );
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SingleChildScrollView(
              child: AcceptanceReview(
                task: task(),
                controller: controller,
                running: true,
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Cancel loop'));
      await tester.pumpAndSettle();
      expect(
        ((calls.single as Map)['TaskRequest']['operation'] as Map).containsKey(
          'CancelLoop',
        ),
        true,
      );
    },
  );
}
