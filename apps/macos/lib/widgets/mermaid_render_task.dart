import 'dart:async';
import 'dart:collection';
import 'dart:isolate';

import 'package:mermaid_core/mermaid_core.dart' as core;
import 'package:mermaid_flutter/mermaid_flutter.dart';

import 'mermaid_layout_worker.dart';
export 'mermaid_layout_worker.dart' show MermaidRenderResult;

abstract interface class MermaidRenderTask {
  Future<MermaidRenderResult> get result;
  void cancel();
}

class MermaidRenderCancelled implements Exception {
  const MermaidRenderCancelled();
}

typedef MermaidRenderer =
    MermaidRenderTask Function(String source, core.MermaidTheme theme);

MermaidRenderTask startMermaidRender(String source, core.MermaidTheme theme) =>
    _IsolateRenderTask(source, theme);

/// Bound concurrent CPU work, not diagram size. Queued jobs can be cancelled
/// before spawning; active jobs terminate their isolate immediately.
class _IsolateRenderTask implements MermaidRenderTask {
  _IsolateRenderTask(this.source, this.theme) {
    _queue.add(this);
    _drain();
  }

  static final _queue = Queue<_IsolateRenderTask>();
  static final _active = <_IsolateRenderTask>{};
  static void _drain() {
    while (_active.length < 2 && _queue.isNotEmpty) {
      final job = _queue.removeFirst();
      _active.add(job);
      unawaited(job._start());
    }
  }

  final String source;
  final core.MermaidTheme theme;
  final _completion = Completer<MermaidRenderResult>();
  final _messages = ReceivePort();
  Isolate? _isolate;
  SendPort? _worker;
  bool _finished = false;

  @override
  Future<MermaidRenderResult> get result => _completion.future;

  Future<void> _start() async {
    _messages.listen((message) {
      if (_finished) return;
      if (message is SendPort) {
        _worker = message;
      } else if (message is List<MermaidTextRequest>) {
        unawaited(_measure(message));
      } else if (message is MermaidRenderResult) {
        _finish();
        _completion.complete(message);
      } else if (message is MermaidLayoutFailure) {
        _fail(FormatException(message.message));
      } else {
        // onError sends a two-element list; onExit sends null. Neither should
        // leave a spinner or occupy a scheduler slot indefinitely.
        _fail(StateError('Diagram layout stopped unexpectedly.'));
      }
    });
    try {
      final isolate = await Isolate.spawn(
        mermaidLayoutWorker,
        (_messages.sendPort, source, theme),
        paused: true,
        onError: _messages.sendPort,
        onExit: _messages.sendPort,
        debugName: 'chat-mermaid-layout',
      );
      if (_finished) {
        isolate.kill(priority: Isolate.immediate);
      } else {
        _isolate = isolate;
        isolate.resume(isolate.pauseCapability!);
      }
    } catch (error) {
      _fail(error);
    }
  }

  Future<void> _measure(List<MermaidTextRequest> requests) async {
    final metrics = <MermaidTextRequest, core.Size>{};
    final slice = Stopwatch()..start();
    try {
      for (final request in requests) {
        if (_finished) return;
        metrics[request] = const FlutterTextMeasurer().measure(
          request.text,
          request.style,
          maxWidth: request.maxWidth,
        );
        // TextPainter is UI-only. Yield between short slices so input,
        // cancellation and frame callbacks continue on large diagrams.
        if (slice.elapsedMilliseconds >= 4) {
          await Future<void>.delayed(Duration.zero);
          slice.reset();
        }
      }
      if (!_finished) _worker!.send(metrics);
    } catch (error) {
      _fail(error);
    }
  }

  void _finish() {
    _finished = true;
    _isolate?.kill(priority: Isolate.immediate);
    _messages.close();
    _queue.remove(this);
    _active.remove(this);
    _drain();
  }

  void _fail(Object error) {
    if (_finished) return;
    _finish();
    _completion.completeError(error);
  }

  @override
  void cancel() => _fail(const MermaidRenderCancelled());
}
