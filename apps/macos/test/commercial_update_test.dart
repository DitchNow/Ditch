import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/data/runtime_models.dart';
import 'package:the_ditch/main.dart';

const _application = MethodChannel('the_ditch/application');
const _releaseId = '11111111-1111-4111-8111-111111111111';

Map<String, dynamic> _release({
  required String bearer,
  String id = _releaseId,
  int build = 113,
  String edition = 'commercial',
  String expiresAt = '2026-09-09T14:30:00.123Z',
}) => {
  'manifest': {
    'edition': edition,
    'release_id': id,
    'appcast_url':
        'https://relay.example.test/v1/$edition/releases/$id/appcast',
    'artifact_url':
        'https://relay.example.test/v1/$edition/releases/$id/artifact/ditch.dmg',
    'artifact_size': 4096,
    'version': '0.1.0',
    'build': '$build',
    'channel': 'beta',
    'release_sequence': build,
  },
  'update_session': {'bearer': bearer, 'expires_at': expiresAt},
};

class _UpdateClient extends DitchRuntimeClient {
  _UpdateClient({this.status = 'active', this.environment = 'staging'})
    : super(socketPath: '/tmp/ditch-update-test.sock');

  final String status;
  final String environment;
  int communityChecks = 0;
  bool communityUnavailable = false;

  int checks = 0;
  Future<Map<String, dynamic>> Function()? onRefresh;

  @override
  Future<RuntimeStatusDto> runtimeStatus() async => RuntimeStatusDto(
    identity: 'The Ditch Runtime',
    pid: 123,
    socketPath: '/tmp/ditch-update-test.sock',
    activeSessionCount: 0,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'test-runtime',
    capabilities: {},
    buildVersion: '0.1.0',
    edition: 'commercial',
    deploymentEnvironment: environment,
    buildIdentifier: 'test-build',
    buildNumber: '112',
    releaseSequence: 112,
  );

  @override
  Future<Map<String, dynamic>> commercialEntitlement() async => {
    'active': status == 'active' || status == 'over_limit',
    'plan': 'commercial_monthly',
    'status': status,
  };

  @override
  Future<Map<String, dynamic>> checkCommunityRelease() async {
    communityChecks++;
    if (communityUnavailable) {
      throw DitchRuntimeException(
        'commercial_release_unavailable',
        'release_unavailable',
      );
    }
    return _release(
      edition: 'community',
      bearer:
          'community-permission-$communityChecks-with-at-least-32-characters',
    );
  }

  @override
  Future<Map<String, dynamic>> checkCommercialRelease() async {
    checks++;
    if (checks > 1 && onRefresh != null) return onRefresh!();
    return _release(
      bearer: 'permission-$checks-with-at-least-32-characters',
      // The discovery session is deliberately expired. Installation must
      // obtain a new session from the runtime, which verifies it in production.
      expiresAt: checks == 1
          ? '2020-01-01T00:00:00.123Z'
          : '2026-09-09T14:30:00.123Z',
    );
  }
}

Future<void> _openUpdate(
  WidgetTester tester,
  _UpdateClient client,
  List<MethodCall> installs, {
  PlatformException? installError,
  bool expectRelease = true,
}) async {
  tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(_application, (
    call,
  ) async {
    if (call.method == 'appVersion') {
      return {'version': '0.1.0', 'build': '112'};
    }
    if (call.method == 'installCommercialUpdate') {
      installs.add(call);
      if (installError != null) throw installError;
      return true;
    }
    return null;
  });
  addTearDown(
    () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      _application,
      null,
    ),
  );
  await tester.pumpWidget(MaterialApp(home: DitchUpdateDialog(client: client)));
  await tester.pumpAndSettle();
  await tester.tap(find.byKey(const Key('check-ditch-update')));
  await tester.pumpAndSettle();
  if (expectRelease) {
    expect(find.text('Ditch 0.1.0.113 is available.'), findsOneWidget);
  }
}

Future<void> _install(WidgetTester tester) async {
  await tester.tap(find.byKey(const Key('install-ditch-update')));
  await tester.pumpAndSettle();
}

