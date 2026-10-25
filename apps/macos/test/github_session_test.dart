import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/github_inbox.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/task_board.dart';
import 'package:the_ditch/application/task_board_controller.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

class SessionFixture {
  int statuses = 0, links = 0, issueReads = 0, syncs = 0;
  bool connected = true, failIssues = false;
  String generation = 'one';
  Completer<void>? pending;
  Future<Map<String, dynamic>> request(Object req) async {
    if ((req as Map)['TaskRequest'] case final Map task) {
      return {
        'TaskResponse': (task['operation'] as Map).containsKey('BoardProjects')
            ? {'BoardProjects': []}
            : {'Tasks': []},
      };
    }
    final op = req['GitHub'];
    if (op == 'Status') {
      statuses++;
      return {
        'GitHub': {'connected': connected, 'generation': generation},
      };
    }
    if (op == 'Disconnect') {
      connected = false;
      return {'GitHub': {}};
    }
    if (op is Map && op.containsKey('Links')) {
      links++;
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
      issueReads++;
      await pending?.future;
      if (failIssues) {
        return {
          'Error': {'message': 'Offline'},
        };
      }
      return {
        'GitHub': {
          'issues': [
            {
              'id': 7,
              'number': 7,
              'title': 'Issue $issueReads',
              'state': 'open',
            },
          ],
          'imported': {},
          'has_more': false,
        },
      };
    }
    if (op is Map && op.containsKey('SyncChanges')) {
      syncs++;
      return {
        'GitHub': {'cursor': 4, 'repository_ids': [], 'refresh_failures': 0},
      };
    }
    if (op is Map && op.containsKey('Import')) {
      return {
        'GitHub': {
          'task_ids': ['imported-task'],
        },
      };
    }
    throw StateError('Unexpected operation: $op');
  }

  Future<void> mount(WidgetTester tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 350,
            child: GitHubInbox(
              request: request,
              projects: const [TaskProjectOption('project', 'Project')],
              onImported: () async {},
            ),
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
  }
}

