import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/data/runtime_transport.dart';

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
  final confirmations = <bool>[];
  bool busy = false;

  @override
  Future<Map<String, dynamic>> installRemoteRuntime(
    String alias, {
    bool legacyMaintenanceConfirmed = false,
  }) async {
    confirmations.add(legacyMaintenanceConfirmed);
    if (busy) {
      throw Exception('Repair deferred: remote agents are still active.');
    }
    if (!legacyMaintenanceConfirmed) {
      throw Exception(
        'remote_runtime_install_failed: legacy_maintenance_required: older runtime',
      );
    }
    return setup;
  }
}

Widget repair(RepairClient client) => MaterialApp(
  home: Scaffold(
    body: AddRemoteProjectDialog(
      client: client,
      initialAlias: 'fixture',
      initialSetup: setup,
      repairOnly: true,
    ),
  ),
);

void main() {
  testWidgets(
    'legacy repair retries only after explicit maintenance confirmation',
    (tester) async {
      final client = RepairClient();
      await tester.pumpWidget(repair(client));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Install / Repair Ditch Runtime'));
      await tester.pumpAndSettle();
      expect(client.confirmations, [false]);
      expect(find.text('Upgrade an older remote runtime'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(client.confirmations, [false]);
      await tester.tap(find.text('Install / Repair Ditch Runtime'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Work finished — repair'));
      await tester.pumpAndSettle();
      expect(client.confirmations, [false, false, true]);
    },
  );

  testWidgets('busy repair reports the error without offering an override', (
    tester,
  ) async {
    final client = RepairClient()..busy = true;
    await tester.pumpWidget(repair(client));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Install / Repair Ditch Runtime'));
    await tester.pumpAndSettle();
    expect(client.confirmations, [false]);
    expect(
      find.textContaining('remote agents are still active'),
      findsOneWidget,
    );
    expect(find.text('Work finished — repair'), findsNothing);
  });

  test('repair timeout includes upload, activation and recovery', () {
    expect(
      RuntimeTransport.responseTimeout({
        'InstallRemoteRuntime': {'alias': 'fixture'},
      }),
      const Duration(minutes: 5),
    );
    expect(
      RuntimeTransport.responseTimeout('RuntimeStatus'),
      const Duration(seconds: 100),
    );
  });
}
