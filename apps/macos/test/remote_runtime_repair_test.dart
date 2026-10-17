import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';

const setup = <String, dynamic>{
  'ready': false,
  'connection_state': 'setup_required',
  'checks': [
    {
      'key': 'runtime',
      'label': 'Ditch Runtime',
      'state': 'install_available',
      'detail': 'Version update required',
    },
  ],
};

class RepairClient extends DitchRuntimeClient {
  int installations = 0;
  int polls = 0;
  bool fail = false;
  int lostResponses = 0;

  @override
  Future<Map<String, dynamic>> installRemoteRuntime(String alias) async {
    installations++;
    return {
      ...setup,
      'connection_state': 'updating',
      'checks': [
        {
          'key': 'runtime',
          'label': 'Ditch Runtime',
          'state': 'checking',
          'detail': 'Waiting for remote work to finish…',
        },
      ],
    };
  }

  @override
  Future<Map<String, dynamic>> remoteRuntimeUpdateStatus(String alias) async {
    polls++;
    if (lostResponses > 0) {
      lostResponses--;
      throw StateError('Runtime connection interrupted');
    }
    if (fail) {
      return {
        ...setup,
        'connection_state': 'update_failed',
        'last_error': 'Runtime checksum mismatch',
      };
    }
    return {
      ...setup,
      'ready': true,
      'connection_state': 'connected',
      'checks': [
        {
          'key': 'runtime',
          'label': 'Ditch Runtime',
          'state': 'ready',
          'detail': 'Runtime updated',
        },
      ],
    };
  }
}

Widget repair(RepairClient client, {Map<String, dynamic> initial = setup}) =>
    MaterialApp(
      home: Scaffold(
        body: AddRemoteProjectDialog(
          client: client,
          initialAlias: 'fixture',
          initialSetup: initial,
          repairOnly: true,
          requireCodexSandbox: false,
        ),
      ),
    );

void main() {
  for (final hadTerminal in [false, true]) {
    testWidgets(
      'closed remote terminal waits for a click (existing: $hadTerminal)',
      (tester) async {
        var automaticStarts = 0;
        var explicitStarts = 0;
        Widget panel(ProjectTerminalSession? terminal) => MaterialApp(
          home: Scaffold(
            body: ProjectToolsPanel(
              width: 300,
              projectId: 'project',
              terminal: terminal,
              files: null,
              showFiles: false,
              onEnsureTerminal: () async {
                automaticStarts++;
              },
              onStartTerminal: terminal == null
                  ? () {
                      explicitStarts++;
                    }
                  : null,
              onEnsureFiles: () async {},
              presentation: TerminalPresentation.docked,
              presentedTool: WorkspaceToolKind.terminal,
              dockedTerminalExpanded: true,
              dockedFilesExpanded: false,
              onToggleDocked: () {},
              onToggleFiles: () {},
              onPresentationChanged: (_) {},
              onToolPresentationChanged: (_, _) {},
              onToggleDirectory: (_) {},
              onOpenFile: (_) {},
              onRevealFile: (_) {},
              onBackToFiles: null,
              onSaveFile: null,
              onReloadFile: null,
              onOverwriteFile: null,
              onRefreshFiles: () {},
            ),
          ),
        );
        if (hadTerminal) {
          await tester.pumpWidget(
            panel(
              ProjectTerminalSession(
                id: 'terminal',
                projectId: 'project',
                shell: '/bin/sh',
              ),
            ),
          );
          await tester.pumpAndSettle();
        }
        await tester.pumpWidget(panel(null));
        await tester.pumpAndSettle();
        expect(automaticStarts, 0);
        expect(explicitStarts, 0);
        expect(find.text('Starting project shell…'), findsNothing);
        await tester.tap(find.text('Open terminal'));
        expect(explicitStarts, 1);
      },
    );
  }

  testWidgets(
    'reopening follows a pending update without submitting another install',
    (tester) async {
      final client = RepairClient();
      await tester.pumpWidget(
        repair(client, initial: {...setup, 'connection_state': 'updating'}),
      );
      await tester.pump(const Duration(seconds: 1));
      await tester.pumpAndSettle();
      expect(client.installations, 0);
      expect(client.polls, 1);
      expect(find.text('Runtime updated'), findsOneWidget);
    },
  );

  testWidgets('a lost progress response retries without reinstalling', (
    tester,
  ) async {
    final client = RepairClient()..lostResponses = 1;
    await tester.pumpWidget(repair(client));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Install / Repair Ditch Runtime'));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.pump(const Duration(seconds: 1));
    await tester.pumpAndSettle();
    expect(client.installations, 1);
    expect(client.polls, 2);
    expect(find.text('Runtime updated'), findsOneWidget);
    expect(find.textContaining('Runtime connection interrupted'), findsNothing);
  });

  testWidgets(
    'one click waits and completes on the same screen without confirmation',
    (tester) async {
      final client = RepairClient();
      await tester.pumpWidget(repair(client));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Install / Repair Ditch Runtime'));
      await tester.pump();
      expect(client.installations, 1);
      expect(find.text('Waiting for remote work to finish…'), findsOneWidget);
      expect(find.text('Upgrade an older remote runtime'), findsNothing);
      expect(find.text('Work finished — repair'), findsNothing);
      await tester.pump(const Duration(seconds: 1));
      await tester.pumpAndSettle();
      expect(client.polls, 1);
      expect(client.installations, 1);
      expect(find.text('Runtime updated'), findsOneWidget);
      expect(find.byType(AlertDialog), findsOneWidget);
    },
  );

  testWidgets('a real failure appears inline without an extra dialog', (
    tester,
  ) async {
    final client = RepairClient()..fail = true;
    await tester.pumpWidget(repair(client));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Install / Repair Ditch Runtime'));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    await tester.pumpAndSettle();
    expect(find.textContaining('Runtime checksum mismatch'), findsOneWidget);
    expect(find.byType(AlertDialog), findsOneWidget);
    expect(client.installations, 1);
  });
}
