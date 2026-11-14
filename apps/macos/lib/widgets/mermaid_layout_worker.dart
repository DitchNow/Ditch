// This file deliberately has no Flutter imports: layout runs in an isolate.
import 'dart:async';
import 'dart:isolate';

import 'package:mermaid_core/mermaid_core.dart' as core;

typedef MermaidTextRequest = ({
  String text,
  core.TextStyleSpec style,
  double? maxWidth,
});

class MermaidRenderResult {
  const MermaidRenderResult(this.scene, this.description);
  final core.RenderScene scene;
  final String description;
}

class MermaidLayoutFailure {
  const MermaidLayoutFailure(this.message);
  final String message;
}

/// Layout is synchronous, but text measurement belongs to Flutter's UI isolate.
/// Discover measurement requests using the library's approximate measurer,
/// then repeat with exact cached metrics. Only a pass with no missing metrics
/// is displayed. The worker can be killed even while Mermaid.render is busy.
void mermaidLayoutWorker((SendPort, String, core.MermaidTheme) input) async {
  final (output, source, theme) = input;
  final replies = ReceivePort();
  final iterator = StreamIterator<dynamic>(replies);
  output.send(replies.sendPort);
  final measurer = _CachedTextMeasurer();
  while (true) {
    measurer.missing.clear();
    core.RenderScene? scene;
    Object? failure;
    try {
      scene = core.Mermaid(measurer: measurer, theme: theme).render(source);
    } catch (error) {
      failure = error;
    }
    if (measurer.missing.isNotEmpty) {
      output.send(measurer.missing.toList(growable: false));
      if (!await iterator.moveNext()) return;
      final response = iterator.current;
      measurer.metrics.addAll(response as Map<MermaidTextRequest, core.Size>);
      continue;
    }
    if (scene == null) {
      Isolate.exit(output, MermaidLayoutFailure('$failure'));
    }
    final size = scene.size;
    if (!size.width.isFinite ||
        !size.height.isFinite ||
        size.width <= 0 ||
        size.height <= 0) {
      Isolate.exit(
        output,
        const MermaidLayoutFailure('Invalid diagram geometry.'),
      );
    }
    final labels = <String>[];
    final pending = scene.nodes.reversed.toList();
    while (pending.isNotEmpty) {
      final node = pending.removeLast();
      if (node is core.SceneText) labels.add(node.text);
      if (node is core.SceneGroup) pending.addAll(node.children.reversed);
    }
    // Transfer the completed scene on exit rather than copying it again for
    // the expanded view. Scene objects contain only pure Dart values.
    Isolate.exit(
      output,
      MermaidRenderResult(scene, 'Mermaid diagram: ${labels.join(', ')}'),
    );
  }
}

class _CachedTextMeasurer implements core.TextMeasurer {
  final metrics = <MermaidTextRequest, core.Size>{};
  final missing = <MermaidTextRequest>{};
  final approximate = const core.ApproximateTextMeasurer();

  @override
  core.Size measure(String text, core.TextStyleSpec style, {double? maxWidth}) {
    final request = (text: text, style: style, maxWidth: maxWidth);
    final cached = metrics[request];
    if (cached != null) return cached;
    missing.add(request);
    return approximate.measure(text, style, maxWidth: maxWidth);
  }
}
