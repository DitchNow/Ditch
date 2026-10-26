import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../design_system/ditch_theme.dart';

/// Registers logical bounds with AppKit. Its overlay also covers native editors.
class NativeFileDropRegion extends StatefulWidget {
  const NativeFileDropRegion({
    required this.enabled,
    required this.remote,
    required this.onDrop,
    required this.child,
    super.key,
  });
  final bool enabled;
  final bool remote;
  final ValueChanged<List<String>> onDrop;
  final Widget child;

  @override
  State<NativeFileDropRegion> createState() => _NativeFileDropRegionState();
}

class _NativeFileDropRegionState extends State<NativeFileDropRegion> {
  static const _channel = MethodChannel('the_ditch/composer_file_drop');
  static final _regions = <int, _NativeFileDropRegionState>{};
  static int _nextId = 0;
  final _id = _nextId++;
  List<Object>? _lastBounds;
  bool get _native => !kIsWeb && defaultTargetPlatform == TargetPlatform.macOS;

  @override
  void initState() {
    super.initState();
    if (!_native) return;
    _regions[_id] = this;
    _channel.setMethodCallHandler((call) async {
      final args = call.arguments as Map;
      final region = _regions[args['id']];
      if (region == null || !region.mounted || !region.widget.enabled) return;
      if (call.method == 'drop') {
        region.widget.onDrop((args['paths'] as List).cast<String>());
      }
    });
    WidgetsBinding.instance.addPostFrameCallback(_refreshBounds);
  }

  Future<void> _invoke(String method, Object arguments) async {
    try {
      await _channel.invokeMethod<void>(method, arguments);
    } on MissingPluginException {
      // Tests and hosts without this bridge have no native drag surface.
    }
  }

  void _refreshBounds(Duration _) {
    if (!mounted) return;
    final box = context.findRenderObject();
    if (box is RenderBox && box.hasSize) {
      final offset = box.localToGlobal(Offset.zero);
      final tokens = context.ditch;
      final background = tokens.surfaceHover.toARGB32();
      final foreground = tokens.ink.toARGB32();
      final accent = tokens.accent.toARGB32();
      final bounds = <Object>[
        offset.dx,
        offset.dy,
        box.size.width,
        box.size.height,
        widget.enabled,
        widget.remote,
        background,
        foreground,
        accent,
      ];
      if (!listEquals(_lastBounds, bounds)) {
        _lastBounds = bounds;
        unawaited(
          _invoke('register', {
            'id': _id,
            'x': offset.dx,
            'y': offset.dy,
            'width': box.size.width,
            'height': box.size.height,
            'enabled': widget.enabled,
            'remote': widget.remote,
            'background': background,
            'foreground': foreground,
            'accent': accent,
          }),
        );
      }
    }
    // Observe frames (including ancestor scrolling) without requesting frames.
    WidgetsBinding.instance.addPostFrameCallback(_refreshBounds);
  }

  @override
  void dispose() {
    if (_native) {
      _regions.remove(_id);
      unawaited(_invoke('unregister', {'id': _id}));
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}