void main() {
  testWidgets('connected GitHub settings do not poll while idle', (
    tester,
  ) async {
    final f = SessionFixture();
    await tester.pumpWidget(
      MaterialApp(home: GitHubSettingsDialog(request: f.request)),
    );
    await tester.pumpAndSettle();
    expect(f.statuses, 1);
    await tester.pump(const Duration(minutes: 30));
    await tester.pumpAndSettle();
    expect(f.statuses, 1);
    expect(f.issueReads, 0);
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets(
    'opening Kanban loads once even before its GitHub pane is expanded',
    (tester) async {
      final f = SessionFixture();
      final controller = TaskBoardController(request: f.request);
      controller.replaceSnapshot({'tasks': [], 'agents': []});
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: TaskBoard(
              controller: controller,
              projects: const [TaskProjectOption('project', 'Project')],
              initialProjectId: 'project',
              connected: true,
              executionProfile: () => {},
              onOpenAgent: (_) {},
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(f.issueReads, 1);
      expect(find.text('Issue 1'), findsNothing);
      final githubToggle = find.byKey(const ValueKey('board-github-toggle'));
      expect(
        find.descendant(of: githubToggle, matching: find.byType(Text)),
        findsNothing,
      );
      expect(tester.widget<IconButton>(githubToggle).isSelected, isFalse);
      await tester.tap(find.byTooltip('GitHub'));
      await tester.pumpAndSettle();
      expect(find.text('Issue 1'), findsOneWidget);
      expect(tester.widget<IconButton>(githubToggle).isSelected, isTrue);
      expect(f.issueReads, 1);
      await tester.tap(find.byTooltip('GitHub'));
      await tester.pumpAndSettle();
      expect(find.text('Issue 1'), findsNothing);
      expect(tester.widget<IconButton>(githubToggle).isSelected, isFalse);
      expect(f.issueReads, 1);
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
    },
  );

  testWidgets('import updates cached cards without fetching the list again', (
    tester,
  ) async {
    final f = SessionFixture();
    await f.mount(tester);
    await tester.pumpAndSettle();
    await tester.pumpWidget(const SizedBox());
    await githubRequest(f.request, {
      'Import': {
        'project_id': 'project',
        'repository_id': 42,
        'numbers': [7],
        'request_id': 'import',
      },
    });
    await f.mount(tester);
    await tester.pumpAndSettle();
    expect(find.textContaining('In board'), findsOneWidget);
    expect(f.issueReads, 1);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'one automatic load per app session, no timer requests or reopen refresh',
    (tester) async {
      final f = SessionFixture();
      await f.mount(tester);
      await tester.pumpAndSettle();
      expect([f.statuses, f.links, f.issueReads, f.syncs], [1, 1, 1, 0]);
      await tester.pump(const Duration(hours: 2));
      await tester.pumpAndSettle();
      expect([f.statuses, f.links, f.issueReads, f.syncs], [1, 1, 1, 0]);
      await tester.pumpWidget(const SizedBox());
      await f.mount(tester);
      await tester.pumpAndSettle();
      expect(find.text('Issue 1'), findsOneWidget);
      expect([f.statuses, f.links, f.issueReads, f.syncs], [1, 1, 1, 0]);
      await tester.tap(find.byIcon(Icons.refresh));
      await tester.pumpAndSettle();
      expect(find.text('Issue 2'), findsOneWidget);
      expect([f.statuses, f.links, f.issueReads, f.syncs], [2, 2, 2, 1]);
      await tester.pumpWidget(const SizedBox());
      // A new app/runtime-client session gets exactly one new initial fetch.
      final restarted = SessionFixture();
      await restarted.mount(tester);
      await tester.pumpAndSettle();
      expect(restarted.issueReads, 1);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'initial read survives pane closure and is not duplicated on reopen',
    (tester) async {
      final f = SessionFixture()..pending = Completer<void>();
      await f.mount(tester);
      expect(f.issueReads, 1);
      await tester.pumpWidget(const SizedBox());
      await f.mount(tester);
      expect(f.issueReads, 1);
      f.pending!.complete();
      await tester.pumpAndSettle();
      expect(find.text('Issue 1'), findsOneWidget);
      expect(f.issueReads, 1);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets(
    'failed first load waits for explicit refresh even after remount',
    (tester) async {
      final f = SessionFixture()..failIssues = true;
      await f.mount(tester);
      await tester.pumpAndSettle();
      await tester.pumpWidget(const SizedBox());
      f.failIssues = false;
      await f.mount(tester);
      await tester.pumpAndSettle();
      await tester.pump(const Duration(minutes: 30));
      expect(f.issueReads, 1);
      await tester.tap(find.byIcon(Icons.refresh));
      await tester.pumpAndSettle();
      expect(f.issueReads, 2);
      expect(find.text('Issue 2'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('filters use cached lists and new queries wait for Refresh', (
    tester,
  ) async {
    final f = SessionFixture();
    await f.mount(tester);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Closed').last);
    await tester.pumpAndSettle();
    expect(f.issueReads, 1);
    expect(find.text('Click Refresh to load issues.'), findsOneWidget);
    await tester.tap(find.byIcon(Icons.refresh));
    await tester.pumpAndSettle();
    expect(f.issueReads, 2);
    await tester.tap(find.text('Closed'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Open').last);
    await tester.pumpAndSettle();
    expect(find.text('Issue 1'), findsOneWidget);
    expect(f.issueReads, 2);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'settings disconnect clears cached private data without polling',
    (tester) async {
      final f = SessionFixture();
      await f.mount(tester);
      await tester.pumpAndSettle();
      expect(find.text('Issue 1'), findsOneWidget);
      await githubRequest(f.request, 'Disconnect');
      await tester.pumpAndSettle();
      expect(find.text('Issue 1'), findsNothing);
      expect(find.text('Connect GitHub'), findsOneWidget);
      expect(f.statuses, 1);
      await tester.pumpWidget(const SizedBox());
      f.connected = true;
      f.generation = 'different-account';
      await githubRequest(f.request, 'Status');
      await f.mount(tester);
      await tester.pumpAndSettle();
      expect(find.text('Issue 1'), findsNothing);
      expect(find.text('Click Refresh to load issues.'), findsOneWidget);
      expect(f.issueReads, 1);
      await tester.pumpWidget(const SizedBox());
    },
  );
}
