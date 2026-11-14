import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/application/agent_prompt_drafts.dart';
import 'package:the_ditch/data/runtime_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

class DesktopClient extends DitchRuntimeClient {
  final requests = <Object>[];
  bool ready = false;
  bool visualizerSupported = true;
  bool toolsRegistered = true;
  final revoke = Completer<Map<String, dynamic>>();
  @override
  Stream<Map<String, dynamic>> subscribeToolEvents() => const Stream.empty();
  @override
  Future<RuntimeStatusDto> runtimeStatus() async => RuntimeStatusDto(
    identity: 'test',
    pid: 1,
    socketPath: '/test',
    activeSessionCount: 1,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'test',
    capabilities: {
      'tool_integrations_v1',
      if (visualizerSupported) 'desktop_visualizer_v1',
    },
  );
  @override
  Future<Map<String, dynamic>> request(Object body) async {
    requests.add(body);
    final operation = (body as Map)['ToolIntegration'] as Map;
    if (operation.containsKey('Revoke')) return revoke.future;
    return {
      'ToolIntegration': {
        'Status': {
          'native_available': true,
          'eligible': true,
          'adapter_connected': toolsRegistered,
          'tools_registered': toolsRegistered,
          'state': 'permissionRequired',
          'accessibility': ready,
          'screen_recording': ready,
          'host_name': 'Ditch Test Runtime',
        },
      },
    };
  }
}

class TurnRequestClient extends DitchRuntimeClient {
  Object? lastRequest;
  @override
  Future<Map<String, dynamic>> request(Object body) async {
    lastRequest = body;
    return {'Accepted': true};
  }
}

