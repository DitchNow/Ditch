import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mermaid_core/mermaid_core.dart' as core;
import 'package:mermaid_flutter/mermaid_flutter.dart';
import 'package:the_ditch/widgets/mermaid_render_task.dart';

import 'fixtures/large_mermaid.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  test('worker uses exact Flutter text metrics', () async {
    const source =
        'flowchart LR\n A["A long label with some words"] --> B["日本語"]';
    const theme = core.MermaidTheme.defaultTheme;
    final task = startMermaidRender(source, theme);
    addTearDown(task.cancel);
    final result = await task.result;
    final expected = const core.Mermaid(
      measurer: FlutterTextMeasurer(),
      theme: theme,
    ).render(source);
    expect(result.scene.size.width, expected.size.width);
    expect(result.scene.size.height, expected.size.height);
    expect(result.description, contains('日本語'));
  });

  test(
    'large nested graph exceeds all former caps and leaves event loop live',
    () async {
      final source = largeMermaid();
      expect(source.length, greaterThan(8192));
      expect(source.split('\n').length, greaterThan(120));
      var ticks = 0;
      final timer = Timer.periodic(
        const Duration(milliseconds: 10),
        (_) => ticks++,
      );
      addTearDown(timer.cancel);
      final memoryBefore = ProcessInfo.currentRss;
      final watch = Stopwatch()..start();
      final task = startMermaidRender(source, core.MermaidTheme.darkTheme);
      addTearDown(task.cancel);
      final result = await task.result.timeout(const Duration(seconds: 60));
      watch.stop();
      expect(result.scene.size.height, greaterThan(8192));
      expect(result.description, contains('Service 239'));
      expect(ticks, greaterThan(1));
      // Diagnostic evidence, not a brittle timing/RSS assertion across machines.
      debugPrint(
        'Large Mermaid: ${source.length} chars, ${watch.elapsedMilliseconds} ms, '
        '${result.scene.size.width} x ${result.scene.size.height}, '
        '$ticks UI timer ticks, RSS delta ${ProcessInfo.currentRss - memoryBefore} bytes',
      );
    },
  );

  test(
    'active and queued cancellation release slots for the next render',
    () async {
      final jobs = [
        for (var i = 0; i < 3; i++)
          startMermaidRender(
            largeMermaid(nodes: 500),
            core.MermaidTheme.defaultTheme,
          ),
      ];
      final cancelled = [
        for (final job in jobs)
          expectLater(job.result, throwsA(isA<MermaidRenderCancelled>())),
      ];
      // Give the workers time to start so cancellation also covers busy layout.
      await Future<void>.delayed(const Duration(milliseconds: 50));
      for (final job in jobs) {
        job.cancel();
      }
      await Future.wait(cancelled);
      final next = startMermaidRender(
        'graph LR\n A --> B',
        core.MermaidTheme.defaultTheme,
      );
      addTearDown(next.cancel);
      expect((await next.result).description, contains('A'));
    },
  );

  test(
    'syntax failure releases its worker and remains an explicit error',
    () async {
      final job = startMermaidRender(
        'not a diagram',
        core.MermaidTheme.defaultTheme,
      );
      addTearDown(job.cancel);
      await expectLater(job.result, throwsA(isA<FormatException>()));
      final next = startMermaidRender(
        'graph LR\n A --> B',
        core.MermaidTheme.defaultTheme,
      );
      addTearDown(next.cancel);
      expect((await next.result).scene.size.width, greaterThan(0));
    },
  );
}
