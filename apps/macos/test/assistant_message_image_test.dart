import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/widgets/assistant_message_image.dart';

class _Headers implements HttpHeaders {
  _Headers(this.contentType);
  @override
  ContentType? contentType;
  final sent = <String, Object>{};
  @override
  void set(String name, Object value, {bool preserveHeaderCase = false}) {
    sent[name] = value;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Response extends StreamView<List<int>> implements HttpClientResponse {
  _Response({
    this.statusCode = 200,
    this.contentLength = -1,
    String type = 'image/png',
    List<List<int>> chunks = const [],
  }) : headers = _Headers(ContentType.parse(type)),
       super(Stream.fromIterable(chunks));
  @override
  final int statusCode;
  @override
  final int contentLength;
  @override
  final HttpHeaders headers;
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Request implements HttpClientRequest {
  _Request(this.response);
  final _Response response;
  @override
  bool followRedirects = true;
  @override
  final _Headers headers = _Headers(null);
  @override
  Future<HttpClientResponse> close() async => response;
  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

class _Client implements HttpClient {
  _Client(_Response response) : request = _Request(response);
  final _Request request;
  bool closed = false;
  Uri? destination;
  @override
  Duration? connectionTimeout;
  @override
  Future<HttpClientRequest> getUrl(Uri uri) async {
    destination = uri;
    return request;
  }

  @override
  void close({bool force = false}) {
    closed = force;
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

void main() {
  test(
    'download sends no credentials, disables redirects and closes its client',
    () async {
      final client = _Client(
        _Response(
          chunks: [
            [1, 2],
            [3, 4],
          ],
        ),
      );
      final bytes = await HttpOverrides.runZoned(
        () => fetchChatImage(Uri.parse('https://example.com/image.png')),
        createHttpClient: (_) => client,
      );
      expect(bytes, [1, 2, 3, 4]);
      expect(client.request.followRedirects, isFalse);
      expect(client.request.headers.sent.keys, [HttpHeaders.acceptHeader]);
      expect(client.closed, isTrue);
    },
  );

  test(
    'download rejects redirects, MIME types, errors and both size overflows',
    () async {
      for (final response in [
        _Response(statusCode: 302),
        _Response(statusCode: 404),
        _Response(type: 'image/svg+xml'),
        _Response(type: 'text/html'),
        _Response(contentLength: chatImageMaxBytes + 1),
        _Response(
          chunks: [
            Uint8List(chatImageMaxBytes),
            [1],
          ],
        ),
      ]) {
        final client = _Client(response);
        await expectLater(
          HttpOverrides.runZoned(
            () => fetchChatImage(Uri.parse('https://example.com/image.png')),
            createHttpClient: (_) => client,
          ),
          throwsFormatException,
        );
        expect(client.closed, isTrue);
      }
    },
  );

  test(
    'unsupported destinations fail before creating a network client',
    () async {
      var clients = 0;
      for (final url in [
        'file:///tmp/a',
        'http://example.com/a',
        '/relative.png',
        'https://user:password@example.com/a',
        'data:image/png;base64,AA',
      ]) {
        await expectLater(
          HttpOverrides.runZoned(
            () => fetchChatImage(Uri.parse(url)),
            createHttpClient: (_) {
              clients++;
              throw StateError('Unexpected network');
            },
          ),
          throwsFormatException,
        );
      }
      expect(clients, 0);
    },
  );
}
