import 'dart:async';
import 'dart:ui' show Tristate;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/task_board.dart';
import 'package:the_ditch/widgets/task_release_dialog.dart';

Map<String, dynamic> row(
  String id, {
  String project = 'p',
  String state = 'InReview',
  String? release,
}) => {
  'id': id,
  'project_id': project,
  'title': 'Task $id',
  'state': state,
  'condition': 'Idle',
  'revision': 1,
  'release_id': release,
};
Map<String, dynamic> label(String id, String project, String name) => {
  'id': id,
  'project_id': project,
  'name': name,
  'created_at': '2026-10-29T12:00:00Z',
};

class Fixture {
  Fixture(this.rows, [this.labels = const []]) {
    controller.replaceSnapshot({'tasks': rows, 'releases': labels});
  }
  List<Map<String, dynamic>> rows, labels;
  final calls = <Map>[];
  bool failSecond = false, loseResponse = false;
  late final controller = TaskBoardController(
    request: (request) async {
      final payload = (request as Map)['TaskRequest'] as Map;
      calls.add(payload);
      final operation = payload['operation'];
      if (operation == 'ListReleases') {
        return {
          'TaskResponse': {
            'Releases': labels
                .where((r) => r['project_id'] == payload['project_id'])
                .toList(),
          },
        };
      }
      final op = operation as Map;
      if (op.containsKey('BoardProjects')) {
        return {
          'TaskResponse': {'BoardProjects': []},
        };
      }
      if (op.containsKey('BoardExecution')) {
        return {
          'TaskResponse': {
            'BoardExecution': {
              'project_id': payload['project_id'],
              'paused': false,
              'revision': 1,
            },
          },
        };
      }
      if (op.containsKey('List')) {
        return {
          'TaskResponse': {'Tasks': rows},
        };
      }
      if (failSecond && payload['project_id'] == 'q') {
        failSecond = false;
        throw TimeoutException('Response unavailable');
      }
      if (loseResponse) {
        loseResponse = false;
        throw TimeoutException('Response unavailable');
      }
      final accepting = op.containsKey('AcceptRelease');
      final batch = (op['AcceptRelease'] ?? op['AssignRelease']) as Map;
      final destination = batch['destination'];
      String? release;
      final created = <Map<String, dynamic>>[];
      if (destination is Map) {
        release = destination['Existing'] as String?;
        if (destination.containsKey('New')) {
          release = 'created-${payload['project_id']}';
          created.add(
            label(
              release,
              payload['project_id'] as String,
              destination['New'] as String,
            ),
          );
          labels = [...labels, ...created];
        }
      }
      final ids = (batch['tasks'] as List)
          .map((v) => (v as Map)['task_id'])
          .toSet();
      final changed = <Map<String, dynamic>>[];
      rows = [
        for (final r in rows)
          if (ids.contains(r['id']))
            (() {
              final next = {
                ...r,
                'state': accepting ? 'Accepted' : r['state'],
                'release_id': release,
                'revision': (r['revision'] as int) + 1,
              };
              changed.add(next);
              return next;
            })()
          else
            r,
      ];
      return {
        'TaskResponse': {
          'ReleaseBatch': {'tasks': changed, 'releases': created},
        },
      };
    },
  );
  List<Map> get mutations => calls
      .where(
        (c) =>
            c['operation'] is Map &&
            ((c['operation'] as Map).containsKey('AcceptRelease') ||
                (c['operation'] as Map).containsKey('AssignRelease')),
      )
      .toList();
}

