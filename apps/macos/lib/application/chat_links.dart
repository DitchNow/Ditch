import 'dart:io';

import 'package:flutter/services.dart';

sealed class ChatLinkTarget {
  const ChatLinkTarget();
}

class ChatWebLink extends ChatLinkTarget {
  const ChatWebLink(this.uri);
  final Uri uri;
}

class ChatFileLink extends ChatLinkTarget {
  const ChatFileLink(this.path, {this.withoutLineSuffix});
  final String path;
  // A colon is legal in a macOS filename. Prefer the exact path if it exists.
  final String? withoutLineSuffix;
}

class UnsupportedChatLink extends ChatLinkTarget {
  const UnsupportedChatLink(this.message);
  final String message;
}

const _unsupported = UnsupportedChatLink(
  'Only HTTPS links and local file paths are supported.',
);
final _controls = RegExp(r'[\x00-\x1f\x7f]');
final _lineFragment = RegExp(
  r'^L[1-9][0-9]*(?:C[1-9][0-9]*)?(?:-L?[1-9][0-9]*(?:C[1-9][0-9]*)?)?$',
);
final _lineSuffix = RegExp(r':[1-9][0-9]*(?::[1-9][0-9]*)?$');

/// Resolves only explicit Markdown destinations, never the process directory.
/// Image loading intentionally has its own, narrower HTTPS policy.
ChatLinkTarget resolveChatLink(
  String destination, {
  String? projectPath,
  bool isRemote = false,
  String? remoteHost,
}) {
  if (destination.isEmpty || _controls.hasMatch(destination)) {
    return _unsupported;
  }
  var uri = Uri.tryParse(destination);
  if (uri == null) return _unsupported;
  // `README.md:12` otherwise parses as a URI scheme, unlike `lib/a.dart:12`.
  final suffix = _lineSuffix.firstMatch(destination);
  if (uri.hasScheme &&
      suffix != null &&
      RegExp(
        r'^[^:/?#]+\.[^:/?#]+$',
      ).hasMatch(destination.substring(0, suffix.start))) {
    uri = Uri.tryParse('./$destination');
    if (uri == null) return _unsupported;
  }
  if (uri.scheme == 'https') {
    return uri.host.isNotEmpty &&
            uri.userInfo.isEmpty &&
            !destination.contains(RegExp(r'\s'))
        ? ChatWebLink(uri)
        : _unsupported;
  }
  // Reject executable schemes, network file hosts, and protocol-relative URLs.
  if (uri.hasScheme && uri.scheme != 'file') return _unsupported;
  // Uri normalizes `file:relative` into an absolute-looking path. Require an
  // explicit slash in the original file URL rather than inventing a root.
  if (uri.scheme == 'file' &&
      !RegExp(r'^file:/', caseSensitive: false).hasMatch(destination)) {
    return _unsupported;
  }
  if (uri.hasAuthority &&
      !(uri.scheme == 'file' &&
          (uri.host.isEmpty || uri.host == 'localhost') &&
          uri.userInfo.isEmpty &&
          !uri.hasPort)) {
    return _unsupported;
  }
  if (uri.hasQuery ||
      (uri.hasFragment && !_lineFragment.hasMatch(uri.fragment))) {
    return _unsupported;
  }
  if (isRemote) {
    return UnsupportedChatLink(
      'This file lives on ${remoteHost ?? 'the remote host'} and cannot be shown in local Finder.',
    );
  }
  try {
    // Decode exactly once. Encoded #, %, spaces, and Unicode belong to the name.
    var path = Uri.decodeComponent(uri.path);
    if (path.isEmpty || _controls.hasMatch(path) || path.startsWith('//')) {
      return _unsupported;
    }
    if (uri.scheme == 'file' && !path.startsWith('/')) return _unsupported;
    if (!path.startsWith('/')) {
      if (projectPath == null ||
          !projectPath.startsWith('/') ||
          projectPath.startsWith('//') ||
          _controls.hasMatch(projectPath)) {
        return const UnsupportedChatLink(
          'The project folder is unavailable for this relative file link.',
        );
      }
      path = '$projectPath/$path';
    }
    path = Uri.file(
      path,
      windows: false,
    ).normalizePath().toFilePath(windows: false);
    final suffix = _lineSuffix.firstMatch(path);
    return ChatFileLink(
      path,
      withoutLineSuffix: suffix == null
          ? null
          : path.substring(0, suffix.start),
    );
  } on FormatException {
    return _unsupported;
  } on ArgumentError {
    return _unsupported;
  }
}

class ChatLinkFailure implements Exception {
  const ChatLinkFailure(this.message);
  final String message;
}

typedef ChatFileRevealer = Future<void> Function(ChatFileLink file);

Future<void> revealChatFile(ChatFileLink file) async {
  var path = file.path;
  if (!path.startsWith('/') ||
      path.startsWith('//') ||
      _controls.hasMatch(path)) {
    throw const ChatLinkFailure('Invalid local file path.');
  }
  if (await FileSystemEntity.type(path) == FileSystemEntityType.notFound) {
    final fallback = file.withoutLineSuffix;
    if (fallback == null ||
        !fallback.startsWith('/') ||
        fallback.startsWith('//') ||
        _controls.hasMatch(fallback) ||
        await FileSystemEntity.type(fallback) ==
            FileSystemEntityType.notFound) {
      throw ChatLinkFailure('File not found: $path');
    }
    path = fallback;
  }
  final revealed = await const MethodChannel(
    'the_ditch/application',
  ).invokeMethod<bool>('revealFileInFinder', path);
  if (revealed != true) {
    throw const ChatLinkFailure(
      'Could not show this file in Finder. It may have moved or become unavailable.',
    );
  }
}
