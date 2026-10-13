import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/project_sidebar_controller.dart';
import 'package:the_ditch/data/project_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/widgets/project_sidebar.dart';
import 'package:the_ditch/main.dart'
    show CommandCenterScreen, DitchRuntimeClient;
import 'package:the_ditch/data/runtime_models.dart';

const local = DitchProject(id: 'local', name: 'Website', path: '/work/site');
const other = DitchProject(id: 'other', name: 'API', path: '/work/api');
const remote = DitchProject(
  id: 'remote',
  name: 'Website server',
  path: '/work/site',
  sshHostAlias: 'production',
);
const projects = [local, other, remote];

class _SidebarRuntime extends DitchRuntimeClient {
  final events = StreamController<Map<String, dynamic>>();

  Map<String, dynamic> get currentSnapshot => {
    'projects': [
      for (final project in projects)
        {
          'id': project.id,
          'name': project.name,
          'root': project.path,
          'execution_target': project.isRemote
              ? {'kind': 'remote', 'ssh_host_alias': project.sshHostAlias}
              : {'kind': 'local'},
        },
    ],
    'agents': <Map<String, dynamic>>[],
    'attention': <Map<String, dynamic>>[],
  };

  @override
  Future<RuntimeStatusDto> runtimeStatus() async => const RuntimeStatusDto(
    identity: 'sidebar-test',
    pid: 1,
    socketPath: '/tmp/sidebar-test.sock',
    activeSessionCount: 0,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'sidebar-instance',
    capabilities: {
      'persistent_sessions_v1',
      'app_server_sessions_v2',
      'always_on_web_access_v1',
    },
  );

  @override
  Future<Map<String, dynamic>> snapshot() async => {
    'Snapshot': currentSnapshot,
  };

  @override
  Future<Stream<Map<String, dynamic>>> subscribeEvents() async => events.stream;

  @override
  Future<Map<String, dynamic>> request(Object body) async => {};
}

ProjectSidebarController memoryController() =>
    ProjectSidebarController(load: () async => null, save: (_) async {});

