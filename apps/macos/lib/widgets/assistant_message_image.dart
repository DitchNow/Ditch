import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../design_system/ditch_theme.dart';

typedef ChatImageFetcher = Future<Uint8List> Function(Uri uri);

Uri? chatHttpsUri(String destination) {
  if (destination.contains(RegExp(r'\s|[\x00-\x1f\x7f]'))) return null;
  final uri = Uri.tryParse(destination);
  return uri != null &&
          uri.scheme == 'https' &&
          uri.host.isNotEmpty &&
          uri.userInfo.isEmpty
      ? uri
      : null;
}

const chatImageMaxBytes = 4 * 1024 * 1024;
const chatImageMaxDimension = 4096;
const chatImageMaxPixels = 8 * 1024 * 1024;

/// No cookies, credentials, or redirects; bounded both while streaming and
/// before decoding. The caller must obtain an explicit preview click first.
Future<Uint8List> fetchChatImage(Uri uri) async {
  if (chatHttpsUri(uri.toString()) == null) {
    throw const FormatException('Only HTTPS images are supported.');
  }
  final client = HttpClient()..connectionTimeout = const Duration(seconds: 10);
  try {
    return await (() async {
      final request = await client.getUrl(uri);
      request.followRedirects = false;
      request.headers.set(
        HttpHeaders.acceptHeader,
        'image/png,image/jpeg,image/webp',
      );
      final response = await request.close();
      if (response.statusCode != HttpStatus.ok ||
          !{
            'image/png',
            'image/jpeg',
            'image/webp',
          }.contains(response.headers.contentType?.mimeType) ||
          response.contentLength > chatImageMaxBytes) {
        throw const FormatException('Unsupported image response.');
      }
      final bytes = BytesBuilder(copy: false);
      await for (final chunk in response) {
        if (bytes.length + chunk.length > chatImageMaxBytes) {
          throw const FormatException('Image exceeds 4 MiB.');
        }
        bytes.add(chunk);
      }
      return bytes.takeBytes();
    })().timeout(const Duration(seconds: 20));
  } finally {
    client.close(force: true);
  }
}

/// Inspect dimensions before allocating decoded pixels. SVG, GIF, HTML and
/// animated images are deliberately not supported.
Future<ui.Image> decodeChatImage(Uint8List bytes) async {
  bool starts(List<int> signature) =>
      bytes.length >= signature.length &&
      List.generate(
        signature.length,
        (i) => bytes[i] == signature[i],
      ).every((v) => v);
  final png = starts([137, 80, 78, 71, 13, 10, 26, 10]);
  final jpeg = starts([255, 216, 255]);
  final webp =
      starts([82, 73, 70, 70]) &&
      bytes.length >= 12 &&
      String.fromCharCodes(bytes.sublist(8, 12)) == 'WEBP';
  if (bytes.length > chatImageMaxBytes || !(png || jpeg || webp)) {
    throw const FormatException(
      'Only PNG, JPEG, and static WebP up to 4 MiB are supported.',
    );
  }
  final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
  ui.ImageDescriptor? descriptor;
  ui.Codec? codec;
  try {
    descriptor = await ui.ImageDescriptor.encoded(buffer);
    if (descriptor.width > chatImageMaxDimension ||
        descriptor.height > chatImageMaxDimension ||
        descriptor.width * descriptor.height > chatImageMaxPixels) {
      throw const FormatException('Image dimensions exceed the preview limit.');
    }
    codec = await descriptor.instantiateCodec();
    if (codec.frameCount != 1) {
      throw const FormatException('Animated images are unsupported.');
    }
    return (await codec.getNextFrame()).image;
  } finally {
    codec?.dispose();
    descriptor?.dispose();
    buffer.dispose();
  }
}

class AssistantMessageImage extends StatefulWidget {
  const AssistantMessageImage({
    required this.destination,
    required this.alt,
    this.fetcher = fetchChatImage,
    super.key,
  });
  final String destination;
  final String alt;
  final ChatImageFetcher fetcher;

  @override
  State<AssistantMessageImage> createState() => _AssistantMessageImageState();
}

class _AssistantMessageImageState extends State<AssistantMessageImage> {
  ui.Image? _image;
  bool _loading = false;
  String? _error;
  int _generation = 0;

  @override
  void didUpdateWidget(AssistantMessageImage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.destination != widget.destination ||
        oldWidget.fetcher != widget.fetcher) {
      _generation++;
      _image?.dispose();
      _image = null;
      _loading = false;
      _error = null;
    }
  }

  @override
  void dispose() {
    _generation++;
    _image?.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    final uri = chatHttpsUri(widget.destination);
    if (uri == null || _loading) return;
    final generation = ++_generation;
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final image = await decodeChatImage(await widget.fetcher(uri));
      if (!mounted || generation != _generation) {
        image.dispose();
        return;
      }
      setState(() {
        _image = image;
        _loading = false;
      });
    } catch (_) {
      if (!mounted || generation != _generation) return;
      setState(() {
        _loading = false;
        _error =
            'Preview unavailable. Use a static PNG, JPEG, or WebP within the size limits.';
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final supported = chatHttpsUri(widget.destination) != null;
    final alt = widget.alt.isEmpty ? 'Image preview' : widget.alt;
    // Fixed reserved space keeps asynchronous loading from moving history.
    return SizedBox(
      width: 440,
      height: 240,
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: context.ditch.surfaceSoft,
          border: Border.all(color: context.ditch.line),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            children: [
              Text(alt, maxLines: 2, overflow: TextOverflow.ellipsis),
              const SizedBox(height: 8),
              Expanded(
                child: _image != null
                    ? Semantics(
                        image: true,
                        label: alt,
                        child: RawImage(image: _image, fit: BoxFit.contain),
                      )
                    : Center(
                        child: _loading
                            ? const CircularProgressIndicator(
                                semanticsLabel: 'Loading image preview',
                              )
                            : Column(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  if (_error != null)
                                    Text(
                                      _error!,
                                      maxLines: 3,
                                      overflow: TextOverflow.ellipsis,
                                    ),
                                  if (!supported)
                                    const Text(
                                      'Only HTTPS image previews are supported.',
                                    )
                                  else
                                    Tooltip(
                                      message: widget.destination,
                                      child: TextButton.icon(
                                        onPressed: _load,
                                        icon: const Icon(Icons.image_outlined),
                                        label: Text(
                                          _error == null
                                              ? 'Load image preview'
                                              : 'Retry preview',
                                        ),
                                      ),
                                    ),
                                ],
                              ),
                      ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
