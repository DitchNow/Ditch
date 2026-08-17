import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';

void main() {
  test('runtime parser accepts unit Accepted responses', () {
    final parsed = parseRuntimeResponseLine(
      '{"protocol_version":1,"id":"00000000-0000-4000-8000-000000000000","sent_at":"2026-01-01T00:00:00Z","body":"Accepted"}',
    );

    expect(parsed, {'Accepted': true});
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
    expect(find.text('Projects'), findsOneWidget);
    expect(find.text('Agents'), findsOneWidget);
    expect(find.text('Attention'), findsOneWidget);
    expect(find.text('Start Codex'), findsOneWidget);
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
    await tester.tap(find.byType(DropdownButtonFormField<ProjectGitPolicy>));
    await tester.pumpAndSettle();
    expect(find.text('Initialize Git Repository'), findsOneWidget);
    expect(find.text('Allow Codex Outside Git'), findsOneWidget);
    expect(find.textContaining('--skip-git-repo-check'), findsOneWidget);
  });

  testWidgets('start codex opens an initial prompt dialog', (tester) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    await tester.tap(find.text('Start Codex'));
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

  testWidgets('start codex remains enabled while another agent is working', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);

    final sessions = [
      AgentSession(
        localId: 'agent-1',
        provider: AgentProvider.codex,
        status: AgentStatus.working,
        currentPrompt: 'Existing run',
        messages: [
          AgentChatMessage(
            role: ChatMessageRole.user,
            text: 'Existing run',
            createdAt: DateTime(2026),
          ),
        ],
      ),
    ];

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: sessions,
            expandedAgentLocalId: 'agent-1',
            chatController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            initialPrompt:
                'Inspect this project and tell me the next useful engineering step.',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    final startButton = tester.widget<ButtonStyleButton>(
      find.ancestor(
        of: find.text('Start Codex'),
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
  });

  testWidgets('composer accepts typed replacement text', (tester) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    await tester.enterText(find.byType(TextField), 'hello');
    await tester.pumpAndSettle();

    expect(find.text('hello'), findsOneWidget);
  });

  testWidgets('stop is disabled without active agent', (tester) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    final stopButton = tester.widget<OutlinedButton>(
      find.widgetWithText(OutlinedButton, 'Stop'),
    );

    expect(stopButton.onPressed, isNull);
  });

  testWidgets('conversation uses native chat surface instead of terminal', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(find.textContaining('[39m'), findsNothing);
    expect(find.textContaining('[?2026h'), findsNothing);
  });

  testWidgets('clicking an agent collapses and expands its chat', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));

    expect(find.byType(AgentChatPanel), findsOneWidget);

    await tester.tap(find.text('Codex').first);
    await tester.pumpAndSettle();

    expect(find.byType(AgentChatPanel), findsNothing);

    await tester.tap(find.text('Codex').first);
    await tester.pumpAndSettle();

    expect(find.byType(AgentChatPanel), findsOneWidget);
  });
}