Widget sidebar(
  ProjectSidebarController controller, {
  double width = 240,
  bool dark = false,
  bool disableAnimations = true,
  Map<String, int>? runningCounts,
  ValueChanged<DitchProject>? onSelect,
}) => MaterialApp(
  theme: dark ? DitchTheme.dark() : DitchTheme.light(),
  builder: (context, child) => MediaQuery(
    data: MediaQuery.of(context).copyWith(disableAnimations: disableAnimations),
    child: child!,
  ),
  home: Scaffold(
    body: Align(
      alignment: Alignment.topLeft,
      child: ProjectSidebar(
        width: width,
        projects: projects,
        controller: controller,
        remoteHostStatus: const {'production': 'offline'},
        selectedProjectKey: local.id,
        onAddProject: () {},
        onSelectProject: onSelect ?? (_) {},
        onRevealProject: (_) {},
        onCopyProjectPath: (_) {},
        onReconnectRemoteProject: (_) {},
        onDeleteProject: (_) {},
        summaryForProject: (project) => ProjectAgentSummary(
          runningCount: runningCounts == null
              ? 2
              : runningCounts[project.id] ?? 0,
          stoppedCount: 1,
          hasUnreadResult: true,
        ),
      ),
    ),
  ),
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('search applies scope, project fields and group names together', () {
    final controller = memoryController();
    addTearDown(controller.dispose);
    final group = controller.createGroup('Product Suite');
    controller.moveProject(local, group);
    controller.moveProject(remote, group);
    expect(controller.matchingProjects(projects), [other, local]);
    controller.setQuery('  SUITE  ');
    expect(controller.matchingProjects(projects), [local]);
    controller.setScope(ProjectScope.remote);
    expect(controller.matchingProjects(projects), [remote]);
    controller.setQuery('PRODUCTION');
    expect(controller.matchingProjects(projects), [remote]);
    controller.setQuery('/work/site');
    expect(controller.matchingProjects(projects), [remote]);
    controller.setQuery('absent');
    expect(controller.matchingProjects(projects), isEmpty);
  });

  test('group moves and deletes never modify projects or collide on paths', () {
    final controller = memoryController();
    addTearDown(controller.dispose);
    final group = controller.createGroup('Related');
    controller.moveProject(local, group);
    expect(controller.groupFor(remote), isNull);
    controller.moveProject(remote, group);
    controller.renameGroup(group, 'Renamed');
    expect(controller.value.groups.single.name, 'Renamed');
    controller.moveProject(local, null);
    expect(controller.groupFor(local), isNull);
    expect(controller.groupFor(remote), group);
    controller.deleteGroup(group);
    expect(controller.value.groups, isEmpty);
    expect(controller.value.memberships, isEmpty);
    expect(projects.length, 3);
  });

  test('preferences round trip and search is temporary', () async {
    String? document;
    final controller = ProjectSidebarController(
      load: () async => document,
      save: (value) async => document = value,
    );
    final group = controller.createGroup('Work');
    controller.moveProject(remote, group);
    controller.toggleGroup(group);
    controller.setScope(ProjectScope.remote);
    controller.setQuery('temporary');
    await controller.flush();
    controller.dispose();
    final restored = ProjectSidebarController(
      load: () async => document,
      save: (_) async {},
    );
    addTearDown(restored.dispose);
    await restored.load();
    expect(restored.value.groups.single.name, 'Work');
    expect(restored.groupFor(remote), group);
    expect(restored.value.collapsed, {group});
    expect(restored.value.scope, ProjectScope.remote);
    expect(restored.value.query, isEmpty);
  });

  test('native preferences use the shared application channel', () async {
    const channel = MethodChannel('the_ditch/application');
    String? saved;
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'setProjectSidebarPreferences') {
        saved = call.arguments as String;
      }
      if (call.method == 'getProjectSidebarPreferences') return saved;
      return null;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    final first = ProjectSidebarController();
    first.createGroup('Native');
    await first.flush();
    first.dispose();
    final restored = ProjectSidebarController();
    addTearDown(restored.dispose);
    await restored.load();
    expect(restored.value.groups.single.name, 'Native');
  });

  test('late preference loading cannot overwrite user organization', () async {
    final pending = Completer<String?>();
    final controller = ProjectSidebarController(
      load: () => pending.future,
      save: (_) async {},
    );
    addTearDown(controller.dispose);
    final loading = controller.load();
    controller.createGroup('New');
    pending.complete(
      jsonEncode({
        'version': 1,
        'groups': [
          {'id': 'old', 'name': 'Old'},
        ],
      }),
    );
    await loading;
    expect(controller.value.groups.single.name, 'New');
  });

  test('saves are serialized and a failed save can be retried', () async {
    final gate = Completer<void>();
    final writes = <String>[];
    var fail = false;
    final controller = ProjectSidebarController(
      load: () async => null,
      save: (data) async {
        writes.add(data);
        if (writes.length == 1) await gate.future;
        if (fail) throw StateError('disk unavailable');
      },
    );
    addTearDown(controller.dispose);
    final group = controller.createGroup('First');
    controller.renameGroup(group, 'Latest');
    await Future<void>.delayed(Duration.zero);
    expect(writes.length, 1);
    gate.complete();
    await controller.flush();
    expect(
      (jsonDecode(writes.last)['groups'] as List).single['name'],
      'Latest',
    );
    fail = true;
    controller.renameGroup(group, 'Retry');
    await controller.flush();
    expect(controller.value.persistenceError, isTrue);
    fail = false;
    controller.retrySave();
    await controller.flush();
    expect(controller.value.persistenceError, isFalse);
  });

  test('malformed and future preference documents fall back safely', () async {
    for (final source in [
      '{',
      '[]',
      '{"version":9}',
      '{"version":1,"groups":[null,{"id":1,"name":false}],"memberships":{"local":"absent"}}',
    ]) {
      final controller = ProjectSidebarController(
        load: () async => source,
        save: (_) async {},
      );
      await controller.load();
      expect(controller.value.groups, isEmpty);
      expect(controller.value.memberships, isEmpty);
      controller.dispose();
    }
  });

  test(
    'reconciliation upgrades provisional identities and retains offline memberships',
    () {
      final controller = memoryController();
      addTearDown(controller.dispose);
      final group = controller.createGroup('Work');
      const provisional = DitchProject(
        name: 'Website server',
        path: '/work/site',
        sshHostAlias: 'production',
      );
      controller.moveProject(provisional, group);
      controller.reconcileProjects(projects);
      expect(controller.value.memberships, {'remote': group});
      controller.reconcileProjects([local]);
      expect(controller.groupFor(remote), group);
    },
  );

  test(
    'explicit navigation reveals a project and clears conflicting filters',
    () {
      final controller = memoryController();
      addTearDown(controller.dispose);
      final group = controller.createGroup('Work');
      controller.moveProject(remote, group);
      controller.toggleGroup(group);
      controller.setQuery('different');
      controller.revealProject(remote);
      expect(controller.value.scope, ProjectScope.remote);
      expect(controller.value.query, isEmpty);
      expect(controller.value.collapsed, isEmpty);
    },
  );

  testWidgets('count tracks search and segments but not collapsed groups', (
    tester,
  ) async {
    final controller = memoryController();
    addTearDown(controller.dispose);
    final group = controller.createGroup('Suite');
    controller.moveProject(local, group);
    controller.moveProject(remote, group);
    await tester.pumpWidget(sidebar(controller));
    expect(find.text('Projects (2)'), findsOneWidget);
    await tester.tap(find.byKey(ValueKey('project-group-$group')));
    await tester.pumpAndSettle();
    expect(find.text('Website'), findsNothing);
    expect(find.text('Projects (2)'), findsOneWidget);
    await tester.tap(find.byKey(const Key('open-project-search')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('project-search-field')),
      'suite',
    );
    await tester.pumpAndSettle();
    expect(find.text('Website'), findsOneWidget);
    expect(find.text('Projects (1)'), findsOneWidget);
    expect(controller.value.collapsed, {group});
    await tester.tap(find.byKey(const Key('clear-project-search')));
    await tester.pumpAndSettle();
    expect(find.text('Website'), findsNothing);
    await tester.tap(find.text('Remote'));
    await tester.pumpAndSettle();
    expect(find.text('Projects (1)'), findsOneWidget);
    await tester.tap(find.byKey(ValueKey('project-group-$group')));
    await tester.pumpAndSettle();
    expect(find.byType(RemoteProjectTag), findsOneWidget);
    expect(find.text('production · Offline'), findsOneWidget);
  });

  testWidgets('search expands to padded width and Escape restores controls', (
    tester,
  ) async {
    final controller = memoryController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(sidebar(controller));
    await tester.tap(find.byKey(const Key('open-project-search')));
    await tester.pumpAndSettle();
    final field = find.byKey(const Key('project-search-field'));
    expect(tester.getSize(field).width, 216);
    expect(find.byKey(const Key('project-scope')), findsNothing);
    await tester.enterText(field, 'no match');
    await tester.pumpAndSettle();
    expect(find.text('Projects (0)'), findsOneWidget);
    expect(find.text('No matching projects'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pumpAndSettle();
    expect(find.byKey(const Key('project-scope')), findsOneWidget);
    expect(find.text('Projects (2)'), findsOneWidget);
  });

  testWidgets(
    'filtered project selection uses identity instead of result index',
    (tester) async {
      final controller = memoryController();
      addTearDown(controller.dispose);
      DitchProject? selected;
      controller.setQuery('API');
      await tester.pumpWidget(
        sidebar(controller, onSelect: (project) => selected = project),
      );
      await tester.tap(find.byKey(const ValueKey('project-tile-other')));
      expect(selected, same(other));
      controller.setScope(ProjectScope.remote);
      controller.setQuery('Website');
      await tester.pumpAndSettle();
      await tester.tap(find.text('Website server'));
      expect(selected, same(remote));
    },
  );

  testWidgets(
    'group menu supports create, move, rename and remove without project deletion',
    (tester) async {
      final controller = memoryController();
      addTearDown(controller.dispose);
      await tester.pumpWidget(sidebar(controller));
      await tester.tap(find.byKey(const Key('project-organization-menu')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('New Group…'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('project-group-name')),
        'Frontend',
      );
      await tester.pump();
      await tester.tap(find.text('Create'));
      await tester.pumpAndSettle();
      final group = controller.value.groups.single.id;
      expect(find.text('No projects yet'), findsOneWidget);
      await tester.tap(
        find.byKey(const ValueKey('project-tile-local')),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Move to Group…'));
      await tester.pumpAndSettle();
      await tester.tap(
        find.descendant(
          of: find.byType(SimpleDialog),
          matching: find.text('Frontend'),
        ),
      );
      await tester.pumpAndSettle();
      expect(controller.groupFor(local), group);
      await tester.tap(
        find.byKey(ValueKey('project-group-$group')),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Rename Group…'));
      await tester.pumpAndSettle();
      await tester.enterText(
        find.byKey(const Key('project-group-name')),
        'Product',
      );
      await tester.tap(find.text('Save'));
      await tester.pumpAndSettle();
      expect(find.text('Product'), findsOneWidget);
      await tester.tap(
        find.byKey(ValueKey('project-group-$group')),
        buttons: kSecondaryMouseButton,
      );
      await tester.pumpAndSettle();
      await tester.tap(find.text('Delete Group'));
      await tester.pumpAndSettle();
      expect(controller.groupFor(local), isNull);
      expect(find.text('Website'), findsOneWidget);
      expect(find.text('Projects (2)'), findsOneWidget);
    },
  );

  testWidgets(
    'runtime refresh preserves the selected remote project with a shared local path',
    (tester) async {
      tester.view.physicalSize = const Size(1400, 900);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      const channel = MethodChannel('the_ditch/application');
      final messenger = tester.binding.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(
        channel,
        (call) async => call.method == 'runtimeAvailable' ? true : null,
      );
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      final runtime = _SidebarRuntime();
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: CommandCenterScreen(runtimeClient: runtime),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(
        find.descendant(
          of: find.byKey(const Key('project-scope')),
          matching: find.text('Remote'),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('project-tile-remote')));
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<ProjectSidebar>(find.byType(ProjectSidebar))
            .selectedProjectKey,
        'remote',
      );
      expect(runtime.events.hasListener, isTrue);
      final refreshed = runtime.currentSnapshot;
      ((refreshed['projects'] as List).last as Map)['name'] =
          'Refreshed server';
      runtime.events.add({
        'event': {'SnapshotReplaced': refreshed},
      });
      await tester.pumpAndSettle();
      expect(
        tester
            .widget<ProjectSidebar>(find.byType(ProjectSidebar))
            .selectedProjectKey,
        'remote',
      );
      expect(
        tester
            .widget<ProjectSidebar>(find.byType(ProjectSidebar))
            .projects
            .singleWhere((project) => project.id == 'remote')
            .name,
        'Refreshed server',
      );
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      unawaited(runtime.events.close());
    },
  );

  test(
    'all-project scope and automatic group expansion survive restart',
    () async {
      String? stored;
      final controller = ProjectSidebarController(
        load: () async => stored,
        save: (data) async => stored = data,
      );
      final group = controller.createGroup('Product');
      controller.moveProject(local, group);
      controller.setScope(ProjectScope.all);
      controller.toggleGroup(otherProjectsGroup.id);
      await controller.flush();
      controller.dispose();

      final restored = ProjectSidebarController(
        load: () async => stored,
        save: (_) async {},
      );
      addTearDown(restored.dispose);
      await restored.load();
      expect(restored.value.scope, ProjectScope.all);
      expect(restored.value.collapsed, {otherProjectsGroup.id});
      expect(restored.matchingProjects(projects), [other, local, remote]);
      expect(restored.value.groups.map((g) => g.id), [group]);
      expect(restored.groupFor(remote), isNull);
      restored.setQuery('other projects');
      expect(restored.matchingProjects(projects), [other, remote]);
      restored.setScope(ProjectScope.remote);
      expect(restored.matchingProjects(projects), [remote]);
      restored.setScope(ProjectScope.all);
      restored.revealProject(remote);
      expect(restored.value.scope, ProjectScope.all);
      expect(restored.value.collapsed, isEmpty);
      expect(restored.value.query, isEmpty);
      restored.toggleGroup(otherProjectsGroup.id);
      restored.deleteGroup(group);
      expect(restored.value.collapsed, isEmpty);
      restored.setQuery('other projects');
      expect(restored.matchingProjects(projects), isEmpty);
    },
  );

  testWidgets('clicking either active segment shows all projects', (
    tester,
  ) async {
    final controller = memoryController();
    addTearDown(controller.dispose);
    await tester.pumpWidget(sidebar(controller));
    final segment = find.byKey(const Key('project-scope'));
    await tester.tap(
      find.descendant(of: segment, matching: find.text('Local')),
    );
    await tester.pumpAndSettle();
    expect(controller.value.scope, ProjectScope.all);
    expect(
      tester.widget<SegmentedButton<ProjectScope>>(segment).selected,
      isEmpty,
    );
    expect(find.text('Projects (3)'), findsOneWidget);
    await tester.tap(find.byKey(const Key('open-project-search')));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<TextField>(find.byKey(const Key('project-search-field')))
          .decoration
          ?.hintText,
      'Search all projects',
    );
    await tester.enterText(
      find.byKey(const Key('project-search-field')),
      'Website',
    );
    await tester.pumpAndSettle();
    expect(find.text('Projects (2)'), findsOneWidget);
    await tester.tap(find.byKey(const Key('clear-project-search')));
    await tester.pumpAndSettle();
    await tester.tap(
      find.descendant(of: segment, matching: find.text('Remote')),
    );
    await tester.pumpAndSettle();
    expect(find.text('Projects (1)'), findsOneWidget);
    await tester.tap(
      find.descendant(of: segment, matching: find.text('Remote')),
    );
    await tester.pumpAndSettle();
    expect(controller.value.scope, ProjectScope.all);
    expect(find.text('Projects (3)'), findsOneWidget);
  });

  testWidgets(
    'Other Projects exists only with custom groups and unassigned matches',
    (tester) async {
      final controller = memoryController();
      addTearDown(controller.dispose);
      await tester.pumpWidget(sidebar(controller));
      expect(find.text('Other Projects'), findsNothing);
      final group = controller.createGroup('Product');
      controller.moveProject(local, group);
      await tester.pumpAndSettle();
      final otherRow = find.byKey(
        ValueKey('project-group-${otherProjectsGroup.id}'),
      );
      expect(find.text('Other Projects'), findsOneWidget);
      expect(
        find.descendant(
          of: otherRow,
          matching: find.byType(PopupMenuButton<String>),
        ),
        findsNothing,
      );
      expect(
        tester.getTopLeft(otherRow).dy,
        greaterThan(
          tester.getTopLeft(find.byKey(ValueKey('project-group-$group'))).dy,
        ),
      );
      await tester.tap(otherRow);
      await tester.pumpAndSettle();
      expect(find.text('API'), findsNothing);
      expect(find.text('Projects (2)'), findsOneWidget);
      controller.setQuery('other projects');
      await tester.pumpAndSettle();
      expect(find.text('API'), findsOneWidget);
      expect(find.text('Projects (1)'), findsOneWidget);
      controller.setQuery('');
      await tester.pumpAndSettle();
      expect(find.text('API'), findsNothing);
      controller.moveProject(other, group);
      await tester.pumpAndSettle();
      expect(find.text('Other Projects'), findsNothing);
      controller.moveProject(local, null);
      await tester.pumpAndSettle();
      expect(find.text('Other Projects'), findsOneWidget);
      controller.deleteGroup(group);
      await tester.pumpAndSettle();
      expect(find.text('Other Projects'), findsNothing);
      expect(find.text('API'), findsOneWidget);
      expect(find.text('Website'), findsOneWidget);
    },
  );

  test('active filter combines with search and scope and persists', () async {
    String? stored;
    final controller = ProjectSidebarController(
      load: () async => stored,
      save: (data) async => stored = data,
    );
    final group = controller.createGroup('Product');
    controller.moveProject(local, group);
    controller.moveProject(remote, group);
    controller.setScope(ProjectScope.all);
    controller.setActiveOnly(true);
    controller.setQuery('Product');
    const activeKeys = {'local', 'remote'};
    expect(
      controller.matchingProjects(projects, activeProjectKeys: activeKeys),
      [local, remote],
    );
    controller.setScope(ProjectScope.remote);
    expect(
      controller.matchingProjects(projects, activeProjectKeys: activeKeys),
      [remote],
    );
    expect(
      controller.matchingProjects(projects, activeProjectKeys: {'local'}),
      isEmpty,
    );
    await controller.flush();
    controller.dispose();
    final restored = ProjectSidebarController(
      load: () async => stored,
      save: (_) async {},
    );
    addTearDown(restored.dispose);
    await restored.load();
    expect(restored.value.activeOnly, isTrue);
    restored.revealProject(other);
    expect(restored.value.activeOnly, isFalse);
  });

  testWidgets(
    'group badges count active projects and follow live activity and filters',
    (tester) async {
      final controller = memoryController();
      addTearDown(controller.dispose);
      final group = controller.createGroup('Client Suite');
      controller.createGroup('Empty group');
      controller.moveProject(local, group);
      controller.moveProject(other, group);
      controller.setScope(ProjectScope.all);
      final running = {'local': 3, 'other': 0, 'remote': 2};
      await tester.pumpWidget(sidebar(controller, runningCounts: running));
      final badge = find.byKey(ValueKey('project-group-activity-$group'));
      final otherBadge = find.byKey(
        ValueKey('project-group-activity-${otherProjectsGroup.id}'),
      );
      expect(tester.widget<ProjectActivityIndicator>(badge).count, 1);
      expect(tester.widget<ProjectActivityIndicator>(otherBadge).count, 1);
      expect(find.text('Projects (3)'), findsOneWidget);
      controller.toggleGroup(group);
      await tester.pumpAndSettle();
      expect(find.text('Website'), findsNothing);
      expect(badge, findsOneWidget);
      await tester.tap(find.byKey(const Key('project-active-filter')));
      await tester.pumpAndSettle();
      expect(find.text('Projects (2)'), findsOneWidget);
      expect(find.text('Empty group'), findsNothing);
      expect(
        tester
            .widget<IconButton>(find.byKey(const Key('project-active-filter')))
            .isSelected,
        isTrue,
      );
      expect(
        tester
            .widget<ProjectSidebar>(find.byType(ProjectSidebar))
            .selectedProjectKey,
        local.id,
      );
      controller.setScope(ProjectScope.remote);
      await tester.pumpAndSettle();
      expect(find.text('Projects (1)'), findsOneWidget);
      expect(badge, findsNothing);
      expect(otherBadge, findsOneWidget);
      controller.setScope(ProjectScope.all);
      controller.setQuery('Client Suite');
      await tester.pumpAndSettle();
      expect(find.text('Projects (1)'), findsOneWidget);
      expect(find.text('Website'), findsOneWidget);
      expect(otherBadge, findsNothing);
      running['local'] = 0;
      await tester.pumpWidget(sidebar(controller, runningCounts: running));
      await tester.pumpAndSettle();
      expect(badge, findsNothing);
      expect(find.text('Projects (0)'), findsOneWidget);
      controller.setQuery('');
      await tester.pumpAndSettle();
      expect(find.text('Projects (1)'), findsOneWidget);
      await tester.tap(find.byKey(const Key('project-active-filter')));
      await tester.pumpAndSettle();
      expect(find.text('Projects (3)'), findsOneWidget);
      expect(badge, findsNothing);
      expect(otherBadge, findsOneWidget);
    },
  );

  testWidgets(
    'activity dot pulses without resizing and respects reduced motion',
    (tester) async {
      Widget indicator({required bool reduceMotion}) => MaterialApp(
        theme: DitchTheme.light(),
        home: MediaQuery(
          data: MediaQueryData(disableAnimations: reduceMotion),
          child: const Scaffold(body: ProjectActivityIndicator(count: 2)),
        ),
      );
      await tester.pumpWidget(indicator(reduceMotion: false));
      final fade = find.descendant(
        of: find.byType(ProjectActivityIndicator),
        matching: find.byType(FadeTransition),
      );
      final size = tester.getSize(fade);
      final initial = tester.widget<FadeTransition>(fade).opacity.value;
      await tester.pump(const Duration(milliseconds: 450));
      expect(tester.widget<FadeTransition>(fade).opacity.value, isNot(initial));
      expect(tester.getSize(fade), size);
      expect(size, const Size(6, 6));
      expect(find.byTooltip('2 projects have running agents'), findsOneWidget);
      await tester.pumpWidget(indicator(reduceMotion: true));
      await tester.pumpAndSettle();
      expect(tester.widget<FadeTransition>(fade).opacity.value, 1);
      await tester.pump(const Duration(seconds: 2));
      expect(tester.widget<FadeTransition>(fade).opacity.value, 1);
      await tester.pumpWidget(const SizedBox.shrink());
      expect(tester.takeException(), isNull);
    },
  );

  for (final width in [160.0, 240.0, 520.0]) {
    for (final dark in [false, true]) {
      testWidgets(
        'sidebar fits $width px in ${dark ? 'dark' : 'light'} theme',
        (tester) async {
          final controller = memoryController();
          addTearDown(controller.dispose);
          final group = controller.createGroup('A long project group name');
          controller.moveProject(remote, group);
          controller.setScope(ProjectScope.remote);
          await tester.pumpWidget(
            sidebar(controller, width: width, dark: dark),
          );
          await tester.pumpAndSettle();
          expect(tester.takeException(), isNull);
          expect(find.byType(RemoteProjectTag), findsOneWidget);
          await tester.tap(find.byKey(const Key('open-project-search')));
          await tester.pumpAndSettle();
          expect(
            tester.getSize(find.byKey(const Key('project-search-field'))).width,
            width - 24,
          );
          expect(tester.takeException(), isNull);
        },
      );
    }
  }
}
