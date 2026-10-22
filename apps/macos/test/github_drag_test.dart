import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/github_inbox.dart';
import 'package:the_ditch/task_board.dart';

class Fixture {
  bool connected = true, skip = false;
  final imports = <Map>[];
  final mutations = <Map>[];
  late final controller = TaskBoardController(request: request);
  Map<String, dynamic> task(String state) => {
    'id': 'task',
    'project_id': 'project',
    'title': 'Board task',
    'description': '',
    'state': state,
    'condition': 'Idle',
    'priority': 'Normal',
    'acceptance_criteria': ['Reviewed'],
    'revision': 1,
    'order_key': 1024,
    'archived': false,
  };
  Future<Map<String, dynamic>> request(Object request) async {
    final body = request as Map;
    final op = body['GitHub'];
    if (op == 'Status') {
      return {
        'GitHub': {
          'connected': connected,
          'generation': connected ? 'current' : 'disconnected',
        },
      };
    }
    if (op is Map && op.containsKey('Links')) {
      return {
        'GitHub': {
          'repositories': [
            {
              'project_id': 'project',
              'repository': {'id': 42, 'full_name': 'owner/repo'},
            },
          ],
        },
      };
    }
    if (op is Map && op.containsKey('Issues')) {
      return {
        'GitHub': {
          'issues': [
            for (var n = 1; n <= 3; n++)
              {
                'id': n,
                'number': n,
                'title': 'Issue $n',
                'state': 'open',
                'body': 'Details $n',
                'html_url': 'https://github.com/owner/repo/issues/$n',
              },
          ],
          'imported': {},
          'has_more': false,
        },
      };
    }
    if (op is Map && op.containsKey('Comments')) {
      return {
        'GitHub': {'comments': [], 'has_more': false},
      };
    }
    if (op is Map && op.containsKey('Import')) {
      imports.add(op['Import'] as Map);
      return {'GitHub': {}};
    }
    if (body['TaskRequest'] case final Map req) {
      if (req['operation'] case final Map operation) {
        if (operation.containsKey('Move')) {
          mutations.add(operation);
          return {
            'TaskResponse': {'Changed': task('Ready')},
          };
        }
      }
    }
    return {
      'TaskResponse': {'Tasks': [], 'BoardProjects': []},
    };
  }

  Future<void> mount(WidgetTester tester, {bool boardTask = false}) async {
    controller.replaceSnapshot({
      'tasks': boardTask ? [task('Backlog')] : [],
      'agents': [],
    });
    await tester.pumpWidget(
      MaterialApp(
        theme: ThemeData(extensions: const [DitchTokens.light]),
        home: Scaffold(
          body: TaskBoard(
            controller: controller,
            projects: const [TaskProjectOption('project', 'Project')],
            connected: true,
            executionProfile: () => {},
            onOpenAgent: (_) {},
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(OutlinedButton, 'GitHub'));
    await tester.pumpAndSettle();
  }
}

Future<TestGesture> dragIssue(
  WidgetTester tester,
  int number,
  String column,
) async {
  final gesture = await tester.startGesture(
    tester.getCenter(find.text('Issue $number')),
  );
  await gesture.moveBy(const Offset(15, 0));
  await tester.pump();
  await gesture.moveTo(
    tester.getCenter(find.byKey(ValueKey('board-column-$column'))),
  );
  await tester.pump(const Duration(milliseconds: 200));
  return gesture;
}

void main() {
  const channel = MethodChannel('the_ditch/application');
  late Fixture fixture;
  setUp(() {
    fixture = Fixture();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          if (call.method == 'getSkipGitHubImportConfirmation') {
            return fixture.skip;
          }
          if (call.method == 'setSkipGitHubImportConfirmation') {
            fixture.skip = call.arguments as bool;
            return null;
          }
          return null;
        });
  });
  tearDown(() {
    fixture.controller.dispose();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });
  testWidgets(
    'shift-select drags a group, confirms once, and persists the skip choice',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1600, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await fixture.mount(tester);
      expect(find.byType(Checkbox), findsNothing);
      await tester.tap(find.text('Issue 1'));
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.tap(find.text('Issue 3'));
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.pump();
      expect(find.text('3 selected · Drag to Backlog'), findsOneWidget);
      final gesture = await dragIssue(tester, 2, 'backlog');
      expect(find.text('3 GitHub issues'), findsOneWidget);
      expect(find.text('Drop into Backlog'), findsOneWidget);
      await gesture.up();
      await tester.pumpAndSettle();
      expect(fixture.imports, isEmpty);
      expect(find.text('Add 3 issues to Backlog?'), findsOneWidget);
      expect(
        tester
            .getSize(
              find
                  .descendant(
                    of: find.byType(AlertDialog),
                    matching: find.byType(Material),
                  )
                  .last,
            )
            .width,
        lessThan(500),
      );
      await tester.tap(find.text('Don’t show this again'));
      await tester.tap(find.text('Add to Backlog'));
      await tester.pumpAndSettle();
      expect(fixture.imports.single['numbers'], [1, 2, 3]);
      expect(fixture.skip, isTrue);
      await tester.pumpWidget(const SizedBox());
      await fixture.mount(tester);
      final next = await dragIssue(tester, 1, 'backlog');
      await next.up();
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      expect(fixture.imports.length, 2);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets(
    'invalid target and canceled confirmation do not import or save preferences',
    (tester) async {
      await tester.binding.setSurfaceSize(const Size(1600, 900));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await fixture.mount(tester);
      final rejected = await dragIssue(tester, 1, 'todo');
      expect(find.text('Drop into Todo'), findsNothing);
      await rejected.up();
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      final accepted = await dragIssue(tester, 1, 'backlog');
      await accepted.up();
      await tester.pumpAndSettle();
      await tester.tap(find.text('Don’t show this again'));
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(fixture.imports, isEmpty);
      expect(fixture.skip, isFalse);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets('stale drag cannot import after disconnect', (tester) async {
    await tester.binding.setSurfaceSize(const Size(1600, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await fixture.mount(tester);
    final data = tester
        .widget<Draggable<GitHubIssueDrag>>(
          find.byKey(const ValueKey('github-issue-1')),
        )
        .data!;
    fixture.connected = false;
    await tester.tap(find.byIcon(Icons.refresh));
    await tester.pumpAndSettle();
    expect(data.canDrop(), isFalse);
    await data.importToBacklog();
    await tester.pumpAndSettle();
    expect(fixture.imports, isEmpty);
    expect(find.byType(AlertDialog), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets('board task drags between columns with destination feedback', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(1600, 900));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await fixture.mount(tester, boardTask: true);
    final gesture = await tester.startGesture(
      tester.getCenter(find.text('Board task')),
    );
    await gesture.moveBy(const Offset(15, 0));
    await tester.pump();
    await gesture.moveTo(
      tester.getCenter(find.byKey(const ValueKey('board-column-todo'))),
    );
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text('Drop into Todo'), findsOneWidget);
    await gesture.up();
    await tester.pumpAndSettle();
    expect(fixture.mutations.single['Move']['column'], 'Todo');
    expect(fixture.controller.task('task')!.column, TaskColumn.todo);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
}
