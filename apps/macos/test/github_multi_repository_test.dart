import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/github_inbox.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/data/task_templates.dart';
import 'package:the_ditch/task_board.dart';
import 'package:the_ditch/application/task_board_controller.dart';

class MultiRepositoryFixture {
  final imports = <Map>[];
  final writes = <Map>[];
  final comments = <Map>[];
  int reads = 0;
  bool failChild = true;
  Map? publication;
  List<Map<String, dynamic>> get links => [
    for (final id in [42, 43])
      {
        'project_id': 'project',
        'repository': {'id': id, 'full_name': 'owner/repo$id'},
      },
  ];
  Future<Map<String, dynamic>> request(Object req) async {
    final op = (req as Map)['GitHub'];
    if (op == 'Status') {
      return {
        'GitHub': {'connected': true, 'generation': 'one'},
      };
    }
    if (op is! Map) throw StateError('$op');
    if (op.containsKey('Links')) {
      return {
        'GitHub': {'repositories': links},
      };
    }
    if (op.containsKey('Publication')) {
      return {
        'GitHub': {'publication': publication},
      };
    }
    if (op.containsKey('IssueTypes')) {
      return {
        'GitHub': {'types': [], 'can_set_type': false},
      };
    }
    if (op.containsKey('CreateIssue')) {
      writes.add(op['CreateIssue'] as Map);
      publication = op['CreateIssue'] as Map;
      return {
        'Error': {'message': 'Response lost'},
      };
    }
    if (op.containsKey('Issues')) {
      reads++;
      final id = op['Issues']['repository_id'];
      return {
        'GitHub': {
          'issues': [
            {
              'id': id * 100,
              'number': 1,
              'title': 'Issue from $id',
              'state': 'open',
              'body': 'Original $id',
              'html_url': 'https://github.com/owner/repo$id/issues/1',
            },
          ],
          'has_more': false,
          'imported': {},
        },
      };
    }
    if (op.containsKey('Import')) {
      final body = op['Import'] as Map;
      imports.add(body);
      if (body['repository_id'] == 43 && failChild) {
        return {
          'Error': {'message': 'Child offline'},
        };
      }
      return {
        'GitHub': {
          'task_ids': ['task-${body['repository_id']}'],
        },
      };
    }
    if (op.containsKey('Comments')) {
      comments.add(op['Comments'] as Map);
      return {
        'GitHub': {'comments': [], 'has_more': false},
      };
    }
    throw StateError('$op');
  }

  Future<void> mount(WidgetTester tester) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 370,
            child: GitHubInbox(
              request: request,
              projects: const [TaskProjectOption('project', 'Project')],
              onImported: () async {},
            ),
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Finder card(int id) =>
      find.byKey(ValueKey('github-issue-project:$id:${id * 100}'));
}

