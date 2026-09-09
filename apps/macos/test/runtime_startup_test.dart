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

void main() {
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
