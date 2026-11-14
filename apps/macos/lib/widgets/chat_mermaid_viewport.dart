import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:mermaid_flutter/mermaid_flutter.dart';

import '../design_system/ditch_theme.dart';
import 'mermaid_render_task.dart';

/// A viewport over the already laid-out scene. Expanding, fitting and zooming
/// never invoke the Mermaid parser or layout engine again.
class ChatMermaidViewport extends StatefulWidget {
  const ChatMermaidViewport({required this.result, super.key});
  final MermaidRenderResult result;

  @override
  State<ChatMermaidViewport> createState() => _ChatMermaidViewportState();
}

class _ChatMermaidViewportState extends State<ChatMermaidViewport> {
  final _transform = TransformationController();
  Size? _viewport;
  double _fitScale = 1;

  @override
  void didUpdateWidget(ChatMermaidViewport oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!identical(oldWidget.result.scene, widget.result.scene)) {
      _viewport = null;
    }
  }

  void _fit() {
    final viewport = _viewport;
    if (viewport == null) return;
    final scene = widget.result.scene;
    _transform.value = Matrix4.identity()
      ..translateByDouble(
        (viewport.width - scene.size.width * _fitScale) / 2,
        (viewport.height - scene.size.height * _fitScale) / 2,
        0,
        1,
      )
      ..scaleByDouble(_fitScale, _fitScale, _fitScale, 1);
  }

  void _zoom(double factor) {
    final viewport = _viewport;
    if (viewport == null) return;
    final currentScale = _transform.value.getMaxScaleOnAxis();
    final nextScale = (currentScale * factor)
        .clamp(_fitScale * 0.1, math.max(8, _fitScale * 8))
        .toDouble();
    final center = viewport.center(Offset.zero);
    final sceneCenter = _transform.toScene(center);
    _transform.value = Matrix4.identity()
      ..translateByDouble(
        center.dx - sceneCenter.dx * nextScale,
        center.dy - sceneCenter.dy * nextScale,
        0,
        1,
      )
      ..scaleByDouble(nextScale, nextScale, nextScale, 1);
  }

  @override
  void dispose() {
    _transform.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Column(
    children: [
      Row(
        mainAxisAlignment: MainAxisAlignment.end,
        children: [
          IconButton(
            tooltip: 'Zoom out',
            onPressed: () => _zoom(1 / 1.5),
            icon: const Icon(Icons.remove),
          ),
          IconButton(
            tooltip: 'Zoom in',
            onPressed: () => _zoom(1.5),
            icon: const Icon(Icons.add),
          ),
          TextButton.icon(
            style: TextButton.styleFrom(
              textStyle: Theme.of(context).textTheme.labelMedium,
            ),
            onPressed: _fit,
            icon: const Icon(Icons.fit_screen),
            label: const Text('Fit diagram'),
          ),
        ],
      ),
      Expanded(
        child: LayoutBuilder(
          builder: (context, constraints) {
            final size = constraints.biggest;
            final scene = widget.result.scene;
            if (_viewport != size) {
              _viewport = size;
              _fitScale =
                  math.min(
                    size.width / scene.size.width,
                    size.height / scene.size.height,
                  ) *
                  0.95;
              WidgetsBinding.instance.addPostFrameCallback((_) {
                if (mounted) _fit();
              });
            }
            return ColoredBox(
              color: context.ditch.surfaceSoft,
              child: Semantics(
                image: true,
                label: widget.result.description,
                child: InteractiveViewer(
                  transformationController: _transform,
                  constrained: false,
                  alignment: Alignment.topLeft,
                  boundaryMargin: const EdgeInsets.all(double.infinity),
                  minScale: _fitScale * 0.1,
                  maxScale: math.max(8, _fitScale * 8),
                  child: RepaintBoundary(
                    child: CustomPaint(
                      size: Size(scene.size.width, scene.size.height),
                      painter: ScenePainter(scene),
                    ),
                  ),
                ),
              ),
            );
          },
        ),
      ),
    ],
  );
}
