import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/github_inbox.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/release_ui_visibility.dart';

void main() {
  testWidgets(
    'empty connected inbox has one setup action and no unused controls',
    (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 350,
              child: GitHubInbox(
                projects: const [TaskProjectOption('project', 'Project')],
                onImported: () async {},
                request: (request) async =>
                    (request as Map)['GitHub'] == 'Status'
                    ? {
                        'GitHub': {'connected': true, 'generation': 'same'},
                      }
                    : {
                        'GitHub': {'repositories': []},
                      },
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.text('Link repository'), findsOneWidget);
      expect(find.text('Open'), findsNothing);
      expect(find.text('Add 0 to Backlog'), findsNothing);
      expect(find.text('Create issue'), findsNothing);
      expect(find.text('Connection'), findsNothing);
      await tester.tap(find.byTooltip('GitHub options'));
      await tester.pumpAndSettle();
      expect(find.text('GitHub account…'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets(
    'one linked repository loads automatically and cards select without checkboxes',
    (tester) async {
      var reads = 0;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SizedBox(
              width: 350,
              child: GitHubInbox(
                projects: const [TaskProjectOption('project', 'Project')],
                onImported: () async {},
                request: (request) async {
                  final op = (request as Map)['GitHub'];
                  if (op == 'Status') {
                    return {
                      'GitHub': {'connected': true, 'generation': 'same'},
                    };
                  }
                  if (op is Map && op.containsKey('Links')) {
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
                  if (op is Map && op.containsKey('Issues')) {
                    reads++;
                    return {
                      'GitHub': {
                        'issues': [
                          {
                            'id': 7,
                            'number': 7,
                            'title': 'Fix upload',
                            'state': 'open',
                          },
                        ],
                        'imported': {},
                        'has_more': false,
                      },
                    };
                  }
                  throw StateError('Unexpected request');
                },
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(reads, 1);
      expect(find.text('Fix upload'), findsOneWidget);
      expect(find.byType(Checkbox), findsNothing);
      expect(find.text('Add 1 to Backlog'), findsNothing);
      await tester.tap(find.text('Fix upload'));
      await tester.pump();
      expect(find.text('1 selected · Drag to Backlog'), findsOneWidget);
      expect(find.byTooltip('Issue info'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets('status polling preserves a failed Connect action', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: GitHubSettingsDialog(
          request: (request) async {
            if ((request as Map)['GitHub'] == 'Connect') {
              throw StateError('Connection was denied.');
            }
            return {
              'GitHub': {'connected': false, 'busy': false},
            };
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Connect GitHub'));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 2));
    await tester.pumpAndSettle();
    expect(find.textContaining('Connection was denied.'), findsOneWidget);
    expect(find.byType(LinearProgressIndicator), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets('failed initial status stops progress and retry recovers', (
    tester,
  ) async {
    var approved = false;
    await tester.pumpWidget(
      MaterialApp(
        home: GitHubSettingsDialog(
          request: (_) async {
            if (!approved) {
              throw StateError(
                'This staging installation needs approval. Installation ID: test-installation',
              );
            }
            return {
              'GitHub': {
                'connected': false,
                'busy': false,
                'provider': 'github_app',
              },
            };
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byType(LinearProgressIndicator), findsNothing);
    expect(find.text('Preparing browser sign-in…'), findsNothing);
    expect(
      find.textContaining('Installation ID: test-installation'),
      findsOneWidget,
    );
    expect(find.text('Try again'), findsOneWidget);
    approved = true;
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(
      find.textContaining('Installation ID: test-installation'),
      findsNothing,
    );
    expect(find.text('Connect GitHub'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'failed connection preserves detail without a progress indicator',
    (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: GitHubSettingsDialog(
            request: (_) async => {
              'GitHub': {
                'connected': false,
                'busy': false,
                'connection_state': 'failed',
                'detail': 'Repository access was removed.',
              },
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(find.text('Repository access was removed.'), findsOneWidget);
      expect(find.text('Try again'), findsOneWidget);
      await tester.pumpWidget(const SizedBox());
    },
  );

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
      expect(find.text('org/private'), findsOneWidget);
      // A page containing only PRs still allows reaching later issue pages.
      expect(find.text('No open issues'), findsOneWidget);
      await tester.tap(find.text('Load more'));
      await tester.pumpAndSettle();
      expect(find.text('Private issue'), findsOneWidget);
      expect(
        tester
            .widget<Draggable<GitHubIssueDrag>>(
              find.byKey(const ValueKey('github-issue-7')),
            )
            .maxSimultaneousDrags,
        0,
      );
      expect(find.textContaining('In board'), findsOneWidget);
      connected = false;
      await tester.tap(find.byIcon(Icons.refresh));
      await tester.pumpAndSettle();
      expect(find.text('Private issue'), findsNothing);
      expect(find.text('Link repository'), findsNothing);
      expect(find.text('Connect GitHub'), findsOneWidget);
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
    expect(
      find.textContaining(
        showGitHubBoardControls
            ? 'GitHub CLI credentials are not used'
            : 'shared with other tools',
      ),
      findsOneWidget,
    );
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
    expect(loads, 1);
    await tester.tap(find.text('Retry repositories'));
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
    await tester.tap(find.text('Browse repositories'));
    await tester.pumpAndSettle();
    expect(find.text('org/repo1'), findsOneWidget);
    await tester.ensureVisible(find.text('Load more repositories'));
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
