import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/application/command_center_controller.dart';
import 'package:the_ditch/data/runtime_models.dart';

void main() {
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
        'instance_id': 'instance-1',
        'codex_home': '/tmp/codex',
        'build_version': '1.0.0',
        'capabilities': ['persistent_sessions_v1'],
      },
    });

    expect(status.pid, 42);
    expect(status.activeSessionCount, 2);
    expect(status.supportsPersistentSessions, isTrue);
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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.text('The Ditch'), findsWidgets);
    expect(find.text('PROJECTS'), findsOneWidget);
    expect(find.text('Agents'), findsOneWidget);
    expect(find.text('Attention'), findsNothing);
    expect(find.byKey(const Key('notification-bell')), findsOneWidget);
    expect(find.text('New Agent'), findsOneWidget);
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

    expect(find.text('The Ditch Runtime is not responding'), findsOneWidget);
    expect(find.text('Retry Connection'), findsOneWidget);
    expect(find.text('Open Activity Monitor'), findsOneWidget);
    expect(find.text('Quit UI'), findsOneWidget);
    expect(find.text('/tmp/ditchd.sock'), findsOneWidget);
  });

  testWidgets('notification center starts empty without activity feed noise', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();

    expect(find.widgetWithText(AlertDialog, 'Add Project'), findsOneWidget);
    expect(find.text('Browse Folder…'), findsOneWidget);
    expect(find.text('Project name'), findsOneWidget);
    expect(find.text('Selected folder'), findsOneWidget);
    expect(find.text('Add & Configure'), findsOneWidget);
    expect(find.textContaining('.ditch/hooks'), findsOneWidget);
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

    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));
    await tester.tap(find.text('Add Project'));
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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(const ValueKey('reveal-project-$path')));
    await tester.pump();

    expect(revealed, isTrue);
    expect(selected, isFalse);
  });

  testWidgets('expanded agent has persistent prompt composer', (tester) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    await tester.enterText(find.byType(TextField), 'hello');
    await tester.pumpAndSettle();

    expect(find.text('hello'), findsOneWidget);
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

  testWidgets('empty state does not expose a meaningless stop action', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.widgetWithText(OutlinedButton, 'Stop'), findsNothing);
  });

  testWidgets('conversation uses native chat surface instead of terminal', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(find.textContaining('[39m'), findsNothing);
    expect(find.textContaining('[?2026h'), findsNothing);
  });

  testWidgets('empty project does not create a synthetic agent session', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

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
