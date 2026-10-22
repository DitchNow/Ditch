import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/data/runtime_models.dart';

class _UnavailableRuntime extends DitchRuntimeClient {
  int statusRequests = 0;

  @override
  Future<RuntimeStatusDto> runtimeStatus() async {
    statusRequests++;
    throw StateError('test connection closed after native readiness');
  }
}

class _ConnectedRuntime extends DitchRuntimeClient {
  _ConnectedRuntime({
    this.environment = 'staging',
    this.wrongSocket = false,
    this.holdConfirmation = false,
  }) : super(deploymentEnvironment: 'staging');

  final String environment;
  final bool wrongSocket;
  final bool holdConfirmation;
  final confirmation = Completer<void>();
  int statusRequests = 0;
  int snapshots = 0;
  final streams = <StreamController<Map<String, dynamic>>>[];

  @override
  Future<RuntimeStatusDto> runtimeStatus() async {
    statusRequests++;
    if (holdConfirmation && statusRequests == 2) await confirmation.future;
    return RuntimeStatusDto(
      identity: 'test',
      pid: 123,
      socketPath: wrongSocket ? '/wrong.sock' : socketPath,
      activeSessionCount: 0,
      attentionCount: 0,
      unreadAttentionCount: 0,
      instanceId: 'test-instance',
      deploymentEnvironment: environment,
      capabilities: {
        'app_server_sessions_v2',
        'persistent_sessions_v1',
        'always_on_web_access_v1',
      },
    );
  }

  @override
  Future<Map<String, dynamic>> snapshot() async {
    snapshots++;
    return {
      'Snapshot': {'projects': [], 'agents': [], 'attention': []},
    };
  }

  @override
  Future<Stream<Map<String, dynamic>>> subscribeEvents() async {
    final stream = StreamController<Map<String, dynamic>>();
    streams.add(stream);
    return stream.stream;
  }

  @override
  Future<Map<String, dynamic>> request(Object body) async => {};
}

