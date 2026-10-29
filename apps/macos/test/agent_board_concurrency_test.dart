import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/main.dart';

void main() {
  testWidgets(
    'New Agent and idle conversation remain usable while board conversation runs',
    (tester) async {
      tester.view.physicalSize = const Size(1400, 1000);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final viewport = ConversationViewportController();
      final scroll = ScrollController();
      addTearDown(viewport.dispose);
      addTearDown(scroll.dispose);
      // Board workers appear in the same session list as manual conversations.
      final board = AgentSession(
        localId: 'board-worker',
        projectId: 'project',
        provider: AgentProvider.codex,
        status: AgentStatus.working,
        canStop: true,
        codexThreadId: 'board-thread',
        userTitle: 'Running board task',
        messages: [],
      );
      final idle = AgentSession(
        localId: 'manual',
        projectId: 'project',
        provider: AgentProvider.codex,
        status: AgentStatus.stopped,
        codexThreadId: 'manual-thread',
        userTitle: 'Idle conversation',
        messages: [],
      );
      final keys = <String, GlobalKey<AgentComposerState>>{
        for (final session in [board, idle])
          session.localId: GlobalKey<AgentComposerState>(),
      };
      String? expanded = idle.localId;
      var starts = 0;
      final prompts = <String>[];
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: StatefulBuilder(
              builder: (context, setState) => AgentsSurface(
                sessions: [board, idle],
                expandedAgentLocalId: expanded,
                focusedAgentLocalId: null,
                chatViewport: viewport,
                agentListController: scroll,
                composerKey: keys[idle.localId]!,
                composerKeyForAgent: (id) => keys[id]!,
                headerKeyForAgent: (id) => ValueKey('header-$id'),
                initialPrompt: '',
                onStartCodex: () => starts++,
                onSubmitPrompt: (session, prompt) async {
                  prompts.add('${session.localId}:$prompt');
                  return true;
                },
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                onFocusAgent: (_) {},
                onToggleExpanded: (session) => setState(
                  () => expanded =
                      expanded == session.localId ? null : session.localId,
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('agents-new-agent-button')));
      expect(starts, 1);
      final editor = find.descendant(
        of: find.byType(AgentComposer),
        matching: find.byType(TextField),
      );
      await tester.enterText(editor, 'Continue independent work');
      await keys[idle.localId]!.currentState!.submit();
      await tester.pumpAndSettle();
      expect(prompts, ['manual:Continue independent work']);
      expect(board.isActive, isTrue);

      await tester.tap(find.byKey(const ValueKey('header-manual')));
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const ValueKey('header-board-worker')));
      // The active conversation has a continuously animated progress indicator.
      await tester.pump(const Duration(milliseconds: 300));
      await tester.enterText(editor, 'Do not start a duplicate turn');
      await keys[board.localId]!.currentState!.submit();
      await tester.pump();
      expect(prompts, ['manual:Continue independent work']);
      await tester.tap(find.byKey(const Key('agents-new-agent-button')));
      expect(starts, 2);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pumpAndSettle();
    },
  );
}