Future<void> dialog(WidgetTester tester, Fixture f, {String? selected}) async {
  await tester.pumpWidget(
    MaterialApp(
      home: Builder(
        builder: (context) => Scaffold(
          body: TextButton(
            onPressed: () => showTaskReleaseDialog(
              context,
              controller: f.controller,
              tasks: f.controller.tasks,
              projectNames: const {'p': 'First', 'q': 'Second'},
              preselectedRelease: selected,
            ),
            child: const Text('Open'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('Open'));
  await tester.pumpAndSettle();
}

Future<void> board(WidgetTester tester, Fixture f, {String? project}) async {
  await tester.binding.setSurfaceSize(const Size(1800, 1100));
  addTearDown(() => tester.binding.setSurfaceSize(null));
  await tester.pumpWidget(
    MaterialApp(
      theme: ThemeData(extensions: const [DitchTokens.light]),
      home: Scaffold(
        body: TaskBoard(
          controller: f.controller,
          projects: const [
            TaskProjectOption('p', 'First'),
            TaskProjectOption('q', 'Second'),
          ],
          initialProjectId: project,
          connected: true,
          executionProfile: () => {},
          onOpenAgent: (_) {},
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

Future<void> selectCard(
  WidgetTester tester,
  String id, {
  LogicalKeyboardKey? modifier,
}) async {
  if (modifier != null) await tester.sendKeyDownEvent(modifier);
  await tester.tap(find.text('Task $id'));
  if (modifier != null) await tester.sendKeyUpEvent(modifier);
  // A single click waits for the double-click inspector gesture to expire.
  await tester.pump(const Duration(milliseconds: 350));
  await tester.pumpAndSettle();
}

Set<String> selectedCards(WidgetTester tester) => tester
    .widgetList<TaskCard>(find.byType(TaskCard))
    .where((card) => card.selected)
    .map((card) => card.task.id)
    .toSet();

void main() {
  testWidgets(
    'release headers support keyboard expansion and ranges skip collapsed tasks',
    (tester) async {
      final f = Fixture(
        [
          row('a', state: 'Accepted', release: 'r1'),
          row('b', state: 'Accepted', release: 'r2'),
          row('c', state: 'Accepted'),
        ],
        [label('r1', 'p', 'First release'), label('r2', 'p', 'Second release')],
      );
      addTearDown(f.controller.dispose);
      final semantics = tester.ensureSemantics();
      await board(tester, f, project: 'p');
      final header = find.text('Second release (1)');
      expect(
        tester.getSemantics(header).flagsCollection.isExpanded,
        Tristate.isTrue,
      );
      Focus.of(tester.element(header)).requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(find.text('Task b'), findsNothing);
      expect(
        tester.getSemantics(header).flagsCollection.isExpanded,
        Tristate.isFalse,
      );
      await selectCard(tester, 'a');
      await selectCard(tester, 'c', modifier: LogicalKeyboardKey.shiftLeft);
      expect(selectedCards(tester), {'a', 'c'});
      expect(
        tester
            .getSemantics(
              find.byWidgetPredicate((w) => w is TaskCard && w.task.id == 'c'),
            )
            .flagsCollection
            .isSelected,
        Tristate.isTrue,
      );
      Focus.of(tester.element(header)).requestFocus();
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(find.text('Task b'), findsOneWidget);
      expect(selectedCards(tester), {'a', 'c'});
      semantics.dispose();
    },
  );

  testWidgets(
    'release cards select ranges, toggle, clear and stay within one column',
    (tester) async {
      final f = Fixture([
        row('a'),
        row('b'),
        row('c'),
        row('d', state: 'Accepted'),
      ]);
      addTearDown(f.controller.dispose);
      await board(tester, f, project: 'p');
      expect(find.text('Select for release'), findsNothing);
      expect(find.text('Selected for release'), findsNothing);
      expect(find.byType(CheckboxListTile), findsNothing);
      await selectCard(tester, 'a');
      expect(selectedCards(tester), {'a'});
      expect(find.byType(AlertDialog), findsNothing);
      await selectCard(tester, 'c', modifier: LogicalKeyboardKey.shiftLeft);
      expect(selectedCards(tester), {'a', 'b', 'c'});
      await selectCard(tester, 'b', modifier: LogicalKeyboardKey.metaLeft);
      expect(selectedCards(tester), {'a', 'c'});
      await selectCard(tester, 'b', modifier: LogicalKeyboardKey.controlLeft);
      expect(selectedCards(tester), {'a', 'b', 'c'});
      await selectCard(tester, 'd', modifier: LogicalKeyboardKey.shiftLeft);
      expect(selectedCards(tester), {'d'});
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(selectedCards(tester), isEmpty);
      await selectCard(tester, 'b', modifier: LogicalKeyboardKey.shiftLeft);
      expect(selectedCards(tester), {'b'});
      expect(f.mutations, isEmpty);
    },
  );

  testWidgets(
    'Done groups collapse independently, retain state and clear hidden selection',
    (tester) async {
      final f = Fixture(
        [
          row('a', state: 'Accepted', release: 'rp'),
          row('b', project: 'q', state: 'Accepted', release: 'rq'),
          row('c', state: 'Accepted'),
        ],
        [label('rp', 'p', 'v1'), label('rq', 'q', 'v1')],
      );
      addTearDown(f.controller.dispose);
      await board(tester, f);
      await selectCard(tester, 'a');
      final toggle = find.byKey(const ValueKey('release-group-toggle-p-rp'));
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(find.text('Task a'), findsNothing);
      expect(find.text('Task b'), findsOneWidget);
      expect(find.text('Task c'), findsOneWidget);
      expect(selectedCards(tester), isEmpty);
      f.controller.applyEvent({
        'TaskChanged': {...f.rows.first, 'revision': 2},
      });
      await tester.pumpAndSettle();
      expect(find.text('Task a'), findsNothing);
      await tester.tap(toggle);
      await tester.pumpAndSettle();
      expect(find.text('Task a'), findsOneWidget);
      expect(selectedCards(tester), isEmpty);
      await tester.tap(
        find.byKey(const ValueKey('release-group-toggle-p-other')),
      );
      await tester.pumpAndSettle();
      expect(find.text('Task c'), findsNothing);
      expect(find.text('Other Tasks (1)'), findsOneWidget);
      expect(f.mutations, isEmpty);
    },
  );

  testWidgets(
    'Done ranges follow release order and batch drop onto a collapsed header',
    (tester) async {
      final f = Fixture(
        [
          row('a', state: 'Accepted'),
          row('b', state: 'Accepted', release: 'r2'),
          row('c', state: 'Accepted', release: 'r1'),
          row('d', state: 'Accepted', release: 'r1'),
        ],
        [label('r1', 'p', 'First release'), label('r2', 'p', 'Second release')],
      );
      addTearDown(f.controller.dispose);
      await board(tester, f, project: 'p');
      await selectCard(tester, 'd');
      await selectCard(tester, 'b', modifier: LogicalKeyboardKey.shiftLeft);
      // Display order is c, d, b, a; controller order is a, b, c, d.
      expect(selectedCards(tester), {'d', 'b'});
      final target = find.byKey(const ValueKey('release-group-toggle-p-other'));
      await tester.tap(target);
      await tester.pumpAndSettle();
      final source = tester.getCenter(find.text('Task d'));
      await tester.dragFrom(source, tester.getCenter(target) - source);
      await tester.pumpAndSettle();
      final batch =
          (f.mutations.single['operation'] as Map)['AssignRelease'] as Map;
      expect((batch['tasks'] as List).map((t) => t['task_id']).toSet(), {
        'd',
        'b',
      });
      expect(batch['destination'], 'Other');
      expect(find.text('Other Tasks (3)'), findsOneWidget);
      expect(find.text('Task d'), findsNothing);
      expect(selectedCards(tester), isEmpty);
    },
  );

  testWidgets(
    'Review ranges skip ineligible tasks and stale anchors are discarded',
    (tester) async {
      final f = Fixture([
        row('a'),
        {...row('b'), 'condition': 'Running'},
        row('c'),
        row('d', project: 'q'),
      ]);
      addTearDown(f.controller.dispose);
      await board(tester, f);
      await selectCard(tester, 'a');
      await selectCard(tester, 'c', modifier: LogicalKeyboardKey.shiftLeft);
      expect(selectedCards(tester), {'a', 'c'});
      f.controller.applyEvent({
        'TaskChanged': {...row('a', state: 'Accepted'), 'revision': 2},
      });
      await tester.pumpAndSettle();
      expect(selectedCards(tester), {'c'});
      await selectCard(tester, 'd', modifier: LogicalKeyboardKey.shiftLeft);
      expect(selectedCards(tester), {'d'});
      await tester.tap(find.byType(DropdownButton<String>));
      await tester.pumpAndSettle();
      await tester.tap(find.text('First').last);
      await tester.pumpAndSettle();
      expect(selectedCards(tester), isEmpty);
      await selectCard(tester, 'c', modifier: LogicalKeyboardKey.shiftLeft);
      expect(selectedCards(tester), {'c'});
    },
  );

  testWidgets('Review batches can be accepted into collapsed releases', (
    tester,
  ) async {
    final f = Fixture([row('a'), row('b')], [label('rp', 'p', 'v1')]);
    addTearDown(f.controller.dispose);
    await board(tester, f, project: 'p');
    final target = find.byKey(const ValueKey('release-group-toggle-p-rp'));
    await tester.tap(target);
    await tester.pumpAndSettle();
    await selectCard(tester, 'a');
    await selectCard(tester, 'b', modifier: LogicalKeyboardKey.shiftLeft);
    final source = tester.getCenter(find.text('Task a'));
    await tester.dragFrom(source, tester.getCenter(target) - source);
    await tester.pumpAndSettle();
    expect(find.textContaining('for 2 task(s)'), findsOneWidget);
    expect(f.mutations, isEmpty);
    await tester.tap(find.text('Accept and move'));
    await tester.pumpAndSettle();
    expect(
      f.controller.tasks.every(
        (t) => t.releaseId == 'rp' && t.column == TaskColumn.done,
      ),
      isTrue,
    );
    expect(find.text('v1 (2)'), findsOneWidget);
    expect(find.text('Task a'), findsNothing);
    expect(selectedCards(tester), isEmpty);
  });

  test('snapshot, events, and deletion preserve project isolation', () {
    final f = Fixture(
      [row('a', state: 'Accepted', release: 'rp')],
      [label('rp', 'p', 'v1'), label('rq', 'q', 'v1')],
    );
    addTearDown(f.controller.dispose);
    expect(f.controller.projectReleases('p').single.id, 'rp');
    f.controller.replaceSnapshot({
      'tasks': [row('a', state: 'Accepted', release: 'rq')],
      'releases': [label('rq', 'q', 'v1')],
    });
    expect(f.controller.releases.keys, ['rq']);
    f.controller.applyEvent({'ReleaseCreated': label('rp', 'p', 'v1')});
    f.controller.applyEvent({
      'ProjectDeleted': {'project_id': 'p'},
    });
    expect(f.controller.releases.keys, ['rq']);
    expect(f.controller.tasks, isEmpty);
  });

  testWidgets('cancel performs no mutations', (tester) async {
    final f = Fixture([row('a')]);
    addTearDown(f.controller.dispose);
    await dialog(tester, f);
    expect(find.text('Other Tasks'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(f.mutations, isEmpty);
    expect(f.controller.task('a')!.column, TaskColumn.inReview);
  });

  testWidgets('existing release accepts a batch only after confirmation', (
    tester,
  ) async {
    final f = Fixture([row('a'), row('b')], [label('rp', 'p', 'v1')]);
    addTearDown(f.controller.dispose);
    await dialog(tester, f, selected: 'rp');
    expect(f.mutations, isEmpty);
    await tester.tap(find.text('Accept and move'));
    await tester.pumpAndSettle();
    final batch =
        (f.mutations.single['operation'] as Map)['AcceptRelease'] as Map;
    expect(batch['destination'], {'Existing': 'rp'});
    expect((batch['tasks'] as List).length, 2);
    expect(
      f.controller.tasks.every(
        (t) => t.releaseId == 'rp' && t.column == TaskColumn.done,
      ),
      isTrue,
    );
  });

  testWidgets('new labels accept arbitrary names and reject duplicates', (
    tester,
  ) async {
    final f = Fixture([row('a')], [label('rp', 'p', 'v1')]);
    addTearDown(f.controller.dispose);
    await dialog(tester, f);
    await tester.tap(find.byKey(const ValueKey('release-choice-p')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Create new release…').last);
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('release-name-p')),
      ' v1 ',
    );
    await tester.pump();
    expect(find.text('This release exists. Select it above.'), findsOneWidget);
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Accept and move'),
          )
          .onPressed,
      isNull,
    );
    await tester.enterText(
      find.byKey(const ValueKey('release-name-p')),
      ' Autumn preview ',
    );
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pumpAndSettle();
    expect(
      f.controller.projectReleases('p').any((r) => r.name == 'Autumn preview'),
      isTrue,
    );
  });

  testWidgets(
    'mixed project failures retry only unresolved batch with same ID',
    (tester) async {
      final f = Fixture([row('a'), row('b', project: 'q')])..failSecond = true;
      addTearDown(f.controller.dispose);
      await dialog(tester, f);
      await tester.tap(find.text('Accept and move'));
      await tester.pumpAndSettle();
      expect(find.text('Updated successfully'), findsOneWidget);
      expect(f.mutations.length, 2);
      await tester.tap(find.text('Retry pending update'));
      await tester.pumpAndSettle();
      expect(f.mutations.length, 3);
      expect(f.mutations[2], f.mutations[1]);
      expect(
        f.controller.tasks.every((t) => t.column == TaskColumn.done),
        isTrue,
      );
    },
  );

  testWidgets('Done groups retain empty releases and Other Tasks per project', (
    tester,
  ) async {
    final f = Fixture(
      [
        row('a', state: 'Accepted', release: 'rp'),
        row('b', project: 'q', state: 'Accepted'),
      ],
      [label('rp', 'p', 'v1'), label('rq', 'q', 'v1')],
    );
    addTearDown(f.controller.dispose);
    await board(tester, f);
    expect(find.text('v1 (1)'), findsOneWidget);
    expect(find.text('v1 (0)'), findsOneWidget);
    expect(find.text('Other Tasks (1)'), findsOneWidget);
    expect(find.text('Other Tasks (0)'), findsOneWidget);
    final source = tester.getCenter(find.text('Task a'));
    final target = tester.getCenter(find.text('Other Tasks (0)'));
    await tester.dragFrom(source, target - source);
    await tester.pumpAndSettle();
    expect(f.mutations.length, 1);
    expect(
      (f.mutations.single['operation'] as Map).containsKey('AssignRelease'),
      isTrue,
    );
    expect(f.controller.task('a')!.releaseId, isNull);
  });

  testWidgets(
    'selected Review cards drag as one batch and cancellation leaves both in Review',
    (tester) async {
      final f = Fixture([row('a'), row('b')]);
      addTearDown(f.controller.dispose);
      await board(tester, f, project: 'p');
      await selectCard(tester, 'a');
      await selectCard(tester, 'b', modifier: LogicalKeyboardKey.shiftLeft);
      final source = tester.getCenter(find.text('Task a'));
      final target = tester.getCenter(find.text('Other Tasks (0)'));
      await tester.dragFrom(source, target - source);
      await tester.pumpAndSettle();
      expect(find.textContaining('for 2 task(s)'), findsOneWidget);
      expect(f.mutations, isEmpty);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(
        f.controller.tasks.every((t) => t.column == TaskColumn.inReview),
        isTrue,
      );
    },
  );
  testWidgets(
    'cross-project and no-op group drops do not reach column acceptance',
    (tester) async {
      final f = Fixture(
        [row('a', state: 'Accepted', release: 'rp'), row('b')],
        [label('rp', 'p', 'v1'), label('rq', 'q', 'v1')],
      );
      addTearDown(f.controller.dispose);
      await board(tester, f);
      var source = tester.getCenter(find.text('Task a'));
      var target = tester.getCenter(find.text('v1 (0)'));
      await tester.dragFrom(source, target - source);
      await tester.pumpAndSettle();
      expect(f.mutations, isEmpty);
      expect(find.byType(AlertDialog), findsNothing);
      source = tester.getCenter(find.text('Task a'));
      target = tester.getCenter(find.text('v1 (1)'));
      await tester.dragFrom(source, target - source);
      await tester.pumpAndSettle();
      expect(f.mutations, isEmpty);
      expect(find.byType(AlertDialog), findsNothing);
      source = tester.getCenter(find.text('Task b'));
      target = tester.getCenter(find.text('v1 (0)'));
      await tester.dragFrom(source, target - source);
      await tester.pumpAndSettle();
      expect(f.mutations, isEmpty);
      expect(find.byType(AlertDialog), findsNothing);
    },
  );

  testWidgets(
    'unselected Review card moves alone with destination preselected',
    (tester) async {
      final f = Fixture([row('a'), row('b')], [label('rp', 'p', 'v1')]);
      addTearDown(f.controller.dispose);
      await board(tester, f, project: 'p');
      await selectCard(tester, 'a');
      final source = tester.getCenter(find.text('Task b'));
      final target = tester.getCenter(find.text('v1 (0)'));
      await tester.dragFrom(source, target - source);
      await tester.pumpAndSettle();
      expect(find.textContaining('for 1 task(s)'), findsOneWidget);
      await tester.tap(find.text('Accept and move'));
      await tester.pumpAndSettle();
      final batch =
          (f.mutations.single['operation'] as Map)['AcceptRelease'] as Map;
      expect(batch['destination'], {'Existing': 'rp'});
      expect((batch['tasks'] as List).single['task_id'], 'b');
      expect(f.controller.task('a')!.column, TaskColumn.inReview);
    },
  );

  test(
    'unconfirmed release requests block unrelated mutations until resolved',
    () async {
      final f = Fixture([row('a')])..loseResponse = true;
      addTearDown(f.controller.dispose);
      await f.controller.releaseBatch(
        f.controller.tasks,
        'Other',
        accept: true,
      );
      expect(f.controller.hasPendingRelease, isTrue);
      await f.controller.mutate('p', {
        'Delete': {'task_id': 'a', 'expected_revision': 1},
      });
      expect(f.calls.length, 1);
      await f.controller.retry();
      expect(f.mutations[0], f.mutations[1]);
      expect(f.controller.task('a')!.column, TaskColumn.done);
    },
  );
}
