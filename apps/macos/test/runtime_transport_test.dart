import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/data/runtime_transport.dart';

void main() {
  test(
    'each environment selects its own socket and unknown environments fail',
    () {
      final home = Platform.environment['HOME'] ?? '.';
      for (final entry in {
        'staging': 'The Ditch Staging',
        'production': 'The Ditch',
      }.entries) {
        expect(
          RuntimeTransport(deploymentEnvironment: entry.key).socketPath,
          '$home/Library/Application Support/${entry.value}/ditchd.sock',
        );
      }
      expect(
        RuntimeTransport().socketPath,
        RuntimeTransport.defaultSocketPath(
          deploymentEnvironment: runtimeDeploymentEnvironment,
        ),
      );
      expect(
        () => RuntimeTransport(deploymentEnvironment: 'typo'),
        throwsArgumentError,
      );
      expect(
        RuntimeTransport(socketPath: '/test.sock').socketPath,
        '/test.sock',
      );
    },
  );

  test(
    'a denied subscription surfaces its protocol error and closes the socket',
    () async {
      final directory = await Directory.systemTemp.createTemp('ipc-');
      final server = await ServerSocket.bind(
        InternetAddress('${directory.path}/s', type: InternetAddressType.unix),
        0,
      );
      addTearDown(() async {
        await server.close();
        await directory.delete(recursive: true);
      });
      final disconnected = Completer<void>();
      Socket? peer;
      addTearDown(() => peer?.destroy());
      server.listen((socket) {
        peer = socket;
        socket.listen(
          (_) {
            socket.writeln(
              jsonEncode({
                'body': {
                  'Error': {
                    'code': 'client_not_authorized',
                    'message': 'Subscription denied',
                  },
                },
              }),
            );
          },
          onDone: () {
            if (!disconnected.isCompleted) disconnected.complete();
          },
        );
      });
      final stream = await RuntimeTransport(
        socketPath: '${directory.path}/s',
      ).subscribeEvents();
      await expectLater(
        stream,
        emitsInOrder([
          emitsError(
            isA<DitchRuntimeException>().having(
              (e) => e.code,
              'code',
              'client_not_authorized',
            ),
          ),
          emitsDone,
        ]),
      );
      await disconnected.future.timeout(const Duration(seconds: 2));
    },
  );

  test('cancelling a healthy subscription releases its socket', () async {
    final directory = await Directory.systemTemp.createTemp('ipc-');
    final server = await ServerSocket.bind(
      InternetAddress('${directory.path}/s', type: InternetAddressType.unix),
      0,
    );
    addTearDown(() async {
      await server.close();
      await directory.delete(recursive: true);
    });
    final disconnected = Completer<void>();
    Socket? peer;
    addTearDown(() => peer?.destroy());
    server.listen((socket) {
      peer = socket;
      socket.listen(
        (_) => socket.writeln(
          jsonEncode({
            'body': {'event': 'Heartbeat'},
          }),
        ),
        onDone: () {
          if (!disconnected.isCompleted) disconnected.complete();
        },
      );
    });
    final stream = await RuntimeTransport(
      socketPath: '${directory.path}/s',
    ).subscribeEvents();
    final received = Completer<void>();
    final subscription = stream.listen((event) {
      expect(event['event'], 'Heartbeat');
      if (!received.isCompleted) received.complete();
    });
    await received.future.timeout(const Duration(seconds: 2));
    await subscription.cancel();
    await disconnected.future.timeout(const Duration(seconds: 2));
  });
}
