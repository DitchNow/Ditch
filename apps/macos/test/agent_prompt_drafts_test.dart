import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/agent_prompt_drafts.dart';
import 'package:the_ditch/data/prompt_attachment.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/main.dart';

class DraftHarness {
  final drafts = AgentPromptDrafts();
  final viewport = ConversationViewportController();
  final composerKey = GlobalKey<AgentComposerState>();
  final submitted = <String>[];
  String project = 'project-a', agent = 'agent-a';
  bool visible = true, enlarged = false, working = false;
  late StateSetter update;

  Future<void> mount(WidgetTester tester) async {
    addTearDown(viewport.dispose);
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: StatefulBuilder(
          builder: (context, setState) {
            update = setState;
            return Scaffold(
              body: AgentPromptDraftScope(
                drafts: drafts,
                projectKey: project,
                child: visible
                    ? AgentChatPanel(
                        key: ValueKey('$agent-$enlarged'),
                        conversationId: agent,
                        messages: const [],
                        viewport: viewport,
                        enlarged: enlarged,
                        composerKey: composerKey,
                        initialPrompt: '',
                        hasSession: agent != 'new-agent',
                        isWorking: working,
                        onSubmitPrompt: submitted.add,
                        onStopCodex: () {},
                      )
                    : const SizedBox(),
              ),
            );
          },
        ),
      ),
    );
    await settle(tester);
  }

  Future<void> settle(WidgetTester tester) async {
    if (working) {
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
    } else {
      await tester.pumpAndSettle();
    }
  }

  String text(WidgetTester tester) =>
      tester.widget<TextField>(find.byType(TextField)).controller!.text;

  Future<void> recreate(WidgetTester tester) async {
    update(() => visible = false);
    await tester.pumpAndSettle();
    expect(find.byType(AgentComposer), findsNothing);
    update(() => visible = true);
    await settle(tester);
  }
}

