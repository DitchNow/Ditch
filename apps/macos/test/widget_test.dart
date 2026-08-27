import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/application/command_center_controller.dart';
import 'package:the_ditch/data/runtime_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

final _testAgentHeaderKeys = <String, GlobalKey>{};

GlobalKey _testAgentHeaderKey(String agentId) =>
    _testAgentHeaderKeys.putIfAbsent(agentId, GlobalKey.new);

const _testProject = DitchProject(
  name: 'Ditch',
  path: '/tmp/the-ditch-test-project',
);

Widget _testApp() => const TheDitchApp(
  connectRuntimeOnStart: false,
  initialProjects: [_testProject],
);

void main() {
  test('agent execution settings expose model and approval controls', () {
    final settings = AgentExecutionSettings();

    expect(settings.approval, AgentApprovalPreset.approveForMe);
    expect(settings.protocolValue.containsKey('network_access'), isFalse);
  });

  test('presentation controller publishes immutable connection states', () {
    final controller = CommandCenterController();
    addTearDown(controller.dispose);
    final observed = <RuntimeConnectionPhase>[];
    controller.addListener(() => observed.add(controller.value.connection));

    controller.connected();
    controller.connecting(reconnecting: true);
    controller.unavailable('fixture offline');

    expect(observed, [
      RuntimeConnectionPhase.connected,
      RuntimeConnectionPhase.reconnecting,
      RuntimeConnectionPhase.unavailable,
    ]);
    expect(controller.value.connectionError, contains('fixture offline'));
  });

  test('presentation controller keeps panel state independent of runtime', () {
    final controller = CommandCenterController();
    addTearDown(controller.dispose);

    controller.toggleSidebar();
    controller.toggleInspector();

    expect(controller.value.sidebarVisible, isFalse);
    expect(controller.value.inspectorVisible, isFalse);
    expect(controller.value.connection, RuntimeConnectionPhase.connecting);
  });

  test('runtime parser accepts unit Accepted responses', () {
    final parsed = parseRuntimeResponseLine(
      '{"protocol_version":1,"id":"00000000-0000-4000-8000-000000000000","sent_at":"2026-01-01T00:00:00Z","body":"Accepted"}',
    );

    expect(parsed, {'Accepted': true});
  });

  test('runtime status DTO validates and types protocol fields', () {
    final status = RuntimeStatusDto.fromResponse({
      'RuntimeStatus': {
        'identity': 'The Ditch Runtime',
        'pid': 42,
        'socket_path': '/tmp/ditchd.sock',
        'active_session_count': 2,
        'attention_count': 1,
        'unread_attention_count': 1,
        'instance_id': 'instance-1',
        'codex_home': '/tmp/codex',
        'codex_binary': '/opt/homebrew/bin/codex',
        'build_version': '1.0.0',
        'capabilities': ['persistent_sessions_v1', 'always_on_web_access_v1'],
      },
    });

    expect(status.pid, 42);
    expect(status.activeSessionCount, 2);
    expect(status.unreadAttentionCount, 1);
    expect(status.codexBinary, '/opt/homebrew/bin/codex');
    expect(status.supportsPersistentSessions, isTrue);
    expect(status.supportsAlwaysOnWebAccess, isTrue);
  });

  test('Codex readiness requires compatibility and authentication', () {
    final report = CodexReadinessReport.fromResponse({
      'CodexReadiness': {
        'path': '/opt/homebrew/bin/codex',
        'version': 'codex-cli 1.2.3',
        'compatible': true,
        'authenticated': false,
        'update_supported': true,
        'doctor_supported': true,
        'issues': ['Codex is not signed in for this user.'],
        'diagnostics': '{}',
      },
    });

    expect(report.ready, isFalse);
    expect(report.updateSupported, isTrue);
    expect(report.issues, hasLength(1));
  });

  test('runtime project parser restores path and Git policy', () {
    final project = parseRuntimeProject({
      'name': 'Recovered',
      'root': '/tmp/recovered',
      'git_policy': 'AllowOutsideGit',
    });

    expect(project, isNotNull);
    expect(project!.name, 'Recovered');
    expect(project.path, '/tmp/recovered');
    expect(project.gitPolicy, ProjectGitPolicy.allowOutsideGit);
  });

  test('runtime project parser preserves remote execution identity', () {
    final project = parseRuntimeProject({
      'id': 'remote-project',
      'name': 'FieldOps',
      'root': '/home/mtn/fieldops',
      'git_policy': 'RequireRepository',
      'execution_target': {
        'kind': 'remote',
        'remote_machine_id': '11111111-1111-4111-8111-111111111111',
        'ssh_host_alias': 'dev-box',
      },
    });

    expect(project, isNotNull);
    expect(project!.isRemote, isTrue);
    expect(project.sshHostAlias, 'dev-box');
    expect(project.path, '/home/mtn/fieldops');
  });

  test('project reconciliation replaces duplicate ids and paths', () {
    final projects = <DitchProject>[
      const DitchProject(
        id: 'project-11',
        name: 'Old',
        path: '/tmp/project-11',
      ),
    ];

    upsertProject(
      projects,
      const DitchProject(
        id: 'project-11',
        name: 'Project#11',
        path: '/tmp/project-11',
      ),
    );

    expect(projects, hasLength(1));
    expect(projects.single.name, 'Project#11');
  });

  test('agent reconciliation removes duplicate runtime ids', () {
    final first = AgentSession(
      localId: 'agent-1',
      provider: AgentProvider.codex,
      status: AgentStatus.failed,
      messages: const [],
    );
    final duplicate = AgentSession(
      localId: 'agent-1',
      provider: AgentProvider.codex,
      status: AgentStatus.starting,
      messages: const [],
    );
    final sessions = [first, duplicate];
    final incoming = AgentSession(
      localId: 'agent-1',
      provider: AgentProvider.codex,
      status: AgentStatus.working,
      messages: const [],
      currentPrompt: 'try again',
    );

    reconcileAgentSession(sessions, incoming);

    expect(sessions, hasLength(1));
    expect(sessions.single.status, AgentStatus.working);
    expect(sessions.single.currentPrompt, 'try again');
  });

  test('live and persisted copies of one prompt reconcile exactly once', () {
    final createdAt = DateTime.utc(2026, 8, 19, 20, 43);
    final live = AgentChatMessage(
      identity: 'agent-1:$createdAt:user',
      role: ChatMessageRole.user,
      text: 'Run the task',
      createdAt: createdAt,
    );
    final persisted = AgentChatMessage(
      identity: 'persisted:agent-1:1',
      role: ChatMessageRole.user,
      text: 'Run the task',
      createdAt: createdAt,
    );
    final deliberatelyRepeated = AgentChatMessage(
      identity: 'persisted:agent-1:2',
      role: ChatMessageRole.user,
      text: 'Run the task',
      createdAt: createdAt.add(const Duration(seconds: 1)),
    );

    expect(uniqueRuntimeMessages([live], [persisted]), isEmpty);
    expect(uniqueRuntimeMessages([live], [persisted, deliberatelyRepeated]), [
      deliberatelyRepeated,
    ]);
  });

  test('groups tool messages by user turn with stable visible identity', () {
    final messages = [
      AgentChatMessage(
        identity: 'user-1',
        role: ChatMessageRole.user,
        text: 'First task',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'tool-1',
        role: ChatMessageRole.tool,
        text: 'command one',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'tool-2',
        role: ChatMessageRole.tool,
        text: 'command two',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'assistant-1',
        role: ChatMessageRole.assistant,
        text: 'Finished',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'user-2',
        role: ChatMessageRole.user,
        text: 'Second task',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'tool-3',
        role: ChatMessageRole.tool,
        text: 'command three',
        createdAt: DateTime(2026),
      ),
    ];

    final items = buildConversationItems(messages, isWorking: true);

    expect(items, hasLength(5));
    expect(items[1].identity, 'tool-activity:tool-1');
    expect(items[1].toolMessages, hasLength(2));
    expect(items[1].isActiveToolGroup, isFalse);
    expect(items[4].identity, 'tool-activity:tool-3');
    expect(items[4].isActiveToolGroup, isTrue);
  });

  testWidgets('tool activity collapses when the active turn finishes', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    addTearDown(viewport.dispose);
    var working = true;
    late StateSetter rebuild;
    final messages = [
      AgentChatMessage(
        identity: 'group-user',
        role: ChatMessageRole.user,
        text: 'Run checks',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'group-tool-1',
        role: ChatMessageRole.tool,
        text: 'cargo test',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'group-tool-2',
        role: ChatMessageRole.tool,
        text: 'flutter test',
        createdAt: DateTime(2026),
      ),
    ];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return ConversationTranscript(
                messages: messages,
                viewport: viewport,
                isWorking: working,
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();

    expect(find.text('Tool activity · 2 actions'), findsOneWidget);
    expect(find.text('cargo test'), findsOneWidget);
    expect(find.text('flutter test'), findsOneWidget);

    rebuild(() => working = false);
    await tester.pump();
    expect(find.text('cargo test'), findsNothing);
    expect(find.text('flutter test'), findsNothing);

    await tester.tap(find.byKey(const Key('tool-activity-toggle')));
    await tester.pump();
    expect(find.text('cargo test'), findsOneWidget);
    expect(find.text('flutter test'), findsOneWidget);
  });

  test('sessions and attention are scoped to their project', () {
    final sessions = [
      AgentSession(
        localId: 'agent-a',
        projectId: 'project-a',
        provider: AgentProvider.codex,
        status: AgentStatus.failed,
        messages: const [],
      ),
      AgentSession(
        localId: 'agent-b',
        projectId: 'project-b',
        provider: AgentProvider.codex,
        status: AgentStatus.failed,
        messages: const [],
      ),
    ];
    final attention = [
      AttentionEvent(
        id: 'global',
        kind: AttentionKind.failed,
        icon: Icons.error_outline,
        title: 'Runtime',
        body: 'Global failure',
        createdAt: DateTime(2026),
      ),
      AttentionEvent(
        id: 'project-a-alert',
        projectId: 'project-a',
        kind: AttentionKind.failed,
        icon: Icons.error_outline,
        title: 'Codex',
        body: 'Project failure',
        createdAt: DateTime(2026),
      ),
    ];

    expect(
      sessionsForProject(sessions, 'project-a').map((item) => item.localId),
      ['agent-a'],
    );
    expect(attentionForProject(attention, 'project-b').map((item) => item.id), [
      'global',
    ]);
  });

  test('project summaries count agents and unread terminal results', () {
    final sessions = [
      AgentSession(
        localId: 'working',
        projectId: 'project-a',
        provider: AgentProvider.codex,
        status: AgentStatus.working,
        messages: const [],
      ),
      AgentSession(
        localId: 'completed',
        projectId: 'project-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
      AgentSession(
        localId: 'other',
        projectId: 'project-b',
        provider: AgentProvider.codex,
        status: AgentStatus.failed,
        messages: const [],
      ),
    ];
    final attention = [
      AttentionEvent(
        id: 'finished-a',
        sessionLocalId: 'completed',
        projectId: 'project-a',
        kind: AttentionKind.completed,
        icon: Icons.check_circle_outline,
        title: 'Finished',
        body: 'Done',
        createdAt: DateTime(2026),
      ),
    ];

    final unread = summarizeProjectAgents(
      sessions: sessions,
      attention: attention,
      projectId: 'project-a',
    );
    expect(unread.runningCount, 1);
    expect(unread.stoppedCount, 1);
    expect(unread.hasUnreadResult, isTrue);

    final read = summarizeProjectAgents(
      sessions: sessions,
      attention: attention,
      projectId: 'project-a',
      readAttentionIds: {'finished-a'},
    );
    expect(read.hasUnreadResult, isFalse);

    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'completed',
      ),
      {'finished-a'},
    );
    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'working',
      ),
      isEmpty,
    );
  });

  test('reading one agent result leaves other project results unread', () {
    final attention = [
      AttentionEvent(
        id: 'result-a',
        sessionLocalId: 'agent-a',
        projectId: 'project-a',
        kind: AttentionKind.completed,
        icon: Icons.check_circle_outline,
        title: 'A finished',
        body: 'Done',
        createdAt: DateTime(2026),
      ),
      AttentionEvent(
        id: 'result-b',
        sessionLocalId: 'agent-b',
        projectId: 'project-a',
        kind: AttentionKind.failed,
        icon: Icons.error_outline,
        title: 'B failed',
        body: 'Failed',
        createdAt: DateTime(2026),
      ),
    ];

    expect(
      summarizeProjectAgents(
        sessions: const [],
        attention: attention,
        projectId: 'project-a',
        readAttentionIds: {'result-a'},
      ).hasUnreadResult,
      isTrue,
    );
    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'agent-a',
        readAttentionIds: {'result-a'},
      ),
      isEmpty,
    );
    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'agent-b',
        readAttentionIds: {'result-a'},
      ),
      {'result-b'},
    );
  });

  test('notification navigation preserves exact project and agent ids', () {
    final target = AgentNotificationTarget.fromArguments({
      'projectId': 'project-a',
      'agentId': 'agent-b',
      'attentionId': 'attention-c',
    });

    expect(target, isNotNull);
    expect(target!.projectId, 'project-a');
    expect(target.agentId, 'agent-b');
    expect(target.attentionId, 'attention-c');
    expect(
      AgentNotificationTarget.fromArguments({'projectId': 'project-a'}),
      isNull,
    );
  });

  testWidgets('renders command center shell', (tester) async {
    await tester.pumpWidget(_testApp());

    expect(find.text('Ditch'), findsWidgets);
    expect(find.text('PROJECTS'), findsOneWidget);
    expect(find.text('Agents'), findsOneWidget);
    expect(find.text('Attention'), findsNothing);
    expect(find.byKey(const Key('notification-bell')), findsOneWidget);
    expect(find.text('New Agent'), findsOneWidget);
  });

  testWidgets('fresh install starts with Codex readiness onboarding', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));
    await tester.pumpAndSettle();

    expect(find.text('Welcome to Ditch'), findsOneWidget);
    expect(find.byKey(const Key('onboarding-continue')), findsOneWidget);
    expect(find.byKey(const Key('first-project-add')), findsNothing);
    expect(find.widgetWithText(AlertDialog, 'Add Project'), findsNothing);
    expect(find.text('/Users/tester/Projects/example'), findsNothing);
    expect(find.text('PROJECTS'), findsNothing);
  });

  testWidgets('first project action remains gated until Codex is ready', (
    tester,
  ) async {
    var addCalls = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: CodexOnboardingView(
          introduced: true,
          checking: false,
          updating: false,
          readiness: const CodexReadinessReport(
            path: '/opt/homebrew/bin/codex',
            version: 'codex-cli 1.2.3',
            compatible: true,
            authenticated: true,
            updateSupported: true,
            doctorSupported: true,
            issues: [],
            diagnostics: '{}',
          ),
          error: null,
          notificationReadiness: const NotificationReadiness(
            authorization: NotificationAuthorizationState.authorized,
            alertsEnabled: true,
            notificationCenterEnabled: true,
            soundsEnabled: true,
          ),
          notificationChecking: false,
          notificationError: null,
          onContinue: () {},
          onCheckAgain: () {},
          onChooseInstallation: () {},
          onUpdate: () {},
          onSignIn: () {},
          onOpenInstallInstructions: () {},
          onManageNotifications: () {},
          onCheckNotifications: () {},
          onAddProject: () => addCalls += 1,
        ),
      ),
    );

    expect(find.text('Codex is ready'), findsOneWidget);
    await tester.tap(find.byKey(const Key('first-project-add')));
    expect(addCalls, 1);
  });

  testWidgets('first project remains gated until notifications are enabled', (
    tester,
  ) async {
    var notificationCalls = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: CodexOnboardingView(
          introduced: true,
          checking: false,
          updating: false,
          readiness: const CodexReadinessReport(
            path: '/opt/homebrew/bin/codex',
            version: 'codex-cli 1.2.3',
            compatible: true,
            authenticated: true,
            updateSupported: true,
            doctorSupported: true,
            issues: [],
            diagnostics: '{}',
          ),
          error: null,
          notificationReadiness: const NotificationReadiness(
            authorization: NotificationAuthorizationState.notDetermined,
            alertsEnabled: false,
            notificationCenterEnabled: false,
            soundsEnabled: false,
          ),
          notificationChecking: false,
          notificationError: null,
          onContinue: () {},
          onCheckAgain: () {},
          onChooseInstallation: () {},
          onUpdate: () {},
          onSignIn: () {},
          onOpenInstallInstructions: () {},
          onManageNotifications: () => notificationCalls += 1,
          onCheckNotifications: () {},
          onAddProject: () {},
        ),
      ),
    );

    expect(find.byKey(const Key('first-project-add')), findsNothing);
    await tester.tap(find.byKey(const Key('enable-notifications')));
    expect(notificationCalls, 1);
  });

  testWidgets('runtime failure has a dedicated recovery surface', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: RuntimeRecoveryView(
          socketPath: '/tmp/ditchd.sock',
          error: 'connection refused',
          onRetry: () {},
          onOpenActivityMonitor: () async => true,
          onQuit: () async => true,
        ),
      ),
    );

    expect(find.text('Ditch Runtime is not responding'), findsOneWidget);
    expect(find.text('Retry Connection'), findsOneWidget);
    expect(find.text('Open Activity Monitor'), findsOneWidget);
    expect(find.text('Quit UI'), findsOneWidget);
    expect(find.text('/tmp/ditchd.sock'), findsOneWidget);
  });

  testWidgets('notification center starts empty without activity feed noise', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.byKey(const Key('notification-bell')));
    await tester.pump();
    expect(find.text('No notifications'), findsOneWidget);
    expect(find.text('Bells enabled'), findsNothing);
    expect(find.text('Codex prompted'), findsNothing);
    expect(find.text('Codex started'), findsNothing);
  });

  testWidgets('a project without saved sessions shows a ready agent card', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: const [],
            expandedAgentLocalId: null,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: 'Start here',
            onStartCodex: () {},
            onStartPrompt: (_) {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byKey(const Key('ready-agent-card')), findsOneWidget);
    expect(find.text('Ready for a new prompt'), findsOneWidget);
  });

  testWidgets('failed session without a Codex thread is read-only', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    var submitted = false;
    final session = AgentSession(
      localId: 'failed-agent',
      projectId: 'project-a',
      provider: AgentProvider.codex,
      status: AgentStatus.failed,
      messages: [
        AgentChatMessage(
          role: ChatMessageRole.system,
          text: 'Codex exited with code 1',
          createdAt: DateTime(2026),
        ),
      ],
      exitCode: 1,
      finishedAt: DateTime(2026),
      resumeBlockReason: 'NoCodexThread',
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: 'Retry',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) => submitted = true,
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.textContaining('Codex never created a thread'), findsOneWidget);
    expect(
      tester
          .widget<NativeComposerTextView>(find.byType(NativeComposerTextView))
          .enabled,
      isFalse,
    );
    expect(submitted, isFalse);
    expect(find.text('Codex exited with code 1'), findsOneWidget);
  });

  testWidgets('notification bell exposes global session actions', (
    tester,
  ) async {
    var opened = false;
    var dismissed = false;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.topRight,
            child: NotificationCenterButton(
              notifications: [
                AttentionEvent(
                  id: 'attention-test',
                  kind: AttentionKind.failed,
                  icon: Icons.error_outline,
                  title: 'Codex failed',
                  body: 'Exit code: 1.',
                  sessionLocalId: 'agent-0',
                  projectName: 'The Ditch',
                  agentName: 'Build agent',
                  createdAt: DateTime(2026),
                ),
              ],
              unreadCount: 1,
              onViewed: () {},
              onOpen: (_) => opened = true,
              onDismiss: (_) => dismissed = true,
              onDismissAll: () {},
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(const Key('notification-bell')));
    await tester.pump();
    expect(find.text('Codex failed'), findsOneWidget);
    expect(find.text('The Ditch · Build agent'), findsOneWidget);
    expect(find.text('Open'), findsOneWidget);
    expect(find.byTooltip('Dismiss notification'), findsOneWidget);

    await tester.tap(find.text('Open'));
    await tester.pump();
    expect(opened, isTrue);

    await tester.tap(find.byKey(const Key('notification-bell')));
    await tester.pump();
    await tester.tap(find.byTooltip('Dismiss notification'));
    await tester.pump();
    expect(dismissed, isTrue);
  });

  testWidgets('chat messages expose copy actions', (tester) async {
    final session = AgentSession(
      localId: 'copy-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: [
        AgentChatMessage(
          role: ChatMessageRole.assistant,
          text: 'Copy this response',
          createdAt: DateTime(2026),
        ),
      ],
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byTooltip('Copy message'), findsOneWidget);
    expect(find.byTooltip('Copy conversation'), findsOneWidget);
    expect(find.byType(SelectionArea), findsWidgets);
  });

  testWidgets('focused agent view exposes return and delete controls', (
    tester,
  ) async {
    final session = AgentSession(
      localId: 'focus-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: const [],
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: session.localId,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byTooltip('Return to agents (Esc)'), findsOneWidget);
    expect(find.byTooltip('Delete agent permanently'), findsOneWidget);
  });

  testWidgets('long conversation retains native scrolling', (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final chatController = ScrollController();
    final agentListController = ScrollController();
    final session = AgentSession(
      localId: 'scroll-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: List.generate(
        30,
        (index) => AgentChatMessage(
          role: ChatMessageRole.assistant,
          text: 'Message $index with enough text to occupy a chat row.',
          createdAt: DateTime(2026),
        ),
      ),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(
              scrollController: chatController,
            ),
            agentListController: agentListController,
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(chatController.position.maxScrollExtent, greaterThan(0));
    expect(chatController.offset, chatController.position.minScrollExtent);
    expect(
      find.text('Message 29 with enough text to occupy a chat row.'),
      findsOneWidget,
    );
    expect(
      find.text('Message 0 with enough text to occupy a chat row.'),
      findsNothing,
    );
    final header = find.byType(InkWell).first;
    final composer = find.byType(AgentComposer);
    final headerTop = tester.getTopLeft(header);
    final composerTop = tester.getTopLeft(composer);
    await tester.drag(find.byType(ListView).last, const Offset(0, 300));
    await tester.pumpAndSettle();
    expect(chatController.offset, greaterThan(0));
    expect(tester.getTopLeft(header), headerTop);
    expect(tester.getTopLeft(composer), composerTop);
  });

  testWidgets('detached conversation counts new messages without jumping', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    final messages = List.generate(
      30,
      (index) => AgentChatMessage(
        identity: 'message-$index',
        role: ChatMessageRole.assistant,
        text: 'Message $index with enough text to occupy a row.',
        createdAt: DateTime(2026),
      ),
    );
    String? historyError;
    late StateSetter rebuild;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return AgentsSurface(
                sessions: [
                  AgentSession(
                    localId: 'detached-agent',
                    provider: AgentProvider.codex,
                    status: AgentStatus.completed,
                    messages: messages,
                    hasOlderMessages: true,
                    historyError: historyError,
                  ),
                ],
                expandedAgentLocalId: 'detached-agent',
                focusedAgentLocalId: null,
                chatViewport: viewport,
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                onFocusAgent: (_) {},
                onToggleExpanded: (_) {},
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, 300));
    await tester.pumpAndSettle();
    expect(viewport.mode, ConversationViewportMode.detached);
    final detachedOffset = viewport.scrollController.offset;

    rebuild(() {
      messages.add(
        AgentChatMessage(
          identity: 'message-30',
          role: ChatMessageRole.assistant,
          text: 'Newest message',
          createdAt: DateTime(2026),
        ),
      );
    });
    await tester.pump();
    await tester.pump();

    expect(
      viewport.scrollController.offset,
      greaterThanOrEqualTo(detachedOffset),
    );
    expect(find.text('1 new message'), findsOneWidget);
    await tester.tap(find.byKey(const Key('conversation-new-messages')));
    await tester.pumpAndSettle();
    expect(viewport.mode, ConversationViewportMode.following);
    expect(viewport.unseenCount, 0);
    expect(viewport.scrollController.offset, 0);

    await tester.drag(find.byType(ListView), const Offset(0, 300));
    await tester.pumpAndSettle();
    final beforeOlderHistory = viewport.scrollController.offset;
    rebuild(() {
      messages.insert(
        0,
        AgentChatMessage(
          identity: 'message-older',
          role: ChatMessageRole.assistant,
          text: 'Older history',
          createdAt: DateTime(2025),
        ),
      );
    });
    await tester.pump();
    expect(viewport.unseenCount, 0);
    expect(
      viewport.scrollController.offset,
      moreOrLessEquals(beforeOlderHistory),
    );

    viewport.scrollController.jumpTo(
      viewport.scrollController.position.maxScrollExtent,
    );
    await tester.pump();
    final beforeHistoryFailure = viewport.scrollController.offset;
    rebuild(() => historyError = 'Could not load message history.');
    await tester.pump();
    expect(find.byKey(const Key('conversation-history-retry')), findsOneWidget);
    expect(
      viewport.scrollController.offset,
      moreOrLessEquals(beforeHistoryFailure),
    );
  });

  testWidgets('focused mode preserves the active agent reading position', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    final session = AgentSession(
      localId: 'focus-scroll-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: List.generate(
        30,
        (index) => AgentChatMessage(
          identity: 'focus-message-$index',
          role: ChatMessageRole.assistant,
          text: 'Message $index with enough text to occupy a row.',
          createdAt: DateTime(2026),
        ),
      ),
    );
    var focused = false;
    late StateSetter rebuild;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return AgentsSurface(
                sessions: [session],
                expandedAgentLocalId: session.localId,
                focusedAgentLocalId: focused ? session.localId : null,
                chatViewport: viewport,
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                onFocusAgent: (_) {},
                onToggleExpanded: (_) {},
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, 300));
    await tester.pumpAndSettle();
    final detachedOffset = viewport.scrollController.offset;

    rebuild(() => focused = true);
    await tester.pump();

    expect(viewport.mode, ConversationViewportMode.detached);
    expect(viewport.scrollController.offset, moreOrLessEquals(detachedOffset));
  });

  testWidgets('opens add project dialog', (tester) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();

    expect(find.widgetWithText(AlertDialog, 'Add Project'), findsOneWidget);
    expect(find.text('Local Project'), findsOneWidget);
    expect(find.text('Remote Project'), findsOneWidget);
    await tester.tap(find.text('Local Project'));
    await tester.pumpAndSettle();
    expect(find.text('Browse Folder…'), findsOneWidget);
    expect(find.text('Project name'), findsOneWidget);
    expect(find.text('Selected folder'), findsOneWidget);
    expect(find.text('Add & Configure'), findsOneWidget);
    expect(find.textContaining('.ditch/hooks'), findsOneWidget);
  });

  testWidgets('add project modal steps dismiss when the backdrop is clicked', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();
    await tester.tapAt(const Offset(4, 4));
    await tester.pumpAndSettle();
    expect(find.text('Local Project'), findsNothing);

    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Local Project'));
    await tester.pumpAndSettle();
    expect(find.text('Browse Folder…'), findsOneWidget);
    await tester.tapAt(const Offset(4, 4));
    await tester.pumpAndSettle();
    expect(find.text('Browse Folder…'), findsNothing);
  });

  testWidgets('folder picker fills the project path and inferred name', (
    tester,
  ) async {
    const channel = MethodChannel('the_ditch/project_picker');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      (call) async => '/tmp/My Project',
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );

    await tester.pumpWidget(_testApp());
    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Local Project'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Browse Folder…'));
    await tester.pumpAndSettle();

    expect(find.text('/tmp/My Project'), findsOneWidget);
    expect(find.text('My Project'), findsOneWidget);
    expect(find.text('Choose how Codex should run'), findsOneWidget);
    expect(
      find.text('Choose how this project should handle Git.'),
      findsOneWidget,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Add & Configure'),
          )
          .onPressed,
      isNull,
    );
    await tester.tap(find.byType(DropdownButtonFormField<ProjectGitPolicy>));
    await tester.pumpAndSettle();
    expect(find.text('Initialize Git Repository'), findsOneWidget);
    expect(find.text('Allow Codex Outside Git'), findsOneWidget);
    expect(find.textContaining('--skip-git-repo-check'), findsOneWidget);
    await tester.tap(find.text('Initialize Git Repository'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Add & Configure'),
          )
          .onPressed,
      isNotNull,
    );
  });

  testWidgets('start codex opens an initial prompt dialog', (tester) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.text('New Agent'));
    await tester.pumpAndSettle();

    expect(
      find.widgetWithText(AlertDialog, 'Start Codex Session'),
      findsOneWidget,
    );
    expect(find.text('Initial prompt'), findsOneWidget);
    final dialog = find.byType(StartCodexSessionDialog);
    expect(
      tester
          .widget<TextField>(
            find.descendant(of: dialog, matching: find.byType(TextField)),
          )
          .controller
          ?.text,
      isEmpty,
    );

    await tester.enterText(
      find.descendant(of: dialog, matching: find.byType(TextField)),
      'new initial prompt',
    );
    await tester.pumpAndSettle();

    expect(
      find.descendant(of: dialog, matching: find.text('new initial prompt')),
      findsOneWidget,
    );
  });

  testWidgets('new agent action lives in the Agents header', (tester) async {
    await tester.pumpWidget(_testApp());

    expect(find.byKey(const Key('agents-new-agent-button')), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(DitchToolbar),
        matching: find.text('New Agent'),
      ),
      findsNothing,
    );
  });

  testWidgets('terminal supports horizontal and vertical workspace expansion', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(_testApp());

    await tester.tap(find.byKey(const Key('terminal-expand-horizontal')));
    await tester.pump();
    expect(find.byType(ProjectTerminalSurface), findsOneWidget);
    expect(
      tester.getTopLeft(find.byType(ProjectTerminalSurface)).dy,
      lessThan(tester.getTopLeft(find.text('Agents')).dy),
    );

    await tester.tap(find.byKey(const Key('terminal-expand-vertical')));
    await tester.pump();
    expect(find.byType(ProjectTerminalSurface), findsOneWidget);
    expect(find.text('Attention'), findsNothing);
  });

  testWidgets('project file tree opens directories and text files', (
    tester,
  ) async {
    final files = ProjectFilesState()
      ..directories[''] = const [
        ProjectFileEntry(
          name: 'lib',
          relativePath: 'lib',
          kind: ProjectFileEntryKind.directory,
          size: 0,
        ),
        ProjectFileEntry(
          name: 'README.md',
          relativePath: 'README.md',
          kind: ProjectFileEntryKind.file,
          size: 10,
        ),
      ];
    ProjectFileEntry? toggled;
    ProjectFileEntry? opened;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectFilesBody(
            files: files,
            presentation: TerminalPresentation.docked,
            onToggleDirectory: (entry) => toggled = entry,
            onOpenFile: (entry) => opened = entry,
            onRevealFile: (_) {},
            onBack: () {},
            onSave: () {},
            onReload: () {},
            onOverwrite: () {},
            onPresentationChanged: (_) {},
          ),
        ),
      ),
    );

    await tester.tap(find.text('lib'));
    await tester.tap(find.text('README.md'));
    expect(toggled?.relativePath, 'lib');
    expect(opened?.relativePath, 'README.md');
    files.dispose();
  });

  testWidgets('project editor edits saves and exposes every presentation', (
    tester,
  ) async {
    final document = ProjectEditorDocument(
      relativePath: 'lib/main.dart',
      content: 'before',
      revision: 'revision-1',
    );
    document.controller.text = 'after';
    final presentations = <TerminalPresentation>[];
    var saves = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectFileEditorBody(
            document: document,
            presentation: TerminalPresentation.docked,
            onBack: () {},
            onSave: () => saves++,
            onReload: () {},
            onOverwrite: () {},
            onPresentationChanged: presentations.add,
          ),
        ),
      ),
    );

    expect(document.dirty, isTrue);
    await tester.tap(find.byKey(const Key('editor-save')));
    await tester.tap(find.byKey(const Key('editor-expand-horizontal')));
    await tester.tap(find.byKey(const Key('editor-expand-vertical')));
    await tester.tap(find.byKey(const Key('editor-maximize')));
    expect(saves, 1);
    expect(presentations, [
      TerminalPresentation.horizontal,
      TerminalPresentation.vertical,
      TerminalPresentation.maximized,
    ]);
    document.dispose();
  });

  testWidgets('maximized terminal restores with close or Escape', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(_testApp());

    await tester.tap(find.byKey(const Key('terminal-maximize')));
    await tester.pump();
    expect(find.byKey(const Key('terminal-maximize-close')), findsOneWidget);
    expect(find.byType(AgentsSurface), findsNothing);

    await tester.tap(find.byKey(const Key('terminal-maximize-close')));
    await tester.pump();
    expect(find.byType(AgentsSurface), findsOneWidget);

    await tester.tap(find.byKey(const Key('terminal-maximize')));
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    expect(find.byType(AgentsSurface), findsOneWidget);
  });

  testWidgets('workspace splitters resize both side panes', (tester) async {
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(_testApp());

    final projectsBefore = tester.getSize(find.byType(ProjectSidebar)).width;
    final inspectorBefore = tester
        .getSize(find.byType(ProjectToolsPanel))
        .width;

    await tester.drag(
      find.byKey(const Key('projects-resize-handle')),
      const Offset(60, 0),
    );
    await tester.pump();
    expect(
      tester.getSize(find.byType(ProjectSidebar)).width,
      greaterThan(projectsBefore),
    );

    await tester.drag(
      find.byKey(const Key('inspector-resize-handle')),
      const Offset(-60, 0),
    );
    await tester.pump();
    expect(
      tester.getSize(find.byType(ProjectToolsPanel)).width,
      greaterThan(inspectorBefore),
    );
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets('project reveal button does not select the project row', (
    tester,
  ) async {
    var selected = false;
    var revealed = false;
    const path = '/tmp/the-ditch-project';

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'The Ditch',
            path: path,
            selected: false,
            onTap: () => selected = true,
            onReveal: () => revealed = true,
            onCopyPath: () {},
            onDelete: () {},
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(const ValueKey('reveal-project-$path')));
    await tester.pump();

    expect(revealed, isTrue);
    expect(selected, isFalse);
  });

  testWidgets('project tile renders agent counts and an unread result dot', (
    tester,
  ) async {
    const path = '/tmp/project-summary';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'Summary Project',
            path: path,
            selected: false,
            runningCount: 2,
            stoppedCount: 3,
            hasUnreadResult: true,
            onTap: () {},
            onReveal: () {},
            onCopyPath: () {},
            onDelete: () {},
          ),
        ),
      ),
    );

    expect(find.text('2 running · 3 stopped'), findsOneWidget);
    expect(find.byKey(const ValueKey('project-unread-$path')), findsOneWidget);
  });

  testWidgets('agent terminal states render as colored status chips', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              AgentStatusChip(status: AgentStatus.working),
              AgentStatusChip(status: AgentStatus.stopping),
              AgentStatusChip(status: AgentStatus.completed),
              AgentStatusChip(status: AgentStatus.failed),
            ],
          ),
        ),
      ),
    );

    expect(find.text('Running'), findsOneWidget);
    expect(find.text('Stopping'), findsOneWidget);
    expect(find.text('Completed'), findsOneWidget);
    expect(find.text('Failed'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('agent-status-completed')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('agent-status-failed')), findsOneWidget);
  });

  testWidgets('stopping agent cannot be reprompted until shutdown finishes', (
    tester,
  ) async {
    final session = AgentSession(
      localId: 'stopping-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.stopping,
      messages: const [],
      codexThreadId: 'thread-stopping',
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ExpandableAgentPanel(
            headerKey: const ValueKey('stopping-agent-header'),
            session: session,
            expanded: true,
            enlarged: false,
            chatViewport: ConversationViewportController(),
            composerKey: GlobalKey<AgentComposerState>(),
            initialPrompt: '',
            onTap: () {},
            onEnlarge: () {},
            onDelete: () {},
            onRename: (_) {},
            onSubmitPrompt: (_) {},
            onStopCodex: () {},
          ),
        ),
      ),
    );

    expect(find.textContaining('waiting for the thread'), findsOneWidget);
    expect(
      tester
          .widget<NativeComposerTextView>(find.byType(NativeComposerTextView))
          .enabled,
      isFalse,
    );
    expect(find.byKey(const ValueKey('agent-status-stopping')), findsOneWidget);
  });

  testWidgets(
    'unread result dot identifies the exact agent and clears on open',
    (tester) async {
      final sessions = [
        AgentSession(
          localId: 'new-result',
          provider: AgentProvider.codex,
          status: AgentStatus.completed,
          messages: const [],
        ),
        AgentSession(
          localId: 'old-result',
          provider: AgentProvider.codex,
          status: AgentStatus.completed,
          messages: const [],
        ),
      ];
      final unreadAgents = {'new-result'};

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: StatefulBuilder(
              builder: (context, setState) => AgentsSurface(
                sessions: sessions,
                expandedAgentLocalId: null,
                focusedAgentLocalId: null,
                chatViewport: ConversationViewportController(),
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                hasUnreadResult: (session) =>
                    unreadAgents.contains(session.localId),
                onFocusAgent: (_) {},
                onToggleExpanded: (session) {
                  setState(() => unreadAgents.remove(session.localId));
                },
              ),
            ),
          ),
        ),
      );

      expect(
        find.byKey(const ValueKey('agent-unread-new-result')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('agent-unread-old-result')),
        findsNothing,
      );

      await tester.tap(find.byKey(_testAgentHeaderKey('new-result')));
      await tester.pump();
      expect(
        find.byKey(const ValueKey('agent-unread-new-result')),
        findsNothing,
      );
    },
  );

  testWidgets('collapsing an expanded agent preserves the element tree', (
    tester,
  ) async {
    final sessions = [
      AgentSession(
        localId: 'collapse-regression-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
      AgentSession(
        localId: 'collapse-regression-b',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
    ];
    String? expandedAgentId = sessions.first.localId;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) => AgentsSurface(
              sessions: sessions,
              expandedAgentLocalId: expandedAgentId,
              focusedAgentLocalId: null,
              chatViewport: ConversationViewportController(),
              agentListController: ScrollController(),
              composerKey: GlobalKey<AgentComposerState>(),
              headerKeyForAgent: _testAgentHeaderKey,
              initialPrompt: '',
              onStartCodex: () {},
              onSubmitPrompt: (_, _) {},
              onStopCodex: (_) {},
              onDeleteAgent: (_) {},
              onRenameAgent: (_, _) {},
              hasUnreadResult: (_) => true,
              onFocusAgent: (_) {},
              onToggleExpanded: (_) {
                setState(() => expandedAgentId = null);
              },
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(_testAgentHeaderKey('collapse-regression-a')));
    await tester.pump();

    expect(tester.takeException(), isNull);
    expect(find.byKey(const ValueKey('collapse-regression-a')), findsOneWidget);
    expect(find.byKey(const ValueKey('collapse-regression-b')), findsOneWidget);
  });

  testWidgets(
    'switching projects shows the new project agent list with none open',
    (tester) async {
      final projectA = AgentSession(
        localId: 'project-switch-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      );
      final projectB = AgentSession(
        localId: 'project-switch-b',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      );
      var sessions = [projectA];
      String? expandedAgentId = projectA.localId;
      String? focusedAgentId = projectA.localId;
      final composerKeys = <String, GlobalKey<AgentComposerState>>{};

      await tester.pumpWidget(
        MaterialApp(
          home: StatefulBuilder(
            builder: (context, setState) => Scaffold(
              appBar: AppBar(
                actions: [
                  TextButton(
                    key: const Key('switch-project-regression'),
                    onPressed: () {
                      setState(() {
                        sessions = [projectB];
                        expandedAgentId = null;
                        focusedAgentId = null;
                      });
                    },
                    child: const Text('Switch'),
                  ),
                ],
              ),
              body: AgentsSurface(
                sessions: sessions,
                expandedAgentLocalId: expandedAgentId,
                focusedAgentLocalId: focusedAgentId,
                chatViewport: ConversationViewportController(),
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                composerKeyForAgent: (agentId) => composerKeys.putIfAbsent(
                  agentId,
                  GlobalKey<AgentComposerState>.new,
                ),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                onFocusAgent: (_) {},
                onToggleExpanded: (_) {},
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.byKey(const Key('switch-project-regression')));
      await tester.pump();

      expect(tester.takeException(), isNull);
      expect(find.byKey(const ValueKey('project-switch-a')), findsNothing);
      expect(find.byKey(const ValueKey('project-switch-b')), findsOneWidget);
      expect(find.text('Agents'), findsOneWidget);
      expect(find.byType(AgentChatPanel), findsNothing);
    },
  );

  testWidgets('project right click exposes copy path and delete actions', (
    tester,
  ) async {
    var copied = false;
    var deleted = false;
    const path = '/tmp/context-project';

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'Context Project',
            path: path,
            selected: false,
            onTap: () {},
            onReveal: () {},
            onCopyPath: () => copied = true,
            onDelete: () => deleted = true,
          ),
        ),
      ),
    );

    final tile = find.byKey(const ValueKey('project-tile-$path'));
    await tester.tap(tile, buttons: kSecondaryMouseButton);
    await tester.pumpAndSettle();
    expect(find.text('Copy Project Path'), findsOneWidget);
    expect(find.text('Delete'), findsOneWidget);

    await tester.tap(find.text('Copy Project Path'));
    await tester.pumpAndSettle();
    expect(copied, isTrue);
    expect(deleted, isFalse);

    await tester.tap(tile, buttons: kSecondaryMouseButton);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(deleted, isTrue);
  });

  testWidgets('expanded agent has persistent prompt composer', (tester) async {
    await tester.pumpWidget(_testApp());

    expect(find.byType(AgentComposer), findsOneWidget);
    expect(find.byType(ThinkingStatusStrip), findsOneWidget);
    expect(find.textContaining('Thinking'), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      isEmpty,
    );
    expect(find.text('Start'), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isTrue,
    );
  });

  testWidgets('composer accepts typed replacement text', (tester) async {
    await tester.pumpWidget(_testApp());

    await tester.enterText(find.byType(TextField), 'hello');
    await tester.pumpAndSettle();

    expect(find.text('hello'), findsOneWidget);
  });

  testWidgets('composer starts compact and keeps controls below the editor', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    final editorShell = tester.widget<AnimatedContainer>(
      find.byKey(const Key('composer-editor-shell')),
    );
    expect(editorShell.constraints?.maxHeight, 48);

    final editorTop = tester.getTopLeft(
      find.byKey(const Key('composer-editor-shell')),
    );
    final approvalTop = tester.getTopLeft(find.text('Approve for me'));
    expect(approvalTop.dy, greaterThan(editorTop.dy));
  });

  testWidgets('whole composer surface focuses the editor', (tester) async {
    final outsideFocus = FocusNode(debugLabel: 'outside-composer-focus');
    addTearDown(outsideFocus.dispose);
    String? submitted;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              Focus(
                focusNode: outsideFocus,
                child: const SizedBox(width: 40, height: 40),
              ),
              AgentComposer(
                initialText: '',
                hasSession: true,
                isWorking: false,
                onSubmit: (value) => submitted = value,
                onStop: () {},
              ),
            ],
          ),
        ),
      ),
    );

    outsideFocus.requestFocus();
    await tester.pump();
    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isFalse,
    );

    final surface = tester.getRect(
      find.byKey(const Key('composer-focus-surface')),
    );
    await tester.tapAt(surface.topLeft + const Offset(4, 4));
    await tester.pump();

    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isTrue,
    );

    await tester.enterText(find.byType(TextField), 'still a button');
    await tester.pump();
    await tester.tap(find.text('Send'));
    await tester.pump();
    expect(submitted, 'still a button');
  });

  testWidgets('native composer focus evicts stale Flutter widget focus', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);

    int? platformViewId;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (call) async {
        if (call.method == 'create') {
          final arguments = call.arguments as Map<dynamic, dynamic>;
          platformViewId = arguments['id'] as int;
        }
        return null;
      },
    );
    addTearDown(() {
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform_views,
        null,
      );
    });

    final staleTerminalFocus = FocusNode(debugLabel: 'stale-terminal-focus');
    addTearDown(staleTerminalFocus.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Column(
          children: [
            const SizedBox(
              width: 320,
              height: 80,
              child: AppKitView(
                viewType: 'the_ditch/composer_text_view',
                layoutDirection: TextDirection.ltr,
              ),
            ),
            Focus(
              focusNode: staleTerminalFocus,
              child: const SizedBox(width: 100, height: 40),
            ),
          ],
        ),
      ),
    );
    await tester.pump();

    final platformViewFocus = tester
        .widget<Focus>(
          find.descendant(
            of: find.byType(AppKitView),
            matching: find.byType(Focus),
          ),
        )
        .focusNode!;
    staleTerminalFocus.requestFocus();
    await tester.pump();
    expect(staleTerminalFocus.hasFocus, isTrue);
    expect(platformViewFocus.hasFocus, isFalse);
    expect(platformViewId, isNotNull);

    final message = SystemChannels.platform_views.codec.encodeMethodCall(
      MethodCall('viewFocused', platformViewId),
    );
    tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      SystemChannels.platform_views.name,
      message,
      (_) {},
    );
    await tester.pump();

    expect(staleTerminalFocus.hasFocus, isFalse);
    expect(platformViewFocus.hasFocus, isTrue);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('composer sends with Enter and keeps focus', (tester) async {
    String? submitted;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentComposer(
            initialText: '',
            hasSession: true,
            isWorking: false,
            onSubmit: (value) => submitted = value,
            onStop: () {},
          ),
        ),
      ),
    );

    await tester.enterText(find.byType(TextField), 'hello');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();

    expect(submitted, 'hello');
    expect(find.text('hello'), findsNothing);
    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isTrue,
    );
  });

  testWidgets('composer keeps an editable draft while agent is working', (
    tester,
  ) async {
    String? submitted;

    Widget buildComposer({required bool isWorking}) => MaterialApp(
      home: Scaffold(
        body: AgentComposer(
          initialText: '',
          hasSession: true,
          isWorking: isWorking,
          onSubmit: (value) => submitted = value,
          onStop: () {},
        ),
      ),
    );

    await tester.pumpWidget(buildComposer(isWorking: true));
    expect(tester.widget<TextField>(find.byType(TextField)).enabled, isTrue);

    await tester.enterText(find.byType(TextField), 'draft next turn');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();

    expect(submitted, isNull);
    expect(find.text('draft next turn'), findsOneWidget);

    await tester.pumpWidget(buildComposer(isWorking: false));
    await tester.pump();
    expect(find.text('draft next turn'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(submitted, 'draft next turn');
  });

  testWidgets('agent title supports inline rename', (tester) async {
    String? renamed;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: EditableAgentTitle(
            title: 'Codex session title',
            hasOverride: false,
            onRename: (value) => renamed = value,
          ),
        ),
      ),
    );

    await tester.tap(find.text('Codex session title'));
    await tester.pump();
    await tester.enterText(find.byKey(const Key('agent-title-editor')), 'Plan');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();

    expect(renamed, 'Plan');
  });

  testWidgets('agent title draft survives live agent updates', (tester) async {
    final headerKey = GlobalKey();
    final session = AgentSession(
      localId: 'agent-live-update',
      provider: AgentProvider.codex,
      status: AgentStatus.working,
      messages: [],
      codexTitle: 'Working agent',
    );

    Widget buildPanel() => MaterialApp(
      home: Scaffold(
        body: ExpandableAgentPanel(
          headerKey: headerKey,
          session: session,
          expanded: false,
          enlarged: false,
          chatViewport: null,
          composerKey: null,
          initialPrompt: '',
          onTap: () {},
          onEnlarge: () {},
          onDelete: () {},
          onRename: (_) {},
          onSubmitPrompt: (_) {},
          onStopCodex: () {},
        ),
      ),
    );

    await tester.pumpWidget(buildPanel());
    await tester.tap(find.text('Working agent'));
    await tester.pump();
    await tester.enterText(
      find.byKey(const Key('agent-title-editor')),
      'Partial rename',
    );

    session.lastVisibleAction = 'Received another tool update';
    session.messages.add(
      AgentChatMessage(
        role: ChatMessageRole.tool,
        text: 'Tool finished',
        createdAt: DateTime(2026),
      ),
    );
    await tester.pumpWidget(buildPanel());
    await tester.pump();

    final editor = tester.widget<TextField>(
      find.byKey(const Key('agent-title-editor')),
    );
    expect(editor.controller?.text, 'Partial rename');
    expect(
      tester.widget<EditableText>(find.byType(EditableText)).focusNode.hasFocus,
      isTrue,
    );
  });

  testWidgets('empty state does not expose a meaningless stop action', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    expect(find.widgetWithText(OutlinedButton, 'Stop'), findsNothing);
  });

  testWidgets('conversation uses native chat surface instead of terminal', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(find.textContaining('[39m'), findsNothing);
    expect(find.textContaining('[?2026h'), findsNothing);
  });

  testWidgets('empty project does not create a synthetic agent session', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(find.byType(ExpandableAgentPanel), findsNothing);
    expect(find.byKey(const Key('ready-agent-card')), findsOneWidget);
  });

  testWidgets('expanded agent owns one bounded conversation scrollable', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1200, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final sessions = [
      AgentSession(
        localId: 'agent-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
      AgentSession(
        localId: 'agent-b',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
    ];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: sessions,
            expandedAgentLocalId: 'agent-a',
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: 'Start here',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byType(DropdownButton<String>), findsNothing);
    expect(find.byType(ExpandableAgentPanel), findsOneWidget);
    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(AgentChatPanel),
        matching: find.byType(ListView),
      ),
      findsOneWidget,
    );
    expect(find.byType(CustomScrollView), findsNothing);
    expect(find.byType(ListView), findsOneWidget);
  });

  testWidgets('later expanded agent scrolls to a pinned visible composer', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(900, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final sessions = List.generate(
      3,
      (index) => AgentSession(
        localId: 'agent-$index',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: index == 2
            ? List.generate(
                30,
                (messageIndex) => AgentChatMessage(
                  identity: 'agent-2-message-$messageIndex',
                  role: ChatMessageRole.assistant,
                  text: 'Message $messageIndex',
                  createdAt: DateTime(2026),
                ),
              )
            : const [],
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: sessions,
            expandedAgentLocalId: 'agent-2',
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    await tester.pump();
    final transcript = find.byType(ListView);
    final header = find.byType(InkWell).first;
    final composer = find.byType(AgentComposer);
    final headerTop = tester.getTopLeft(header);
    final composerTop = tester.getTopLeft(composer);
    await tester.drag(transcript, const Offset(0, 300));
    await tester.pumpAndSettle();
    expect(tester.getTopLeft(header), headerTop);
    expect(tester.getTopLeft(composer), composerTop);
  });

  testWidgets('chat messages alternate clearly between left and right', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              AgentChatBubble(
                message: AgentChatMessage(
                  role: ChatMessageRole.user,
                  text: 'User message',
                  createdAt: DateTime(2026),
                ),
              ),
              AgentChatBubble(
                message: AgentChatMessage(
                  role: ChatMessageRole.assistant,
                  text: 'Assistant message',
                  createdAt: DateTime(2026),
                ),
              ),
            ],
          ),
        ),
      ),
    );

    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Align && widget.alignment == Alignment.centerRight,
      ),
      findsOneWidget,
    );
    expect(
      find.byWidgetPredicate(
        (widget) => widget is Align && widget.alignment == Alignment.centerLeft,
      ),
      findsOneWidget,
    );
  });
}
