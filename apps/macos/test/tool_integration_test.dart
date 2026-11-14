import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/data/runtime_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

class DesktopClient extends DitchRuntimeClient {
  final requests = <Object>[];
  final revoke = Completer<Map<String, dynamic>>();
  @override
  Stream<Map<String, dynamic>> subscribeToolEvents() => const Stream.empty();
  @override
  Future<RuntimeStatusDto> runtimeStatus() async => const RuntimeStatusDto(
    identity: 'test',
    pid: 1,
    socketPath: '/test',
    activeSessionCount: 1,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'test',
    capabilities: {'tool_integrations_v1'},
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
          'state': 'permissionRequired',
          'accessibility': false,
          'screen_recording': false,
          'host_name': 'Ditch Test Runtime',
        },
      },
    };
  }
}

void main() {
  Future<void> controls(
    WidgetTester tester,
    AgentExecutionSettings settings,
    DesktopClient client,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: Scaffold(
          body: AgentExecutionControls(
            settings: settings,
            runtimeClient: client,
            agentId: 'agent',
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

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
        find.text('Computer use · macOS permissions required'),
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
      expect(find.text('Stopping computer use…'), findsOneWidget);
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
    await controls(tester, AgentExecutionSettings(), client);
    expect(find.byType(DesktopIntegrationStatus), findsNothing);
    expect(client.requests, isEmpty);
    await tester.pumpWidget(const SizedBox());
  });
}
