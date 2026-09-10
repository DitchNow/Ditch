import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/skills_controller.dart';
import 'package:the_ditch/data/task_models.dart';
import 'package:the_ditch/skills_directory.dart';

Map<String, dynamic> entry(
  String id, {
  bool enabled = true,
  bool managed = false,
}) => {
  'identity': id,
  'name': 'Same name',
  'description': 'A test skill',
  'path': '/fixture/$id/SKILL.md',
  'scope': managed ? 'managed' : 'user',
  'source': '/fixture',
  'source_id': 'source',
  'relative_path': id,
  'content_hash': 'hash-$id',
  'revision': null,
  'license': null,
  'enabled': enabled,
  'recognized': true,
  'managed': managed,
  'has_scripts': true,
  'validation_error': null,
  'missing_dependencies': <String>[],
};
void main() {
  test('pagination retains same-name skills as separate identities', () async {
    final offsets = <int>[];
    final controller = SkillsController(
      request: (request) async {
        final op =
            ((request as Map)['SkillRequest'] as Map)['operation'] as Map;
        final offset = (op['List'] as Map)['offset'] as int;
        offsets.add(offset);
        return {
          'SkillResponse': {
            'Entries': {
              'entries': [entry('$offset')],
              'next_offset': offset == 0 ? 1 : null,
              'errors': [],
              'app_server': true,
            },
          },
        };
      },
    );
    final entries = await controller.list('project');
    expect(offsets, [0, 1]);
    expect(entries.length, 2);
    expect(entries.map((s) => s['identity']).toSet().length, 2);
    controller.dispose();
  });
  test('errors are visible and do not report an installed result', () async {
    final controller = SkillsController(
      request: (_) async => {
        'SkillResponse': {
          'Error': {
            'code': 'hash_changed',
            'message': 'Review changed content before starting',
          },
        },
      },
    );
    expect(
      await controller.call('project', {
        'ConfirmInstall': {'plan_id': 'id'},
      }),
      isNull,
    );
    expect(controller.error, contains('changed content'));
    expect(controller.busy, false);
    controller.dispose();
  });
  testWidgets(
    'picker blocks disabled entries and returns explicit path and hash',
    (tester) async {
      final controller = SkillsController(
        request: (_) async => {
          'SkillResponse': {
            'Entries': {
              'entries': [entry('one'), entry('two', enabled: false)],
              'next_offset': null,
              'errors': [],
              'app_server': true,
            },
          },
        },
      );
      List<Map<String, dynamic>>? selected;
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: Builder(
              builder: (context) => TextButton(
                onPressed: () async {
                  selected = await showSkillPicker(
                    context,
                    controller,
                    'project',
                    [],
                  );
                },
                child: const Text('Pick'),
              ),
            ),
          ),
        ),
      );
      await tester.tap(find.text('Pick'));
      await tester.pumpAndSettle();
      expect(find.byType(CheckboxListTile), findsNWidgets(2));
      final disabled = tester
          .widgetList<CheckboxListTile>(find.byType(CheckboxListTile))
          .last;
      expect(disabled.onChanged, isNull);
      await tester.tap(find.byType(CheckboxListTile).first);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Use selected skills'));
      await tester.pumpAndSettle();
      expect(selected!.single['path'], '/fixture/one/SKILL.md');
      expect(selected!.single['content_hash'], 'hash-one');
      expect(selected!.single['origin'], 'User');
      controller.dispose();
    },
  );
  testWidgets('more than three selections require an override confirmation', (
    tester,
  ) async {
    final controller = SkillsController(
      request: (_) async => {
        'SkillResponse': {
          'Entries': {
            'entries': [],
            'next_offset': null,
            'errors': [],
            'app_server': true,
          },
        },
      },
    );
    var accepted = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Builder(
            builder: (context) => TextButton(
              onPressed: () async {
                accepted =
                    await showSkillPicker(context, controller, 'project', [
                      for (var i = 0; i < 4; i++)
                        SkillsController.binding(entry('$i')),
                    ]) !=
                    null;
              },
              child: const Text('Pick'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Pick'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Use selected skills'));
    await tester.pumpAndSettle();
    expect(accepted, false);
    expect(find.text('Attach more than three skills?'), findsOneWidget);
    await tester.tap(find.text('Confirm'));
    await tester.pumpAndSettle();
    expect(accepted, true);
    controller.dispose();
  });
  testWidgets(
    'catalog never installs before reviewing and confirming the plan',
    (tester) async {
      final operations = <String>[];
      final skill = entry('one');
      final controller = SkillsController(
        request: (request) async {
          final op =
              ((request as Map)['SkillRequest'] as Map)['operation'] as Map;
          final name = op.keys.single as String;
          operations.add(name);
          return {
            'SkillResponse': switch (name) {
              'Sources' => {
                'Sources': {
                  'sources': [
                    {
                      'id': 'source',
                      'name': 'Fixture collection',
                      'location': '/fixture',
                      'reference': 'HEAD',
                      'seeded': false,
                    },
                  ],
                  'next_offset': null,
                },
              },
              'PrepareInstall' => {
                'Plan': {
                  'id': 'plan',
                  'entry': skill,
                  'source': {'reference': 'HEAD'},
                  'resolved_revision': null,
                  'destination': '/managed/one',
                  'included_paths': ['SKILL.md'],
                },
              },
              'ConfirmInstall' => {'Installed': skill},
              _ => {
                'Entries': {
                  'entries': name == 'BrowseSource' ? [skill] : [],
                  'next_offset': null,
                  'errors': [],
                  'app_server': true,
                },
              },
            },
          };
        },
      );
      await tester.binding.setSurfaceSize(const Size(680, 600));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: SkillsDirectory(
              controller: controller,
              projects: const [TaskProjectOption('project', 'Project')],
              connected: true,
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Available'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Browse source'));
      await tester.pumpAndSettle();
      expect(operations, isNot(contains('ConfirmInstall')));
      await tester.tap(find.text('Install…'));
      await tester.pumpAndSettle();
      expect(find.text('Install this revision?'), findsOneWidget);
      expect(find.textContaining('Unknown — not verified'), findsOneWidget);
      expect(operations, isNot(contains('ConfirmInstall')));
      await tester.tap(find.text('Confirm'));
      await tester.pumpAndSettle();
      expect(operations.where((v) => v == 'ConfirmInstall').length, 1);
      expect(tester.takeException(), isNull);
      controller.dispose();
    },
  );
}
