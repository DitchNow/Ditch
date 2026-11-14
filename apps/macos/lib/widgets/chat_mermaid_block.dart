import 'package:flutter/material.dart';
import 'package:mermaid_core/mermaid_core.dart' as core;
import 'package:mermaid_flutter/mermaid_flutter.dart';

import '../design_system/ditch_theme.dart';
import 'chat_code_source.dart';
import 'chat_mermaid_viewport.dart';
import 'mermaid_render_task.dart';

class ChatMermaidBlock extends StatefulWidget {
  const ChatMermaidBlock({
    required this.source,
    this.renderer = startMermaidRender,
    super.key,
  });

  final String source;
  final MermaidRenderer renderer;

  @override
  State<ChatMermaidBlock> createState() => _ChatMermaidBlockState();
}

enum _Status { loading, ready, cancelled, failed }

class _Preview {
  const _Preview(this.source, this.status, {this.result, this.error});
  final String source;
  final _Status status;
  final MermaidRenderResult? result;
  final String? error;
}

class _ChatMermaidBlockState extends State<ChatMermaidBlock> {
  final _preview = ValueNotifier(const _Preview('', _Status.loading));
  core.MermaidTheme? _theme;
  MermaidRenderTask? _task;
  int _generation = 0;
  bool _showSource = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final theme = Theme.of(context);
    final tokens = context.ditch;
    final nextTheme = MaterialMermaidTheme.fromColorScheme(
      theme.colorScheme.copyWith(
        surface: tokens.surfaceSoft,
        secondaryContainer: tokens.successSoft,
        onSecondaryContainer: tokens.success,
        secondary: tokens.success,
        surfaceContainerLow: tokens.surface,
        surfaceContainerHigh: tokens.surfaceSoft,
        onSurfaceVariant: tokens.strongMuted,
      ),
      textTheme: theme.textTheme,
    );
    if (_theme == nextTheme) return;
    _theme = nextTheme;
    _schedule();
  }

  @override
  void didUpdateWidget(ChatMermaidBlock oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.source != widget.source ||
        oldWidget.renderer != widget.renderer) {
      _schedule();
    }
  }

  void _schedule() {
    final generation = ++_generation;
    _task?.cancel();
    _task = null;
    // Avoid notifying the expanded route during an inherited-theme rebuild.
    // Also paint a loading frame before beginning font measurement.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || generation != _generation) return;
      final source = widget.source;
      if (source.contains('%%{') || source.trimLeft().startsWith('---')) {
        _preview.value = _Preview(
          source,
          _Status.failed,
          error: 'Diagram configuration is not supported. Its source is below.',
        );
        return;
      }
      _preview.value = _Preview(source, _Status.loading);
      try {
        final task = _task = widget.renderer(source, _theme!);
        task.result.then(
          (result) {
            if (!mounted || generation != _generation) return;
            _task = null;
            _preview.value = _Preview(source, _Status.ready, result: result);
          },
          onError: (Object error) {
            if (!mounted || generation != _generation) return;
            _task = null;
            _preview.value = _Preview(
              source,
              error is MermaidRenderCancelled
                  ? _Status.cancelled
                  : _Status.failed,
              error: 'This diagram could not be rendered. Its source is below.',
            );
          },
        );
      } catch (_) {
        _preview.value = _Preview(
          source,
          _Status.failed,
          error: 'This diagram could not be rendered. Its source is below.',
        );
      }
    });
  }

  void _cancel() {
    ++_generation;
    _task?.cancel();
    _task = null;
    _preview.value = _Preview(widget.source, _Status.cancelled);
  }

  void _retry() {
    _schedule();
    setState(() {}); // Ensure the scheduled frame runs after a button press.
  }

  @override
  void dispose() {
    ++_generation;
    _task?.cancel();
    _preview.dispose();
    super.dispose();
  }

  Widget _pending(BuildContext context, _Preview preview) => Center(
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (preview.status == _Status.loading) ...[
            const SizedBox.square(
              dimension: 20,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(height: 12),
          ],
          Semantics(
            liveRegion: true,
            child: Text(switch (preview.status) {
              _Status.loading => 'Rendering diagram…',
              _Status.cancelled => 'Diagram rendering cancelled.',
              _ => preview.error ?? 'Diagram unavailable.',
            }, style: TextStyle(color: context.ditch.muted)),
          ),
          TextButton(
            onPressed: preview.status == _Status.loading ? _cancel : _retry,
            child: Text(
              preview.status == _Status.loading
                  ? 'Cancel rendering'
                  : 'Retry diagram',
            ),
          ),
        ],
      ),
    ),
  );

  void _expand() {
    showDialog<void>(
      context: context,
      builder: (context) => Dialog(
        insetPadding: const EdgeInsets.all(24),
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            children: [
              Row(
                children: [
                  const Expanded(child: Text('Mermaid diagram')),
                  IconButton(
                    tooltip: 'Close diagram',
                    onPressed: () => Navigator.of(context).pop(),
                    icon: const Icon(Icons.close),
                  ),
                ],
              ),
              Expanded(
                child: ValueListenableBuilder(
                  valueListenable: _preview,
                  builder: (context, preview, _) => preview.result == null
                      ? _pending(context, preview)
                      : ChatMermaidViewport(result: preview.result!),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: _preview,
    builder: (context, value, _) {
      final preview = value.source == widget.source
          ? value
          : _Preview(widget.source, _Status.loading);
      final result = preview.result;
      final buttonStyle = TextButton.styleFrom(
        textStyle: Theme.of(context).textTheme.labelMedium,
      );
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            child: Wrap(
              spacing: 8,
              children: [
                TextButton.icon(
                  style: buttonStyle,
                  onPressed: result == null ? null : _expand,
                  icon: const Icon(Icons.open_in_full, size: 16),
                  label: const Text('Expand diagram'),
                ),
                TextButton.icon(
                  style: buttonStyle,
                  onPressed: () => setState(() => _showSource = !_showSource),
                  icon: const Icon(Icons.code, size: 16),
                  label: Text(_showSource ? 'Hide source' : 'Show source'),
                ),
              ],
            ),
          ),
          // Loading, cancelled and ready states reserve the same viewport.
          // Only the viewport is bounded, never the diagram's logical size.
          if (_showSource)
            ChatCodeSource(source: widget.source)
          else
            SizedBox(
              height: 360,
              width: double.infinity,
              child: result == null
                  ? _pending(context, preview)
                  : Padding(
                      padding: const EdgeInsets.all(12),
                      child: Semantics(
                        image: true,
                        label: result.description,
                        child: FittedBox(
                          fit: BoxFit.contain,
                          child: CustomPaint(
                            size: Size(
                              result.scene.size.width,
                              result.scene.size.height,
                            ),
                            painter: ScenePainter(result.scene),
                          ),
                        ),
                      ),
                    ),
            ),
          if (!_showSource && preview.status == _Status.failed)
            ChatCodeSource(source: widget.source),
        ],
      );
    },
  );
}