void main() {
  testWidgets('late readiness cannot erase a rejected event subscription', (
    tester,
  ) async {
    var nativeRequests = 0;
    const channel = MethodChannel('the_ditch/application');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
      call,
    ) async {
      if (call.method == 'runtimeAvailable') {
        nativeRequests++;
        return true;
      }
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );
    final client = _ConnectedRuntime(holdConfirmation: true);
    await tester.pumpWidget(
      MaterialApp(
        home: CommandCenterScreen(
          runtimeClient: client,
          deploymentEnvironment: 'staging',
        ),
      ),
    );
    await tester.pump();
    expect(client.streams.length, 1);
    client.streams.single.addError(
      const DitchRuntimeException(
        'client_not_authorized',
        'Subscription denied',
      ),
    );
    await tester.pump();
    await tester.pump();
    client.confirmation.complete();
    await tester.pump();
    await tester.pump(const Duration(seconds: 2));
    expect(find.byType(RuntimeRecoveryView), findsOneWidget);
    expect(find.textContaining('Subscription denied'), findsWidgets);
    expect(nativeRequests, 1);
    await tester.pumpWidget(const SizedBox.shrink());
    unawaited(client.streams.single.close());
    expect(tester.takeException(), isNull);
  });

  for (final mismatch in ['environment', 'socket']) {
    testWidgets('rejects wrong $mismatch before reading a snapshot', (
      tester,
    ) async {
      var nativeRequests = 0;
      const channel = MethodChannel('the_ditch/application');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        if (call.method == 'runtimeAvailable') {
          nativeRequests++;
          return true;
        }
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      final client = _ConnectedRuntime(
        environment: mismatch == 'environment' ? 'production' : 'staging',
        wrongSocket: mismatch == 'socket',
      );
      await tester.pumpWidget(
        MaterialApp(
          home: CommandCenterScreen(
            runtimeClient: client,
            deploymentEnvironment: 'staging',
          ),
        ),
      );
      await tester.pump();
      expect(client.snapshots, 0);
      expect(client.streams, isEmpty);
      expect(find.byType(RuntimeRecoveryView), findsOneWidget);
      expect(find.textContaining('different identity'), findsWidgets);
      await tester.pump(const Duration(seconds: 5));
      expect(nativeRequests, 1);
      await tester.pumpWidget(const SizedBox.shrink());
    });
  }

  testWidgets(
    'short-lived event streams exhaust retries; manual retry recovers',
    (tester) async {
      var nativeRequests = 0;
      const channel = MethodChannel('the_ditch/application');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        if (call.method == 'runtimeAvailable') {
          nativeRequests++;
          return true;
        }
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      final client = _ConnectedRuntime();
      await tester.pumpWidget(
        MaterialApp(
          home: CommandCenterScreen(
            runtimeClient: client,
            deploymentEnvironment: 'staging',
          ),
        ),
      );
      await tester.pump();
      for (var attempt = 0; attempt < 3; attempt++) {
        expect(client.streams.length, attempt + 1);
        unawaited(client.streams.last.close());
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 500));
        await tester.pump();
      }
      expect(nativeRequests, 3);
      expect(find.byType(RuntimeRecoveryView), findsOneWidget);
      await tester.pump(const Duration(seconds: 5));
      expect(nativeRequests, 3);
      await tester.tap(find.text('Retry Connection'));
      await tester.pump();
      expect(nativeRequests, 4);
      expect(find.byType(RuntimeRecoveryView), findsNothing);
      // A sustained connection replenishes the budget for a later disconnect.
      await tester.pump(const Duration(seconds: 31));
      unawaited(client.streams.last.close());
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 500));
      await tester.pump();
      expect(nativeRequests, 5);
      // Authorization failures are terminal, even with retries remaining.
      client.streams.last.addError(
        const DitchRuntimeException(
          'client_not_authorized',
          'Subscription denied',
        ),
      );
      unawaited(client.streams.last.close());
      await tester.pump();
      await tester.pump();
      expect(find.byType(RuntimeRecoveryView), findsOneWidget);
      expect(find.textContaining('Subscription denied'), findsWidgets);
      await tester.pump(const Duration(seconds: 2));
      expect(nativeRequests, 5);
      await tester.pumpWidget(const SizedBox.shrink());
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'connection races retry twice then allow a fresh manual attempt',
    (tester) async {
      var nativeRequests = 0;
      const channel = MethodChannel('the_ditch/application');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        if (call.method == 'runtimeAvailable') {
          nativeRequests++;
          return true;
        }
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      final client = _UnavailableRuntime();
      await tester.pumpWidget(
        MaterialApp(home: CommandCenterScreen(runtimeClient: client)),
      );
      await tester.pump();
      expect(nativeRequests, 1);
      expect(find.byType(RuntimeRecoveryView), findsNothing);
      for (var retry = 0; retry < 2; retry++) {
        await tester.pump(const Duration(milliseconds: 500));
        await tester.pump();
      }
      expect(nativeRequests, 3);
      expect(client.statusRequests, 3);
      expect(find.byType(RuntimeRecoveryView), findsOneWidget);
      await tester.pump(const Duration(seconds: 2));
      expect(nativeRequests, 3);
      await tester.tap(find.text('Retry Connection'));
      await tester.pump();
      expect(nativeRequests, 4);
      expect(find.byType(RuntimeRecoveryView), findsNothing);
      await tester.pumpWidget(const SizedBox.shrink());
      await tester.pump(const Duration(seconds: 1));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'startup stays pending until native recovery finishes and Retry starts it again',
    (tester) async {
      final attempts = <Completer<bool>>[];
      const channel = MethodChannel('the_ditch/application');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        if (call.method == 'runtimeAvailable') {
          final pending = Completer<bool>();
          attempts.add(pending);
          return pending.future;
        }
        return switch (call.method) {
          'getThemeMode' => 'system',
          'getPaneWidths' => <String, double>{},
          _ => null,
        };
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );

      await tester.pumpWidget(const TheDitchApp());
      await tester.pump(const Duration(milliseconds: 100));
      expect(attempts, hasLength(1));
      expect(find.text('Starting runtime'), findsOneWidget);
      expect(find.byType(RuntimeRecoveryView), findsNothing);

      attempts.single.completeError(
        PlatformException(
          code: 'runtime_startup_failed',
          message:
              'The previous Ditch Runtime is still shutting down. Click Retry to continue.',
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byType(RuntimeRecoveryView), findsOneWidget);
      expect(find.textContaining('PlatformException'), findsNothing);
      expect(find.textContaining('runtime_startup_failed'), findsNothing);
      expect(find.textContaining('still shutting down'), findsWidgets);

      await tester.tap(find.text('Retry Connection'));
      await tester.pump(const Duration(milliseconds: 100));
      expect(attempts, hasLength(2));
      expect(find.text('Starting runtime'), findsOneWidget);
      expect(find.byType(RuntimeRecoveryView), findsNothing);

      // Disposing the UI during a pending native startup must not update a
      // disposed presentation controller when the result arrives.
      await tester.pumpWidget(const SizedBox.shrink());
      attempts.last.complete(false);
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );
}
