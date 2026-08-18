import 'dart:convert';
import 'dart:io';
import 'dart:math';

class RuntimeTransport {
  RuntimeTransport({String? socketPath})
    : socketPath = socketPath ?? defaultSocketPath();

  final String socketPath;
  final _random = Random.secure();

  static String defaultSocketPath() {
    final home = Platform.environment['HOME'] ?? '.';
    return '$home/Library/Application Support/The Ditch/ditchd.sock';
  }

  Future<Map<String, dynamic>> request(Object body) async {
    final socket = await Socket.connect(
      InternetAddress(socketPath, type: InternetAddressType.unix),
      0,
      timeout: const Duration(seconds: 2),
    );
    try {
      socket.writeln(
        jsonEncode({
          'protocol_version': 1,
          'id': _newRequestUuid(),
          'sent_at': DateTime.now().toUtc().toIso8601String(),
          'body': body,
        }),
      );
      await socket.flush();
      final line = await utf8.decoder
          .bind(socket)
          .transform(const LineSplitter())
          .first;
      return parseRuntimeResponseLine(line);
    } finally {
      socket.destroy();
    }
  }

  Future<Stream<Map<String, dynamic>>> subscribeEvents({
    int sinceSequence = 0,
  }) async {
    final socket = await Socket.connect(
      InternetAddress(socketPath, type: InternetAddressType.unix),
      0,
      timeout: const Duration(seconds: 2),
    );
    socket.writeln(
      jsonEncode({
        'protocol_version': 1,
        'id': _newRequestUuid(),
        'sent_at': DateTime.now().toUtc().toIso8601String(),
        'body': {
          'SubscribeEvents': {'since_sequence': sinceSequence},
        },
      }),
    );
    await socket.flush();
    return utf8.decoder.bind(socket).transform(const LineSplitter()).map((
      line,
    ) {
      final decoded = jsonDecode(line);
      if (decoded is! Map<String, dynamic>) {
        throw const FormatException('runtime event envelope was not an object');
      }
      final body = decoded['body'];
      if (body is! Map<String, dynamic>) {
        throw const FormatException('runtime event body was not an object');
      }
      return body;
    });
  }

  String _newRequestUuid() {
    final bytes = List<int>.generate(16, (_) => _random.nextInt(256));
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    final hex = bytes
        .map((byte) => byte.toRadixString(16).padLeft(2, '0'))
        .join();
    return '${hex.substring(0, 8)}-${hex.substring(8, 12)}-${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}';
  }
}

class DitchRuntimeException implements Exception {
  const DitchRuntimeException(this.code, this.message);

  final String code;
  final String message;

  @override
  String toString() => '$code: $message';
}

Map<String, dynamic> parseRuntimeResponseLine(String line) {
  final decoded = jsonDecode(line);
  if (decoded is! Map<String, dynamic>) {
    throw const FormatException('runtime response was not an object');
  }

  final responseBody = decoded['body'];
  if (responseBody == 'Accepted') {
    return const {'Accepted': true};
  }
  if (responseBody is! Map<String, dynamic>) {
    throw const FormatException('runtime response body was not an object');
  }

  final error = responseBody['Error'];
  if (error is Map<String, dynamic>) {
    final code = error['code']?.toString() ?? 'runtime_error';
    final message = error['message']?.toString() ?? 'Unknown runtime error';
    throw DitchRuntimeException(code, message);
  }
  return responseBody;
}
