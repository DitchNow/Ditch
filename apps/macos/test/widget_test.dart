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

  test('classifies stderr chunks as visible diagnostics', () {
    final diagnostic = codexStderrDiagnosticFromChunk(
      'ERROR codex_models_manager::cache: failed to load models cache',
    );

    expect(diagnostic, isNotNull);
    expect(diagnostic!.kind, CodexProcessEventKind.diagnostic);
    expect(diagnostic.isVisibleInChat, isTrue);
    expect(diagnostic.text, contains('models cache'));
  });

  test('ignores empty stderr chunks', () {
    expect(codexStderrDiagnosticFromChunk('   \n'), isNull);
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

  testWidgets('renders command center shell', (tester) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.text('The Ditch'), findsWidgets);
    expect(find.text('PROJECTS'), findsOneWidget);
    expect(find.text('Agents'), findsOneWidget);
    expect(find.text('Attention'), findsOneWidget);
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

  testWidgets('attention starts empty instead of showing activity feed noise', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.text('No agent sessions need attention.'), findsOneWidget);
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
            chatController: ScrollController(),
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
            chatController: ScrollController(),
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

  testWidgets('attention cards expose session actions', (tester) async {
    var opened = false;
    var dismissed = false;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AttentionPanel(
            width: 320,
            events: [
              AttentionEvent(
                id: 'attention-test',
                kind: AttentionKind.failed,
                icon: Icons.error_outline,
                title: 'Codex failed',
                body: 'Exit code: 1.',
                sessionLocalId: 'agent-0',
                createdAt: DateTime(2026),
              ),
            ],
            canStopSession: (_) => false,
            onOpenSession: (_) => opened = true,
            onStopSession: (_) {},
            onDismiss: (_) => dismissed = true,
          ),
        ),
      ),
    );

    expect(find.text('Codex failed'), findsOneWidget);
    expect(find.text('Open'), findsOneWidget);
    expect(find.text('Dismiss'), findsOneWidget);

    await tester.tap(find.text('Open'));
    await tester.pump();
    expect(opened, isTrue);

    await tester.tap(find.text('Dismiss'));
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
            chatController: ScrollController(),
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
            chatController: ScrollController(),
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
            chatController: chatController,
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
    expect(
      chatController.offset,
      moreOrLessEquals(chatController.position.maxScrollExtent),
    );
    chatController.jumpTo(0);
    await tester.pump();
    final header = find.byType(InkWell).first;
    final composer = find.byType(AgentComposer);
    final headerTop = tester.getTopLeft(header);
    final composerTop = tester.getTopLeft(composer);
    await tester.drag(find.byType(ListView).last, const Offset(0, -300));
    await tester.pumpAndSettle();
    expect(chatController.offset, greaterThan(0));
    expect(agentListController.offset, 0);
    expect(tester.getTopLeft(header), headerTop);
    expect(tester.getTopLeft(composer), composerTop);
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
      find.descendant(
        of: dialog,
        matching: find.text(
          'Inspect this project and tell me the next useful engineering step.',
        ),
      ),
      findsOneWidget,
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

  testWidgets('new agent toolbar action remains enabled while agents work', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: DitchToolbar(
          projectName: 'Fixture',
          connection: RuntimeConnectionPhase.connected,
          attentionCount: 0,
          sidebarVisible: true,
          inspectorVisible: true,
          onToggleSidebar: () {},
          onToggleInspector: () {},
          onNewAgent: () {},
        ),
      ),
    );

    final startButton = tester.widget<ButtonStyleButton>(
      find.ancestor(
        of: find.text('New Agent'),
        matching: find.byWidgetPredicate(
          (widget) => widget is ButtonStyleButton,
        ),
      ),
    );

    expect(startButton.onPressed, isNotNull);
  });

  testWidgets('expanded agent has persistent prompt composer', (tester) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.byType(AgentComposer), findsOneWidget);
    expect(find.byType(ThinkingStatusStrip), findsOneWidget);
    expect(find.textContaining('Thinking'), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    expect(
      find.text(
        'Inspect this project and tell me the next useful engineering step.',
      ),
      findsOneWidget,
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

  testWidgets('expanded agent stays in list with a bounded conversation', (
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
            chatController: ScrollController(),
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
    // The second row remains in the lazily built outer list below the
    // viewport-sized expanded row.
    expect(find.byType(ExpandableAgentPanel), findsOneWidget);
    expect(find.byType(AgentChatPanel), findsOneWidget);
    final embeddedChat = tester.widget<AgentChatPanel>(
      find.byType(AgentChatPanel),
    );
    expect(embeddedChat.embedded, isFalse);
    expect(
      find.descendant(
        of: find.byType(AgentChatPanel),
        matching: find.byType(ListView),
      ),
      findsOneWidget,
    );
    expect(find.byType(ListView), findsNWidgets(2));
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
                chatController: ScrollController(),
                agentListController: ScrollController(),
              ),
              AgentChatBubble(
                message: AgentChatMessage(
                  role: ChatMessageRole.assistant,
                  text: 'Assistant message',
                  createdAt: DateTime(2026),
                ),
                chatController: ScrollController(),
                agentListController: ScrollController(),
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