void main() {
  testWidgets('native availability does not claim the tools are connected', (
    tester,
  ) async {
    final client = DesktopClient()
      ..ready = true
      ..toolsRegistered = false;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: DesktopIntegrationStatus(
            client: client,
            fullAccess: true,
            isRemote: false,
            agentId: 'agent',
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Connecting'), findsOneWidget);
    expect(find.text('Peekaboo'), findsNothing);
    client.toolsRegistered = true;
    await tester.pump(const Duration(seconds: 4));
    await tester.pumpAndSettle();
    expect(find.text('Peekaboo'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });
  Future<void> controls(
    WidgetTester tester,
    AgentExecutionSettings settings,
    DesktopClient client, {
    bool visualizer = false,
    ValueChanged<bool>? onVisualizerChanged,
    bool nextTurn = false,
    bool remote = false,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: Scaffold(
          body: AgentExecutionControls(
            settings: settings,
            runtimeClient: client,
            agentId: 'agent',
            visualizer: visualizer,
            onVisualizerChanged: onVisualizerChanged,
            appliesNextTurn: nextTurn,
            isRemote: remote,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  test(
    'start resume and follow-up send turn options separately from prompts and profiles',
    () async {
      final client = TurnRequestClient();
      const options = {'computer_use_visualizer': true};
      await client.startCodexSession(
        projectName: 'p',
        projectRoot: '/p',
        prompt: 'hello',
        turnOptions: options,
      );
      expect(
        (client.lastRequest as Map)['StartCodexSession']['turn_options'],
        options,
      );
      await client.resumeCodexSession(
        projectName: 'p',
        projectRoot: '/p',
        threadId: 'thread',
        prompt: 'hello',
        turnOptions: options,
      );
      expect(
        (client.lastRequest as Map)['ResumeCodexSession']['turn_options'],
        options,
      );
      await client.promptAgent(
        agentId: 'a',
        prompt: 'hello',
        turnOptions: options,
      );
      final payload = (client.lastRequest as Map)['PromptAgent'] as Map;
      expect(payload['turn_options'], options);
      expect(payload['prompt'], 'hello');
      expect(
        (payload['execution_profile'] as Map).containsKey(
          'computer_use_visualizer',
        ),
        isFalse,
      );
      await client.promptAgent(agentId: 'a', prompt: 'hello');
      expect(
        ((client.lastRequest as Map)['PromptAgent'] as Map).containsKey(
          'turn_options',
        ),
        isFalse,
      );
    },
  );

  test('visualizer choice is isolated and frozen for a submission', () {
    final drafts = AgentPromptDrafts();
    final first = drafts.forAgent('a', 'hello');
    first.visualizer = true;
    expect(first.beginSubmission(), isTrue);
    first.visualizer = false;
    expect(first.submittedTurnOptions['computer_use_visualizer'], isTrue);
    expect(drafts.forAgent('b', '').visualizer, isFalse);
    first.finishSubmission(
      accepted: false,
      submittedRevision: first.revision,
      submittedAttachments: [],
    );
    expect(first.text, 'hello');
    expect(first.visualizer, isFalse);
    first.beginSubmission();
    expect(first.submittedTurnOptions['computer_use_visualizer'], isFalse);
    first.finishSubmission(
      accepted: true,
      submittedRevision: first.revision,
      submittedAttachments: [],
    );
    expect(first.text, '');
  });

  testWidgets(
    'OSS visualizer is available only with local Full Access and readiness',
    (tester) async {
      final client = DesktopClient()..ready = true;
      final settings = AgentExecutionSettings(
        profile: {
          'approval': 'FullAccess',
          'model': 'qwen',
          'inference_endpoint_id': 'endpoint',
        },
      );
      bool? changed;
      await controls(
        tester,
        settings,
        client,
        onVisualizerChanged: (v) => changed = v,
      );
      final toggle = find.byKey(const Key('computer-use-visualizer'));
      expect(toggle, findsOneWidget);
      expect(
        tester
            .widget<Semantics>(
              find
                  .ancestor(
                    of: toggle,
                    matching: find.byWidgetPredicate(
                      (widget) =>
                          widget is Semantics &&
                          widget.properties.label == 'Computer-use visualizer',
                    ),
                  )
                  .first,
            )
            .properties
            .toggled,
        isFalse,
      );
      await tester.tap(toggle);
      expect(changed, isTrue);
      await controls(
        tester,
        settings,
        client,
        visualizer: true,
        onVisualizerChanged: (v) => changed = v,
        nextTurn: true,
      );
      expect(
        find.byTooltip(
          'Show computer-use targets and actions on the next turn',
        ),
        findsOneWidget,
      );
      await controls(
        tester,
        settings,
        client,
        visualizer: true,
        onVisualizerChanged: (v) => changed = v,
        remote: true,
      );
      expect(tester.widget<TextButton>(toggle).onPressed, isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('new-session dialog exposes the same OSS visualizer choice', (
    tester,
  ) async {
    final client = DesktopClient()..ready = true;
    final draft = AgentPromptDraft('');
    final settings = AgentExecutionSettings(
      profile: {
        'approval': 'FullAccess',
        'model': 'qwen',
        'inference_endpoint_id': 'endpoint',
      },
    );
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: Scaffold(
          body: StartCodexSessionDialog(
            initialPrompt: 'Test computer use',
            runtimeClient: client,
            settings: settings,
            promptDraft: draft,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final toggle = find.byKey(const Key('computer-use-visualizer'));
    expect(toggle, findsOneWidget);
    await tester.tap(toggle);
    await tester.pumpAndSettle();
    expect(draft.visualizer, isTrue);
    expect(
      tester
          .widget<Semantics>(
            find
                .ancestor(
                  of: toggle,
                  matching: find.byWidgetPredicate(
                    (widget) =>
                        widget is Semantics &&
                        widget.properties.label == 'Computer-use visualizer',
                  ),
                )
                .first,
          )
          .properties
          .toggled,
      isTrue,
    );
    await tester.pumpWidget(const SizedBox());
    draft.dispose();
  });

  testWidgets('missing OS permissions and older runtimes disable the switch', (
    tester,
  ) async {
    final client = DesktopClient();
    final settings = AgentExecutionSettings(
      profile: {
        'approval': 'FullAccess',
        'model': 'qwen',
        'inference_endpoint_id': 'endpoint',
      },
    );
    await controls(tester, settings, client, onVisualizerChanged: (_) {});
    final toggle = find.byKey(const Key('computer-use-visualizer'));
    expect(tester.widget<TextButton>(toggle).onPressed, isNull);
    await tester.pumpWidget(const SizedBox());
    client.ready = true;
    client.visualizerSupported = false;
    await controls(tester, settings, client, onVisualizerChanged: (_) {});
    expect(tester.widget<TextButton>(toggle).onPressed, isNull);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'downgrade waits for runtime revocation and polling never prompts macOS',
    (tester) async {
      final client = DesktopClient();
      final settings = AgentExecutionSettings(
        profile: {
          'approval': 'FullAccess',
          'model': 'qwen',
          'inference_endpoint_id': 'endpoint',
        },
      );
      await controls(tester, settings, client);
      expect(
        find.byTooltip('Computer use · macOS permissions required'),
        findsOneWidget,
      );
      expect(
        client.requests.toString(),
        isNot(contains('RequestSystemPermission')),
      );
      await tester.tap(find.byType(DropdownButton<AgentApprovalPreset>));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Ask for approval').last);
      await tester.pumpAndSettle();
      expect(settings.approval, AgentApprovalPreset.fullAccess);
      expect(find.byTooltip('Stopping computer use…'), findsOneWidget);
      client.revoke.complete({'ToolIntegration': 'Revoked'});
      await tester.pumpAndSettle();
      expect(settings.approval, AgentApprovalPreset.ask);
      await tester.pumpWidget(const SizedBox());
    },
  );

  testWidgets('failed revocation never claims that access was removed', (
    tester,
  ) async {
    final client = DesktopClient();
    final settings = AgentExecutionSettings(
      profile: {
        'approval': 'FullAccess',
        'model': 'qwen',
        'inference_endpoint_id': 'endpoint',
      },
    );
    await controls(tester, settings, client);
    await tester.tap(find.byType(DropdownButton<AgentApprovalPreset>));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Ask for approval').last);
    await tester.pump();
    client.revoke.completeError(
      const DitchRuntimeException('desktop_still_stopping', 'Still stopping'),
    );
    await tester.pumpAndSettle();
    expect(settings.approval, AgentApprovalPreset.fullAccess);
    expect(
      find.textContaining('Could not finish stopping computer use'),
      findsOneWidget,
    );
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets('OpenAI controls do not call or show desktop integration', (
    tester,
  ) async {
    final client = DesktopClient();
    await controls(
      tester,
      AgentExecutionSettings(),
      client,
      visualizer: true,
      onVisualizerChanged: (_) {},
    );
    expect(find.byType(DesktopIntegrationStatus), findsNothing);
    expect(find.byKey(const Key('computer-use-visualizer')), findsNothing);
    expect(client.requests, isEmpty);
    await tester.pumpWidget(const SizedBox());
  });
}