void main() {
  testWidgets(
    'attachments restore per agent and project through enlargement and removal',
    (tester) async {
      final f = DraftHarness();
      f.drafts
          .forAgent('agent-a', '')
          .attachments
          .add(PromptAttachment('/tmp/first.md'));
      await f.mount(tester);
      expect(find.text('first.md'), findsOneWidget);
      f.update(() => f.agent = 'agent-b');
      await f.settle(tester);
      expect(find.text('first.md'), findsNothing);
      f.update(() => f.agent = 'agent-a');
      await f.settle(tester);
      f.update(() => f.enlarged = true);
      await f.settle(tester);
      await f.recreate(tester);
      expect(find.text('first.md'), findsOneWidget);
      await tester.tap(find.byTooltip('Remove first.md'));
      await f.recreate(tester);
      expect(find.text('first.md'), findsNothing);
      f.drafts
          .forNewAgent('project-a', '')
          .attachments
          .add(PromptAttachment('/tmp/new.md'));
      f.update(() => f.agent = 'new-agent');
      await f.settle(tester);
      expect(find.text('new.md'), findsOneWidget);
      f.update(() => f.project = 'project-b');
      await f.settle(tester);
      expect(find.text('new.md'), findsNothing);
      f.update(() => f.project = 'project-a');
      await f.settle(tester);
      expect(find.text('new.md'), findsOneWidget);
    },
  );

  testWidgets('macOS editor changes restore into a recreated native view', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final messenger = tester.binding.defaultBinaryMessenger;
    final createdTexts = <String>[];
    final channels = <MethodChannel>[];
    String nativeText = '';
    messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
      call,
    ) async {
      if (call.method == 'create') {
        final args = call.arguments as Map;
        final params =
            const StandardMessageCodec().decodeMessage(
                  ByteData.sublistView(args['params'] as Uint8List),
                )
                as Map;
        nativeText = params['text'] as String;
        createdTexts.add(nativeText);
        final channel = MethodChannel(
          'the_ditch/composer_text_view/${args['id']}',
        );
        channels.add(channel);
        messenger.setMockMethodCallHandler(channel, (call) async {
          if (call.method == 'getText') return nativeText;
          if (call.method == 'clearText') nativeText = '';
          if (call.method == 'setText') nativeText = call.arguments as String;
          if (call.method == 'focus') return true;
          return null;
        });
      }
      return null;
    });
    addTearDown(() {
      messenger.setMockMethodCallHandler(SystemChannels.platform_views, null);
      for (final channel in channels) {
        messenger.setMockMethodCallHandler(channel, null);
      }
    });
    final f = DraftHarness();
    await f.mount(tester);
    nativeText = 'Native draft\nwith more detail';
    final received = Completer<void>();
    messenger.handlePlatformMessage(
      channels.last.name,
      const StandardMethodCodec().encodeMethodCall(
        MethodCall('textChanged', nativeText),
      ),
      (_) => received.complete(),
    );
    await received.future;
    await tester.pump();
    expect(f.drafts.forAgent('agent-a', '').text, nativeText);
    await f.recreate(tester);
    expect(createdTexts.last, 'Native draft\nwith more detail');
    await tester.tap(find.text('Send'));
    await tester.pumpAndSettle();
    expect(f.submitted, ['Native draft\nwith more detail']);
    await f.recreate(tester);
    expect(createdTexts.last, isEmpty);
    await tester.pumpWidget(const SizedBox());
    await tester.pumpAndSettle();
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('drafts survive project switching, unmounting, and enlargement', (
    tester,
  ) async {
    final f = DraftHarness();
    await f.mount(tester);
    await tester.enterText(
      find.byType(TextField),
      'First line\n  Unfinished prompt 📝',
    );
    f.update(() {
      f.project = 'project-b';
      f.agent = 'agent-b';
    });
    await tester.pumpAndSettle();
    expect(f.text(tester), isEmpty);
    await tester.enterText(find.byType(TextField), 'Another agent draft');
    f.update(() {
      f.project = 'project-a';
      f.agent = 'agent-a';
    });
    await tester.pumpAndSettle();
    expect(f.text(tester), 'First line\n  Unfinished prompt 📝');
    await f.recreate(tester);
    expect(f.text(tester), 'First line\n  Unfinished prompt 📝');
    f.update(() => f.enlarged = true);
    await tester.pumpAndSettle();
    expect(f.text(tester), 'First line\n  Unfinished prompt 📝');
    expect(f.drafts.forAgent('agent-b', '').text, 'Another agent draft');
    expect(f.submitted, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('sending and erasing clear only that agent draft', (
    tester,
  ) async {
    final f = DraftHarness();
    await f.mount(tester);
    f.drafts.forAgent('agent-b', '').text = 'Keep me';
    await tester.enterText(find.byType(TextField), 'Send this');
    await tester.pump();
    await tester.tap(find.text('Send'));
    await tester.pumpAndSettle();
    expect(f.submitted, ['Send this']);
    await f.recreate(tester);
    expect(f.text(tester), isEmpty);
    await tester.enterText(find.byType(TextField), 'Delete this');
    await tester.enterText(find.byType(TextField), '');
    await f.recreate(tester);
    expect(f.text(tester), isEmpty);
    expect(f.drafts.forAgent('agent-b', '').text, 'Keep me');
  });

  testWidgets(
    'working agent retains next prompt and Enter clears it when sent',
    (tester) async {
      final f = DraftHarness()..working = true;
      await f.mount(tester);
      await tester.enterText(find.byType(TextField), 'Next turn');
      await f.recreate(tester);
      expect(f.text(tester), 'Next turn');
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(f.submitted, isEmpty);
      f.update(() => f.working = false);
      await tester.pumpAndSettle();
      await tester.tap(find.byType(TextField));
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(f.submitted, ['Next turn']);
      await f.recreate(tester);
      expect(f.text(tester), isEmpty);
    },
  );

  testWidgets(
    'new agent drafts are scoped by project even with a reused composer',
    (tester) async {
      final f = DraftHarness()..agent = 'new-agent';
      await f.mount(tester);
      await tester.enterText(find.byType(TextField), 'Project A starter');
      f.update(() => f.project = 'project-b');
      await tester.pumpAndSettle();
      expect(f.text(tester), isEmpty);
      await tester.enterText(find.byType(TextField), 'Project B starter');
      f.update(() => f.project = 'project-a');
      await tester.pumpAndSettle();
      expect(f.text(tester), 'Project A starter');
      expect(tester.takeException(), isNull);
    },
  );

  test(
    'deleted agents and projects release drafts; a fresh session is empty',
    () {
      final drafts = AgentPromptDrafts();
      drafts.forAgent('a', '').text = 'A';
      drafts.forAgent('b', '').text = 'B';
      drafts.forNewAgent('project', '').text = 'Starter';
      drafts.retainAgents({'a'});
      expect(drafts.forAgent('b', '').text, isEmpty);
      expect(drafts.forAgent('a', '').text, 'A');
      drafts.removeAgent('a');
      drafts.removeProject('project');
      expect(drafts.forAgent('a', '').text, isEmpty);
      expect(drafts.forNewAgent('project', '').text, isEmpty);
      drafts.forAgent('a', '').text = 'Only in this session';
      expect(AgentPromptDrafts().forAgent('a', '').text, isEmpty);
    },
  );
}
