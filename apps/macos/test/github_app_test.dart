import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/github_inbox.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/release_ui_visibility.dart';

void main() {
  testWidgets(
    'paged source refresh keeps the first cursor for later arrivals',
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
      expect(syncs.map((v) => v['cursor']), [0, 0]);
      expect(syncs[1]['after_task'], '11111111-1111-4111-8111-111111111111');
      expect(refreshed, 1);
      await tester.pump(const Duration(seconds: 30));
      await tester.pumpAndSettle();
      expect(syncs.last['cursor'], 11);
      expect(refreshed, 2);
      await tester.pumpWidget(const SizedBox());
    },
    skip: !showGitHubBoardControls,
  );
  test(
    'staging enables Board/GitHub without exposing other unfinished controls',
    () {
      const staging =
          String.fromEnvironment('DITCH_DEPLOYMENT_ENVIRONMENT') == 'staging' &&
          String.fromEnvironment('DITCH_RELAY_ORIGIN') ==
              'https://ditch-remote-relay-staging.matin-1a7.workers.dev';
      expect(showGitHubBoardControls, staging);
      expect(showUnfinishedFeatureControls, isFalse);
    },
  );
  testWidgets(
    'remote suggestions require explicit selection and linking',
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
        isEmpty,
      );
      await tester.tap(find.text('Git remote: owner/selected'));
      await tester.pump();
      expect(links, isEmpty);
      expect(
        tester.widget<TextField>(find.byType(TextField)).controller!.text,
        'owner/selected',
      );
      await tester.tap(find.text('Verify and link'));
      await tester.pumpAndSettle();
      expect(links.single, {
        'project_id': 'project',
        'repository': 'owner/selected',
      });
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
                submissions.add(
                  ((request as Map)['GitHub'] as Map)['CreateIssue'] as Map,
                );
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
