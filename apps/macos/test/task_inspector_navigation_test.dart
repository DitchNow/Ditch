import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/task_board.dart';
import 'task_agent_attachment_test.dart' show agent;
import 'task_queue_test.dart' show QueueFixture, task;

Future<QueueFixture> openInspector(
  WidgetTester tester, {
  bool inProgress = false,
  bool withAgent = false,
  bool longText = true,
  ValueChanged<String>? onOpenAgent,
}) async {
  final text = longText
      ? List.generate(90, (i) => 'Line $i of the task content.').join('\n')
      : 'Short description';
  final f = QueueFixture([
    {
      ...task('a', position: inProgress ? 1 : null),
      'description': text,
      'planning': {'text': text},
      if (withAgent) 'assigned_agent_id': 'alpha',
      if (withAgent && inProgress) 'condition': 'Running',
    },
  ], agents: withAgent ? [agent('alpha', taskId: 'a')] : []);
  tester.view.physicalSize = const Size(1000, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(f.controller.dispose);
  f.controller.replaceSnapshot({'tasks': f.rows, 'agents': f.agents});
  await tester.pumpWidget(
    MaterialApp(
      theme: DitchTheme.light().copyWith(platform: TargetPlatform.macOS),
      home: Builder(
        builder: (context) => Scaffold(
          body: TextButton(
            onPressed: () => showTaskInspector(
              context,
              f.controller,
              f.controller.task('a')!,
              const [TaskProjectOption('project', 'Project')],
              () => {},
              onOpenAgent ?? (_) {},
            ),
            child: const Text('Open'),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.text('Open'));
  await tester.pumpAndSettle();
  return f;
}

ScrollableState fieldScroll(WidgetTester tester, String key) =>
    tester.state<ScrollableState>(
      find.descendant(
        of: find.byKey(ValueKey(key)),
        matching: find.byType(Scrollable),
      ),
    );

ScrollableState modalScroll(WidgetTester tester) =>
    tester.state<ScrollableState>(
      find
          .descendant(
            of: find.byType(SingleChildScrollView),
            matching: find.byType(Scrollable),
          )
          .first,
    );

Offset fieldPoint(WidgetTester tester, String key) => tester
    .getRect(find.byKey(ValueKey(key)))
    .intersect(tester.getRect(find.byType(SingleChildScrollView)))
    .center;

Future<void> wheel(WidgetTester tester, Offset point, double delta) async {
  await tester.sendEventToBinding(
    PointerScrollEvent(position: point, scrollDelta: Offset(0, delta)),
  );
  await tester.pump();
}

void main() {
  for (final key in ['task-description', 'task-plan']) {
    testWidgets('$key hands wheel scrolling to modal at both boundaries', (
      tester,
    ) async {
      await openInspector(tester);
      final outer = modalScroll(tester).position;
      final inner = fieldScroll(tester, key).position;
      outer.jumpTo(outer.maxScrollExtent / 2);
      await tester.pumpAndSettle();
      final point = fieldPoint(tester, key);
      inner.jumpTo(inner.maxScrollExtent / 2);
      final before = outer.pixels;
      final innerBefore = inner.pixels;
      await wheel(tester, point, 30);
      expect(inner.pixels, innerBefore + 30);
      expect(outer.pixels, before);
      inner.jumpTo(inner.maxScrollExtent);
      await tester.pump();
      final downBefore = outer.pixels;
      await wheel(tester, fieldPoint(tester, key), 30);
      expect(outer.pixels, greaterThan(downBefore));
      inner.jumpTo(inner.minScrollExtent);
      final upBefore = outer.pixels;
      await wheel(tester, fieldPoint(tester, key), -20);
      expect(outer.pixels, lessThan(upBefore));
      expect(tester.takeException(), isNull);
    });

    for (final direction in [1.0, -1.0]) {
      testWidgets(
        '$key hands trackpad scrolling to modal (direction $direction)',
        (tester) async {
          await openInspector(tester);
          final outer = modalScroll(tester).position;
          final inner = fieldScroll(tester, key).position;
          outer.jumpTo(outer.maxScrollExtent / 2);
          await tester.pumpAndSettle();
          final boundary = direction > 0
              ? inner.maxScrollExtent
              : inner.minScrollExtent;
          inner.jumpTo(boundary - direction * 20);
          final before = outer.pixels;
          final gesture = await tester.createGesture(
            kind: PointerDeviceKind.trackpad,
          );
          final point = fieldPoint(tester, key);
          await gesture.panZoomStart(point);
          await gesture.panZoomUpdate(point, pan: Offset(0, -direction * 10));
          await gesture.panZoomUpdate(point, pan: Offset(0, -direction * 110));
          await gesture.panZoomUpdate(point, pan: Offset(0, -direction * 150));
          await tester.pump();
          expect(inner.pixels, boundary);
          // Only the unconsumed 20 pixels reach the modal.
          expect(outer.pixels, closeTo(before + direction * 20, 0.01));
          // Continuing the same gesture stays with the modal at the boundary.
          await gesture.panZoomUpdate(point, pan: Offset(0, -direction * 180));
          await tester.pump();
          expect(outer.pixels, closeTo(before + direction * 50, 0.01));
          // Reversing scrolls the text again, not both regions.
          final parentAfter = outer.pixels;
          await gesture.panZoomUpdate(point, pan: Offset(0, -direction * 160));
          await tester.pump();
          expect(inner.pixels, closeTo(boundary - direction * 20, 0.01));
          expect(outer.pixels, parentAfter);
          // A large continuation stops at the modal's own boundary.
          await gesture.panZoomUpdate(
            point,
            pan: Offset(0, -direction * 10000),
          );
          await tester.pump();
          expect(
            outer.pixels,
            direction > 0 ? outer.maxScrollExtent : outer.minScrollExtent,
          );
          await gesture.panZoomEnd();
          await tester.pumpAndSettle();
          expect(tester.takeException(), isNull);
        },
      );
    }
  }

  testWidgets(
    'Go to agent closes the modal and opens the attached conversation',
    (tester) async {
      String? opened;
      await openInspector(
        tester,
        inProgress: true,
        withAgent: true,
        onOpenAgent: (id) => opened = id,
      );
      final button = find.byTooltip('Go to agent');
      expect(button.hitTestable(), findsOneWidget);
      final assignment = find.ancestor(
        of: button,
        matching: find.byType(ListTile),
      );
      final name = find.descendant(
        of: assignment,
        matching: find.text('Agent alpha'),
      );
      expect(name, findsOneWidget);
      expect(tester.getRect(button).right, lessThan(tester.getRect(name).left));
      expect(find.text('Go to agent'), findsNothing);
      await tester.tap(button);
      await tester.pumpAndSettle();
      expect(opened, 'alpha');
      expect(find.byType(AlertDialog), findsNothing);
    },
  );

  testWidgets('Go to agent is absent for a queued task without an agent', (
    tester,
  ) async {
    await openInspector(tester, inProgress: true);
    expect(find.byTooltip('Go to agent'), findsNothing);
  });

  testWidgets('Go to agent is absent in Todo', (tester) async {
    await openInspector(tester, withAgent: true);
    expect(find.byTooltip('Go to agent'), findsNothing);
  });
}