void main() {
  for (final environment in ['staging', 'production']) {
    for (final status in ['inactive', 'expired', 'active', 'over_limit']) {
      testWidgets(
        '$environment $status selects and refreshes the authorized update route',
        (tester) async {
          final client = _UpdateClient(
            status: status,
            environment: environment,
          );
          final installs = <MethodCall>[];
          await _openUpdate(tester, client, installs);
          await _install(tester);
          final paid = status == 'active' || status == 'over_limit';
          expect(client.checks, paid ? 2 : 0);
          expect(client.communityChecks, paid ? 0 : 2);
          expect(installs, hasLength(1));
          final arguments = installs.single.arguments as Map;
          expect(arguments['edition'], paid ? 'commercial' : 'community');
          expect(
            arguments['authorization_bearer'],
            contains(paid ? 'permission-2-' : 'community-permission-2-'),
          );
          expect(
            arguments['appcast_url'],
            contains('/v1/${paid ? 'commercial' : 'community'}/releases/'),
          );
        },
      );
    }
  }

  testWidgets(
    'no published Community update shows a message without launching Sparkle',
    (tester) async {
      final client = _UpdateClient(
        status: 'inactive',
        environment: 'production',
      )..communityUnavailable = true;
      final installs = <MethodCall>[];
      await _openUpdate(tester, client, installs, expectRelease: false);
      expect(client.communityChecks, 1);
      expect(client.checks, 0);
      expect(installs, isEmpty);
      expect(
        find.text('No compatible Community update is currently available.'),
        findsOneWidget,
      );
      expect(find.byKey(const Key('install-ditch-update')), findsNothing);
    },
  );

  testWidgets('install replaces the expired discovery permission', (
    tester,
  ) async {
    final client = _UpdateClient();
    final installs = <MethodCall>[];
    await _openUpdate(tester, client, installs);
    await _install(tester);

    expect(client.checks, 2);
    expect(installs, hasLength(1));
    expect(
      installs.single.arguments,
      authorizedCommercialUpdateArguments(
        _release(bearer: 'permission-2-with-at-least-32-characters'),
      ),
    );
    expect(find.textContaining('Follow the Sparkle window'), findsOneWidget);
  });

  testWidgets(
    'expired native permission has a readable error and fresh retry',
    (tester) async {
      final client = _UpdateClient();
      final installs = <MethodCall>[];
      await _openUpdate(
        tester,
        client,
        installs,
        installError: PlatformException(code: 'invalid_update_session'),
      );
      await _install(tester);
      expect(
        find.textContaining('download permission is invalid or expired'),
        findsOneWidget,
      );
      expect(find.textContaining('PlatformException'), findsNothing);
      expect(find.textContaining('could not check for updates'), findsNothing);
      await _install(tester);
      expect(client.checks, 3);
      expect(installs, hasLength(2));
      expect(
        installs.last.arguments['authorization_bearer'],
        'permission-3-with-at-least-32-characters',
      );
    },
  );

  testWidgets('unexpected native install error describes installation', (
    tester,
  ) async {
    await _openUpdate(
      tester,
      _UpdateClient(),
      [],
      installError: PlatformException(
        code: 'unexpected',
        message: 'internal details',
      ),
    );
    await _install(tester);
    expect(
      find.text('Ditch could not start installation. Please try again.'),
      findsOneWidget,
    );
    expect(find.textContaining('internal details'), findsNothing);
  });

  testWidgets(
    'failed refresh never starts the updater with cached permission',
    (tester) async {
      final client = _UpdateClient()
        ..onRefresh = () async => throw const DitchRuntimeException(
          'commercial_release_unavailable',
          'release_unavailable',
        );
      final installs = <MethodCall>[];
      await _openUpdate(tester, client, installs);
      await _install(tester);
      expect(installs, isEmpty);
      expect(
        find.text('No compatible Commercial update is currently available.'),
        findsOneWidget,
      );
    },
  );

  testWidgets('a different release is shown before it can be installed', (
    tester,
  ) async {
    final freshRelease = _release(
      bearer: 'new-release-permission-with-at-least-32-characters',
      id: '22222222-2222-4222-8222-222222222222',
      build: 114,
    );
    final client = _UpdateClient()..onRefresh = () async => freshRelease;
    final installs = <MethodCall>[];
    await _openUpdate(tester, client, installs);
    await _install(tester);
    expect(installs, isEmpty);
    expect(
      find.textContaining('Ditch 0.1.0.114 is now available.'),
      findsOneWidget,
    );
    await _install(tester);
    expect(client.checks, 3);
    expect(
      installs.single.arguments,
      authorizedCommercialUpdateArguments(freshRelease),
    );
  });

  testWidgets('refresh does not install an already installed release', (
    tester,
  ) async {
    final client = _UpdateClient()
      ..onRefresh = () async =>
          _release(bearer: 'unused-permission', build: 112);
    final installs = <MethodCall>[];
    await _openUpdate(tester, client, installs);
    await _install(tester);
    expect(installs, isEmpty);
    expect(find.byKey(const Key('install-ditch-update')), findsNothing);
    expect(find.textContaining('Ditch is up to date'), findsOneWidget);
  });

  testWidgets('closing the dialog during refresh does not start installation', (
    tester,
  ) async {
    final pending = Completer<Map<String, dynamic>>();
    final client = _UpdateClient()..onRefresh = () => pending.future;
    final installs = <MethodCall>[];
    await _openUpdate(tester, client, installs);
    await tester.tap(find.byKey(const Key('install-ditch-update')));
    await tester.pump();
    expect(
      tester
          .widget<FilledButton>(find.byKey(const Key('install-ditch-update')))
          .onPressed,
      isNull,
    );
    await tester.pumpWidget(const SizedBox.shrink());
    pending.complete(_release(bearer: 'unused-permission'));
    await tester.pumpAndSettle();
    expect(installs, isEmpty);
    expect(tester.takeException(), isNull);
  });
}
