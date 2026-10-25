import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/github_inbox.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/release_ui_visibility.dart';

void main() {
  testWidgets(
    'manual source refresh reconciles all pages without timer requests',
    (tester) async {
      final syncs = <Map>[];
      var refreshed = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: GitHubInbox(
              projects: const [TaskProjectOption('project', 'Project')],
              onImported: () async {
                refreshed++;
              },
              request: (request) async {
                final operation = (request as Map)['GitHub'];
                if (operation == 'Status') {
                  return {
                    'GitHub': {'connected': true, 'generation': 'current'},
                  };
                }
                if (operation is Map && operation.containsKey('Links')) {
                  return {
                    'GitHub': {
                      'repositories': [
                        {
                          'project_id': 'project',
                          'repository': {
                            'id': 42,
                            'full_name': 'owner/selected',
                          },
                        },
                      ],
                    },
                  };
                }
                if (operation is Map && operation.containsKey('SyncChanges')) {
                  syncs.add(operation['SyncChanges'] as Map);
                  return {
                    'GitHub': {
                      'cursor': syncs.length * 11,
                      'repository_ids': [42],
                      'refresh_failures': 0,
                      'after_task': syncs.length == 1
                          ? '11111111-1111-4111-8111-111111111111'
                          : null,
                    },
                  };
                }
                throw StateError('Unexpected operation: $operation');
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.pump(const Duration(seconds: 30));
      await tester.pumpAndSettle();
      expect(syncs, isEmpty);
      await tester.tap(find.byIcon(Icons.refresh));
      await tester.pumpAndSettle();
      expect(syncs.map((v) => v['cursor']), [0, 0]);
      expect(syncs[1]['after_task'], '11111111-1111-4111-8111-111111111111');
      expect(refreshed, 1);
      await tester.pump(const Duration(seconds: 30));
      await tester.pumpAndSettle();
      expect(refreshed, 1);
      await tester.tap(find.byIcon(Icons.refresh));
      await tester.pumpAndSettle();
      expect(syncs.last['cursor'], 0);
      expect(refreshed, 2);
      await tester.pumpWidget(const SizedBox());
    },
    skip: !showGitHubBoardControls,
  );
  test(
    'released Board/GitHub controls leave other unfinished controls hidden',
    () {
      expect(showGitHubBoardControls, isTrue);
      expect(showUnfinishedFeatureControls, isFalse);
    },
  );
  testWidgets(
    'remote fills the repository URL but still requires a link click',
    (tester) async {
      final links = <Map>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: GitHubRepositoryPicker(
              projects: const [TaskProjectOption('project', 'Project')],
              request: (request) async {
                final op = (request as Map)['GitHub'];
                if (op is Map && op.containsKey('RemoteSuggestions')) {
                  return {
                    'GitHub': {
                      'repositories': ['owner/selected'],
                    },
                  };
                }
                if (op is Map && op.containsKey('Link')) {
                  links.add(op['Link'] as Map);
                  return {
                    'GitHub': {'id': 42},
                  };
                }
                throw StateError('Unexpected operation');
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(links, isEmpty);
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        'https://github.com/owner/selected',
      );
      await tester.tap(find.text('Verify and link'));
      await tester.pumpAndSettle();
      expect(links.single, {
        'project_id': 'project',
        'repository': 'https://github.com/owner/selected',
      });
      await tester.pumpWidget(const SizedBox());
    },
    skip: !showGitHubBoardControls,
  );
  testWidgets('late remote detection preserves typed input', (tester) async {
    final remote = Completer<Map<String, dynamic>>();
    await tester.pumpWidget(
      MaterialApp(
        home: GitHubRepositoryPicker(
          projects: const [TaskProjectOption('a', 'Project A')],
          request: (_) => remote.future,
        ),
      ),
    );
    await tester.enterText(find.byType(TextField), 'owner/manual');
    remote.complete({
      'GitHub': {
        'repositories': ['owner/detected'],
      },
    });
    await tester.pumpAndSettle();
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller!.text,
      'owner/manual',
    );
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  }, skip: !showGitHubBoardControls);
  testWidgets(
    'project changes ignore stale remotes and fill the new remote',
    (tester) async {
      final oldRemote = Completer<Map<String, dynamic>>();
      await tester.pumpWidget(
        MaterialApp(
          home: GitHubRepositoryPicker(
            projects: const [
              TaskProjectOption('a', 'Project A'),
              TaskProjectOption('b', 'Project B'),
            ],
            request: (request) async {
              final project =
                  (((request as Map)['GitHub'] as Map)['RemoteSuggestions']
                      as Map)['project_id'];
              if (project == 'a') return oldRemote.future;
              return {
                'GitHub': {
                  'repositories': ['owner/new-project'],
                },
              };
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Project A'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Project B').last);
      await tester.pumpAndSettle();
      oldRemote.complete({
        'GitHub': {
          'repositories': ['owner/old-project'],
        },
      });
      await tester.pumpAndSettle();
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        'https://github.com/owner/new-project',
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
    skip: !showGitHubBoardControls,
  );
  testWidgets(
    'no remote leaves linking disabled until a repository is entered',
    (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: GitHubRepositoryPicker(
            projects: const [TaskProjectOption('a', 'Project A')],
            request: (_) async => {
              'GitHub': {'repositories': []},
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<FilledButton>(
              find.widgetWithText(FilledButton, 'Verify and link'),
            )
            .onPressed,
        isNull,
      );
      await tester.enterText(find.byType(TextField), 'owner/manual');
      await tester.pump();
      expect(
        tester
            .widget<FilledButton>(
              find.widgetWithText(FilledButton, 'Verify and link'),
            )
            .onPressed,
        isNotNull,
      );
      await tester.enterText(find.byType(TextField), '   ');
      await tester.pump();
      expect(
        tester
            .widget<FilledButton>(
              find.widgetWithText(FilledButton, 'Verify and link'),
            )
            .onPressed,
        isNull,
      );
      await tester.pumpWidget(const SizedBox());
    },
    skip: !showGitHubBoardControls,
  );
  testWidgets(
    'issue creation requires a click and retries with the same identity',
    (tester) async {
      final submissions = <Map>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: GitHubCreateIssueDialog(
              link: {
                'project_id': 'project',
                'repository': {'id': 42, 'full_name': 'owner/selected'},
              },
              request: (request) async {
                final operation = (request as Map)['GitHub'] as Map;
                if (operation.containsKey('IssueTypes')) {
                  return {
                    'GitHub': {'types': [], 'can_set_type': false},
                  };
                }
                submissions.add(operation['CreateIssue'] as Map);
                throw StateError('github_create_outcome_unknown');
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.enterText(find.byType(TextField).first, 'My issue');
      await tester.enterText(find.byType(TextField).last, 'My description');
      expect(submissions, isEmpty);
      await tester.tap(find.text('Create issue'));
      await tester.pumpAndSettle();
      expect(submissions.single['repository_id'], 42);
      expect(
        tester.widget<TextField>(find.byType(TextField).first).enabled,
        isFalse,
      );
      await tester.tap(find.text('Retry same request'));
      await tester.pumpAndSettle();
      expect(submissions.length, 2);
      expect(submissions[0], submissions[1]);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets('browser flow has no device code or shared gh disclosure', (
    tester,
  ) async {
    final calls = <Object>[];
    var started = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: GitHubSettingsDialog(
            request: (request) async {
              final op = (request as Map)['GitHub'];
              calls.add(op);
              if (op == 'Connect') started = true;
              return {
                'GitHub': {
                  'provider': 'github_app',
                  'connected': false,
                  'busy': started,
                  'connection_state': started
                      ? 'authenticating'
                      : 'disconnected',
                  'generation': started ? 'new' : 'none',
                  'browser_ready': started,
                  'detail': 'Choose repositories in your browser.',
                },
              };
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('shared GitHub'), findsNothing);
    await tester.tap(find.text('Connect GitHub'));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.pump();
    expect(calls.where((v) => v == 'Connect').length, 1);
    expect(calls.where((v) => v == 'OpenBrowser').length, 1);
    expect(find.text('Copy code'), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });
}