void main() {
  testWidgets(
    'task templates preserve user edits and Save only creates a local task',
    (tester) async {
      final operations = <Map>[];
      final controller = TaskBoardController(
        request: (request) async {
          final operation =
              ((request as Map)['TaskRequest'] as Map)['operation'] as Map;
          operations.add(operation);
          final draft = operation['CreateBacklog']['draft'] as Map;
          return {
            'TaskResponse': {
              'Changed': {
                ...draft,
                'id': 'local',
                'project_id': 'project',
                'state': 'Backlog',
                'condition': 'Idle',
                'revision': 1,
                'order_key': 1024,
                'archived': false,
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
                onPressed: () => showTaskEditor(context, controller, const [
                  TaskProjectOption('project', 'Project'),
                ]),
                child: const Text('New'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('New'));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('task-type-Custom')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Bug').last);
      await tester.pumpAndSettle();
      final description = find.byWidgetPredicate(
        (v) => v is TextFormField && v.controller?.text == taskTemplates['Bug'],
      );
      expect(description, findsOneWidget);
      await tester.enterText(description, 'My requirements');
      await tester.tap(find.byKey(const ValueKey('task-type-Bug')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Feature').last);
      await tester.pumpAndSettle();
      expect(
        find.byWidgetPredicate(
          (v) => v is TextFormField && v.controller?.text == 'My requirements',
        ),
        findsOneWidget,
      );
      await tester.enterText(
        find.byWidgetPredicate(
          (v) => v is TextField && v.decoration?.labelText == 'Title',
        ),
        'Local only',
      );
      await tester.tap(find.text('Save Task'));
      await tester.pumpAndSettle();
      expect(
        operations.single['CreateBacklog']['draft'],
        containsPair('issue_kind', 'Feature'),
      );
      expect(
        operations.single['CreateBacklog']['draft'],
        containsPair('description', 'My requirements'),
      );
      await tester.pumpWidget(const SizedBox());
      controller.dispose();
    },
  );
  testWidgets(
    'submodule selection is explicit and partial linking retains successful repositories',
    (tester) async {
      final linked = <String>[];
      bool? result;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) => TextButton(
                child: const Text('Link'),
                onPressed: () async {
                  result = await showDialog<bool>(
                    context: context,
                    builder: (_) => GitHubRepositoryPicker(
                      projects: const [TaskProjectOption('project', 'Project')],
                      request: (request) async {
                        final op = (request as Map)['GitHub'] as Map;
                        if (op.containsKey('RemoteSuggestions')) {
                          return {
                            'GitHub': {
                              'repositories': ['owner/main'],
                              'candidates': [
                                {
                                  'repository': 'owner/main',
                                  'paths': ['.'],
                                  'is_root': true,
                                },
                                {
                                  'repository': 'owner/child',
                                  'paths': ['modules/child'],
                                  'is_root': false,
                                },
                              ],
                              'warnings': [],
                            },
                          };
                        }
                        final name = op['Link']['repository'] as String;
                        linked.add(name);
                        if (name == 'owner/child') {
                          return {
                            'Error': {'message': 'Repository not installed'},
                          };
                        }
                        return {'GitHub': {}};
                      },
                    ),
                  );
                },
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Link'));
      await tester.pumpAndSettle();
      expect(
        tester.widget<CheckboxListTile>(find.byType(CheckboxListTile)).value,
        isFalse,
      );
      await tester.tap(find.text('owner/child'));
      await tester.tap(find.text('Verify and link'));
      await tester.pumpAndSettle();
      expect(linked, ['https://github.com/owner/main', 'owner/child']);
      await tester.tap(find.text('Verify and link'));
      await tester.pumpAndSettle();
      expect(linked.where((v) => v.contains('/main')).length, 1);
      await tester.tap(find.text('Done'));
      await tester.pumpAndSettle();
      expect(result, isTrue);
      await tester.pumpWidget(const SizedBox());
    },
  );
  test('bundled templates match the editable Markdown sources', () {
    for (final kind in taskIssueKinds) {
      expect(
        taskTemplates[kind],
        File(
          'assets/task_templates/${kind.toLowerCase()}.md',
        ).readAsStringSync(),
      );
    }
  });
  testWidgets(
    'same issue numbers stay distinct across repositories; partial import retries only failures',
    (tester) async {
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('the_ditch/application'),
        (call) async => true,
      );
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          const MethodChannel('the_ditch/application'),
          null,
        ),
      );
      final f = MultiRepositoryFixture();
      await f.mount(tester);
      expect(f.reads, 2);
      await tester.tap(find.text('Issue from 42'));
      await tester.sendKeyDownEvent(LogicalKeyboardKey.shiftLeft);
      await tester.tap(find.text('Issue from 43'));
      await tester.sendKeyUpEvent(LogicalKeyboardKey.shiftLeft);
      await tester.pumpAndSettle();
      final drag = tester.widget<Draggable<GitHubIssueDrag>>(f.card(42)).data!;
      expect(drag.count, 2);
      await drag.importToBacklog();
      await tester.pumpAndSettle();
      expect(f.imports.map((v) => v['repository_id']), [42, 43]);
      expect(find.textContaining('In board'), findsOneWidget);
      f.failChild = false;
      await tester
          .widget<Draggable<GitHubIssueDrag>>(f.card(43))
          .data!
          .importToBacklog();
      await tester.pumpAndSettle();
      expect(f.imports.map((v) => v['repository_id']), [42, 43, 43]);
      expect(f.imports[1]['request_id'], f.imports[2]['request_id']);
      expect(find.textContaining('In board'), findsNWidgets(2));
      await tester.pump(const Duration(hours: 1));
      await tester.pumpWidget(const SizedBox());
      await f.mount(tester);
      expect(f.reads, 2);
      expect(find.textContaining('In board'), findsNWidgets(2));
      await tester.tap(
        find.descendant(of: f.card(43), matching: find.byTooltip('Issue info')),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Load comments (0)'));
      await tester.pumpAndSettle();
      expect(f.comments.single['repository_id'], 43);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets(
    'publishing an existing task restores its durable request after reopening',
    (tester) async {
      final f = MultiRepositoryFixture();
      final task = TaskDto.fromJson({
        'id': 'local-task',
        'project_id': 'project',
        'title': 'Local title',
        'description': 'User requirements',
        'issue_kind': 'Feature',
        'state': 'Ready',
        'priority': 'Normal',
        'condition': 'Idle',
        'revision': 1,
        'order_key': 1024,
        'archived': false,
      });
      Future<void> mount() async {
        await tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: GitHubCreateIssueDialog(request: f.request, task: task),
            ),
          ),
        );
        await tester.pumpAndSettle();
      }

      await mount();
      expect(f.writes, isEmpty);
      await tester.tap(find.text('Create issue'));
      await tester.pumpAndSettle();
      expect(f.writes.single, containsPair('task_id', 'local-task'));
      expect(f.writes.single, containsPair('body', 'User requirements'));
      expect(f.writes.single, containsPair('issue_kind', 'Feature'));
      expect(f.writes.single, containsPair('issue_type', null));
      await tester.pumpWidget(const SizedBox());
      await mount();
      await tester.tap(find.text('Retry same request'));
      await tester.pumpAndSettle();
      expect(f.writes.length, 2);
      expect(f.writes.first, f.writes.last);
      await tester.pumpWidget(const SizedBox());
    },
  );
}
