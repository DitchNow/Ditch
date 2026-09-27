import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

class RecordingClient extends DitchRuntimeClient {
  final requests = <Object>[];
  @override
  Future<Map<String, dynamic>> request(Object body) async {
    requests.add(body);
    return {'Accepted': true};
  }
}

void main() {
  test(
    'each session restores its own profile and preserves next-turn edits',
    () {
      final first = AgentExecutionSettings(
        profile: {'model': 'remote-model', 'approval': 'Ask'},
      );
      final second = AgentExecutionSettings(
        profile: {'model': 'local-model', 'approval': 'ApproveForMe'},
      );
      first.setModel('next-model');
      first.setApproval(AgentApprovalPreset.fullAccess);
      first.syncProfile({'model': 'remote-model', 'approval': 'Ask'});
      expect(first.protocolValue['model'], 'next-model');
      expect(first.protocolValue['approval'], 'FullAccess');
      expect(second.protocolValue['model'], 'local-model');
      expect(second.protocolValue['approval'], 'ApproveForMe');
    },
  );

  test(
    'start, resume and follow-up encode the chosen session profile',
    () async {
      final client = RecordingClient();
      final settings = AgentExecutionSettings(
        profile: {'model': 'remote-model', 'approval': 'Ask'},
      );
      await client.startCodexSession(
        projectId: 'remote-project',
        projectName: 'Remote',
        projectRoot: '/srv/project',
        prompt: 'start',
        executionProfile: settings.protocolValue,
      );
      settings.setModel('next-model');
      settings.setApproval(AgentApprovalPreset.approveForMe);
      await client.promptAgent(
        agentId: 'agent',
        prompt: 'continue',
        executionProfile: settings.protocolValue,
      );
      await client.resumeCodexSession(
        projectName: 'Remote',
        projectRoot: '/srv/project',
        threadId: 'thread',
        prompt: 'resume',
        executionProfile: settings.protocolValue,
      );
      final start = (client.requests[0] as Map)['StartCodexSession'] as Map;
      final prompt = (client.requests[1] as Map)['PromptAgent'] as Map;
      final resume = (client.requests[2] as Map)['ResumeCodexSession'] as Map;
      expect(start['execution_profile'], containsPair('approval', 'Ask'));
      expect(prompt['execution_profile'], containsPair('model', 'next-model'));
      expect(
        prompt['execution_profile'],
        containsPair('approval', 'ApproveForMe'),
      );
      expect(resume['execution_profile'], prompt['execution_profile']);
    },
  );

  testWidgets(
    'composers load models from the current project scope and allow Ask',
    (tester) async {
      final local = AgentExecutionSettings();
      final remote = AgentExecutionSettings(
        profile: {'model': 'unlisted-saved-model', 'approval': 'Ask'},
      );
      var localLoads = 0;
      var remoteLoads = 0;
      Widget view(
        AgentExecutionSettings settings,
        Future<List<AgentModelOption>> Function() load,
      ) => MaterialApp(
        theme: DitchTheme.light(),
        home: Scaffold(
          body: AgentSettingsScope(
            settings: settings,
            loadModels: load,
            child: AgentComposer(
              initialText: '',
              hasSession: true,
              isWorking: true,
              onSubmit: (_) {},
              onStop: () {},
            ),
          ),
        ),
      );
      await tester.pumpWidget(
        view(local, () async {
          localLoads++;
          return const [
            AgentModelOption(
              id: 'local-model',
              displayName: 'Local model',
              isDefault: true,
            ),
          ];
        }),
      );
      await tester.pump();
      expect(localLoads, 1);
      expect(local.models.single.id, 'local-model');
      // A new scope must not inherit the previous host's model list or selection.
      await tester.pumpWidget(
        view(remote, () async {
          remoteLoads++;
          return const [
            AgentModelOption(
              id: 'remote-model',
              displayName: 'Remote model',
              isDefault: true,
            ),
          ];
        }),
      );
      await tester.pump();
      expect(remoteLoads, 1);
      expect(remote.models.single.id, 'remote-model');
      expect(remote.model, 'unlisted-saved-model');
      final approval = tester.widget<DropdownButton<AgentApprovalPreset>>(
        find.byType(DropdownButton<AgentApprovalPreset>),
      );
      expect(approval.items!.first.enabled, isTrue);
      expect(find.text('Applies next turn'), findsOneWidget);
      expect(tester.takeException(), isNull);
    },
  );

  test('late model discovery cannot overwrite a user selection', () async {
    final settings = AgentExecutionSettings();
    final result = Completer<List<AgentModelOption>>();
    final loading = settings.loadModels(() => result.future);
    settings.setModel('chosen');
    result.complete(const [
      AgentModelOption(id: 'default', displayName: 'Default', isDefault: true),
    ]);
    await loading;
    expect(settings.model, 'chosen');
    expect(settings.loadingModels, isFalse);
  });
}
