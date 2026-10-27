import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/widgets/native_file_drop_region.dart';

void main() {
  testWidgets(
    'region refreshes layout and rejects disposed and disabled callbacks',
    (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      const channel = MethodChannel('the_ditch/composer_file_drop');
      final calls = <MethodCall>[];
      final drops = <List<String>>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        calls.add(call);
        return null;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      Future<void> mount(
        String identity, {
        double width = 300,
        bool enabled = true,
      }) async {
        await tester.pumpWidget(
          MaterialApp(
            theme: DitchTheme.light(),
            home: Align(
              alignment: Alignment.topLeft,
              child: NativeFileDropRegion(
                key: ValueKey(identity),
                enabled: enabled,
                remote: false,
                onDrop: drops.add,
                child: SizedBox(width: width, height: 180),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
      }

      Future<void> deliver(int id) async {
        final done = Completer<void>();
        tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          channel.name,
          const StandardMethodCodec().encodeMethodCall(
            MethodCall('drop', {
              'id': id,
              'paths': ['/tmp/file.md'],
            }),
          ),
          (_) => done.complete(),
        );
        await done.future;
      }

      Map getRegistration() =>
          calls.lastWhere((call) => call.method == 'register').arguments as Map;
      await mount('first');
      final first = getRegistration()['id'] as int;
      expect(getRegistration()['width'], 300);
      await deliver(first);
      expect(drops, hasLength(1));
      await mount('first', width: 450);
      expect(getRegistration()['width'], 450);
      await mount('second');
      expect(
        calls.any(
          (call) =>
              call.method == 'unregister' &&
              (call.arguments as Map)['id'] == first,
        ),
        isTrue,
      );
      await deliver(first);
      expect(drops, hasLength(1));
      final second = getRegistration()['id'] as int;
      await mount('second', enabled: false);
      await deliver(second);
      expect(drops, hasLength(1));
      await tester.pumpWidget(const SizedBox());
      await tester.pump();
      expect(calls.last.method, 'unregister');
      debugDefaultTargetPlatformOverride = null;
    },
  );
}
