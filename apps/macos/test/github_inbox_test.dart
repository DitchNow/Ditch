import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/github_inbox.dart';
import 'package:the_ditch/data/task_models.dart';

void main() {
  testWidgets(
    'inbox disables imported issues and clears private data on disconnect',
    (tester) async {
      var connected = true;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 350,
              child: GitHubInbox(
                projects: const [TaskProjectOption('project', 'Project')],
                onImported: () async {},
                request: (request) async {
                  final operation = (request as Map)['GitHub'];
                  if (operation == 'Status') {
                    return {
                      'GitHub': {
                        'connected': connected,
                        'generation': connected ? 'bound' : 'disconnected',
                      },
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
                              'full_name': 'org/private',
                            },
                          },
                        ],
                      },
                    };
                  }
                  if (operation is Map && operation.containsKey('Issues')) {
                    return {
                      'GitHub': {
                        'issues': operation['Issues']['page'] == 1
                            ? []
                            : [
                                {
                                  'id': 7,
                                  'number': 7,
                                  'title': 'Private issue',
                                  'state': 'closed',
                                },
                              ],
                        'imported': {'7': 'task'},
                        'has_more': operation['Issues']['page'] == 1,
                      },
                    };
                  }
                  throw StateError('Unexpected operation');
                },
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('No issues loaded'), findsOneWidget);
      await tester.tap(find.text('Select a linked repository'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('org/private').last);
      await tester.pumpAndSettle();
      // A page containing only PRs still allows reaching later issue pages.
      expect(find.text('No issues loaded'), findsOneWidget);
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      expect(find.text('#7 Private issue'), findsOneWidget);
      expect(
        tester
            .widget<CheckboxListTile>(find.byType(CheckboxListTile))
            .onChanged,
        isNull,
      );
      expect(find.textContaining('Already imported'), findsOneWidget);
      connected = false;
      await tester.pump(const Duration(seconds: 2));
      await tester.pumpAndSettle();
      expect(find.text('#7 Private issue'), findsNothing);
      expect(
        tester
            .widget<TextButton>(
              find.widgetWithText(TextButton, 'Link repository'),
            )
            .onPressed,
        isNull,
      );
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets('one Connect click starts browser login with inline disclosure', (
    tester,
  ) async {
    final calls = <Object>[];
    var connected = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: GitHubSettingsDialog(
            request: (request) async {
              final operation = (request as Map)['GitHub'];
              calls.add(operation);
              if (operation == 'Connect') connected = true;
              return {
                'GitHub': {
                  'installed': connected,
                  'connected': connected,
                  'connection_state': connected ? 'connected' : 'disconnected',
                  'generation': connected ? 'bound' : 'initial',
                  'account': connected ? {'id': 1, 'login': 'alice'} : null,
                },
              };
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(calls.every((v) => v == 'Status'), isTrue);
    expect(find.textContaining('shared with other tools'), findsOneWidget);
    for (final removed in [
      'Use this account',
      'Repair CLI',
      'Sign in with browser',
      'Agree and continue',
      'Install & Connect',
    ]) {
      expect(find.text(removed), findsNothing);
    }
    await tester.tap(find.text('Connect GitHub'));
    await tester.pumpAndSettle();
    expect(calls.where((v) => v != 'Status'), ['Connect']);
    expect(find.text('Connected as @alice'), findsOneWidget);
    expect(find.text('Disconnect'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'browser launch failure offers a working retry without restarting login',
    (tester) async {
      var opens = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: GitHubSettingsDialog(
              request: (request) async {
                final operation = (request as Map)['GitHub'];
                if (operation == 'OpenBrowser' && ++opens == 1) {
                  return {
                    'Error': {'message': 'launch failed'},
                  };
                }
                return {
                  'GitHub': {
                    'connected': false,
                    'busy': true,
                    'generation': 'login',
                    'connection_state': 'authenticating',
                    'device_code': 'ABCD-1234',
                    'browser_ready': true,
                  },
                };
              },
            ),
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(
        find.textContaining('Could not open your browser'),
        findsOneWidget,
      );
      await tester.pump(const Duration(seconds: 2));
      expect(opens, 1);
      await tester.tap(find.text('Open browser'));
      await tester.pump();
      expect(opens, 2);
      expect(find.textContaining('Could not open your browser'), findsNothing);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('inbox retries a failed repository list without reconnecting', (
    tester,
  ) async {
    var loads = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 350,
            child: GitHubInbox(
              projects: const [TaskProjectOption('project', 'Project')],
              onImported: () async {},
              request: (request) async {
                final operation = (request as Map)['GitHub'];
                if (operation == 'Status') {
                  return {
                    'GitHub': {'connected': true, 'generation': 'same'},
                  };
                }
                if (operation is Map && operation.containsKey('Links')) {
                  if (++loads == 1) {
                    return {
                      'Error': {'message': 'Temporary read failure'},
                    };
                  }
                  return {
                    'GitHub': {
                      'repositories': [
                        {
                          'project_id': 'project',
                          'repository': {'id': 42, 'full_name': 'org/repo'},
                        },
                      ],
                    },
                  };
                }
                throw StateError('Unexpected operation');
              },
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Retry repositories'), findsOneWidget);
    await tester.pump(const Duration(seconds: 2));
    await tester.pumpAndSettle();
    expect(loads, 2);
    expect(find.text('Retry repositories'), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('Disconnect uses only the local runtime operation', (
    tester,
  ) async {
    final calls = <Object>[];
    var connected = true;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: GitHubSettingsDialog(
            request: (request) async {
              calls.add(request);
              if ((request as Map)['GitHub'] == 'Disconnect') connected = false;
              return {
                'GitHub': {
                  'installed': true,
                  'connected': connected,
                  'consent': true,
                  'generation': connected ? 'bound' : 'disconnected',
                  'connection_state': connected ? 'connected' : 'disconnected',
                },
              };
            },
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Disconnect'));
    await tester.pumpAndSettle();
    expect(calls.where((v) => (v as Map)['GitHub'] != 'Status').toList(), [
      {'GitHub': 'Disconnect'},
    ]);
    expect(find.text('Connect GitHub'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'device panel exposes code and cancel, opens expected flow once',
    (tester) async {
      final calls = <Object>[];
      var pending = true;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: GitHubSettingsDialog(
              request: (request) async {
                calls.add(request);
                if ((request as Map)['GitHub'] == 'Cancel') pending = false;
                return {
                  'GitHub': {
                    'installed': true,
                    'connected': false,
                    'consent': true,
                    'generation': 'session',
                    'connection_state': pending
                        ? 'authenticating'
                        : 'disconnected',
                    'busy': pending,
                    'device_code': pending ? 'ABCD-1234' : null,
                    'browser_ready': pending,
                  },
                };
              },
            ),
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      expect(find.text('ABCD-1234'), findsOneWidget);
      expect(find.text('Copy code'), findsOneWidget);
      await tester.pump(const Duration(seconds: 2));
      expect(
        calls.where((v) => (v as Map)['GitHub'] == 'OpenBrowser').length,
        1,
      );
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(find.text('ABCD-1234'), findsNothing);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('repository discovery provides explicit Load More', (
    tester,
  ) async {
    final pages = <int>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: GitHubRepositoryPicker(
            projects: const [],
            request: (request) async {
              final operation = (request as Map)['GitHub'] as Map;
              final page = operation['Repositories']['page'] as int;
              pages.add(page);
              return {
                'GitHub': {
                  'repositories': [
                    {'id': page, 'full_name': 'org/repo$page', 'private': true},
                  ],
                  'has_more': page == 1,
                },
              };
            },
          ),
        ),
      ),
    );
    await tester.tap(find.text('Browse accessible repositories'));
    await tester.pumpAndSettle();
    expect(find.text('org/repo1'), findsOneWidget);
    await tester.tap(find.text('Load more repositories'));
    await tester.pumpAndSettle();
    expect(pages, [1, 2]);
    expect(find.text('org/repo2'), findsOneWidget);
    expect(find.text('Load more repositories'), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });

  test('GitHub errors retain their useful runtime message', () async {
    await expectLater(
      githubRequest(
        (_) async => {
          'Error': {'message': 'Repository access unavailable'},
        },
        'Status',
      ),
      throwsA(
        isA<StateError>().having(
          (e) => e.message,
          'message',
          contains('Repository access unavailable'),
        ),
      ),
    );
  });
}
