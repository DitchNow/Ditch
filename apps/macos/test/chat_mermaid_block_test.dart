import 'dart:async';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mermaid_flutter/mermaid_flutter.dart';
import 'package:mermaid_core/mermaid_core.dart' as core;
import 'package:the_ditch/widgets/chat_mermaid_viewport.dart';
import 'package:the_ditch/widgets/mermaid_render_task.dart';
import 'fixtures/large_mermaid.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/widgets/assistant_message_content.dart';
import 'package:the_ditch/widgets/chat_code_source.dart';
import 'package:the_ditch/widgets/chat_mermaid_block.dart';

const flow = 'flowchart LR\n  A[Plan] --> B{Review}\n  B --> C[Ship]\n';

Future<void> settleMermaid(WidgetTester tester) async {
  for (var i = 0; i < 600; i++) {
    await tester.pump(const Duration(milliseconds: 20));
    if (find.text('Rendering diagram…').evaluate().isEmpty) return;
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 10)),
    );
  }
  fail('Mermaid rendering did not finish');
}

class ControlledRenderTask implements MermaidRenderTask {
  final completion = Completer<MermaidRenderResult>();
  bool cancelled = false;
  @override
  Future<MermaidRenderResult> get result => completion.future;
  @override
  void cancel() {
    cancelled = true;
  }
}

MermaidRenderResult testScene(String label) => MermaidRenderResult(
  core.RenderScene(size: const core.Size(140, 100), nodes: const []),
  label,
);

Widget host(String source, {bool dark = false, double width = 320}) =>
    MaterialApp(
      themeAnimationDuration: Duration.zero,
      theme: dark ? DitchTheme.dark() : DitchTheme.light(),
      home: Scaffold(
        body: SingleChildScrollView(
          child: SizedBox(
            width: width,
            child: AssistantMessageContent(text: '```mermaid\n$source```'),
          ),
        ),
      ),
    );

Finder get scenePaint => find.byWidgetPredicate(
  (widget) => widget is CustomPaint && widget.painter is ScenePainter,
);

void main() {
  test('Markdown copy preserves backticks with a valid enclosing fence', () {
    expect(
      mermaidMarkdown('graph LR\n A["```"]'),
      '````mermaid\ngraph LR\n A["```"]\n````\n',
    );
  });

  testWidgets(
    'large generated diagram renders and expansion reuses the scene',
    (tester) async {
      await tester.pumpWidget(host(largeMermaid(), width: 240));
      await settleMermaid(tester);
      expect(scenePaint, findsOneWidget);
      final scene =
          (tester.widget<CustomPaint>(scenePaint).painter as ScenePainter)
              .scene;
      expect(scene.size.height, greaterThan(8192));
      await tester.tap(find.text('Expand diagram'));
      await tester.pumpAndSettle();
      final viewport = tester.widget<ChatMermaidViewport>(
        find.byType(ChatMermaidViewport),
      );
      expect(identical(scene, viewport.result.scene), isTrue);
      final viewer = tester.widget<InteractiveViewer>(
        find.byType(InteractiveViewer),
      );
      final matrix = viewer.transformationController!.value;
      final fit = matrix.getMaxScaleOnAxis();
      expect(fit, lessThan(0.1));
      expect(matrix.entry(0, 0), closeTo(fit, 0.000001));
      expect(matrix.entry(1, 1), closeTo(fit, 0.000001));
      await tester.tap(find.byTooltip('Zoom in'));
      await tester.pump();
      expect(
        viewer.transformationController!.value.getMaxScaleOnAxis(),
        closeTo(fit * 1.5, 0.000001),
      );
      await tester.drag(find.byType(InteractiveViewer), const Offset(0, -100));
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'cancel, retry and late results preserve viewport and latest source',
    (tester) async {
      final tasks = <ControlledRenderTask>[];
      MermaidRenderTask render(String _, core.MermaidTheme _) {
        final task = ControlledRenderTask();
        tasks.add(task);
        return task;
      }

      Widget controlled(String source) => MaterialApp(
        home: Scaffold(
          body: SizedBox(
            width: 320,
            child: ChatMermaidBlock(source: source, renderer: render),
          ),
        ),
      );
      await tester.pumpWidget(controlled(flow));
      await tester.pump();
      final loadingSize = tester.getSize(find.byType(ChatMermaidBlock));
      await tester.tap(find.text('Cancel rendering'));
      await tester.pump();
      expect(tasks.first.cancelled, isTrue);
      expect(find.text('Diagram rendering cancelled.'), findsOneWidget);
      await tester.tap(find.text('Retry diagram'));
      await tester.pump();
      expect(tasks, hasLength(2));
      await tester.pumpWidget(controlled('graph LR\n X --> Y'));
      await tester.pump();
      expect(tasks[1].cancelled, isTrue);
      tasks.first.completion.complete(testScene('stale cancelled'));
      tasks[1].completion.complete(testScene('stale previous source'));
      tasks.last.completion.complete(testScene('current source'));
      await tester.pump();
      expect(find.bySemanticsLabel('current source'), findsOneWidget);
      expect(find.bySemanticsLabel('stale previous source'), findsNothing);
      expect(tester.getSize(find.byType(ChatMermaidBlock)), loadingSize);
      await tester.pumpWidget(controlled('graph LR\n Z --> Y'));
      await tester.pumpWidget(const SizedBox());
      expect(tasks.last.cancelled, isTrue);
      tasks.last.completion.complete(testScene('disposed'));
      await tester.pump();
      expect(tester.takeException(), isNull);
    },
  );

  for (final dark in [false, true]) {
    for (final sample in <String, String>{
      'flowchart': flow,
      'sequence': 'sequenceDiagram\n  Alice->>Bob: Hello\n  Bob-->>Alice: Hi\n',
      'state': 'stateDiagram-v2\n  [*] --> Ready\n  Ready --> Finished\n',
      'class': 'classDiagram\n  Animal <|-- Duck\n',
      'ER': 'erDiagram\n  CUSTOMER ||--o{ ORDER : places\n',
      'pie': 'pie title Traffic\n  "Direct" : 40\n  "Search" : 60\n',
      'XY': 'xychart-beta\n  x-axis [Mon, Tue, Wed]\n  bar [12, 18, 15]\n',
    }.entries) {
      testWidgets('${sample.key} renders in ${dark ? 'dark' : 'light'} theme', (
        tester,
      ) async {
        final semantics = tester.ensureSemantics();
        await tester.pumpWidget(host(sample.value, dark: dark, width: 220));
        await settleMermaid(tester);
        expect(scenePaint, findsOneWidget);
        expect(find.byType(ChatCodeSource), findsNothing);
        expect(
          find.bySemanticsLabel(RegExp('Mermaid diagram:')),
          findsOneWidget,
        );
        expect(
          tester.getSize(find.byType(FittedBox)).width,
          lessThanOrEqualTo(220),
        );
        expect(
          tester.getSize(find.byType(FittedBox)).height,
          lessThanOrEqualTo(360),
        );
        expect(tester.takeException(), isNull);
        semantics.dispose();
      });
    }
  }

  testWidgets('theme changes repaint; unchanged rebuilds reuse the scene', (
    tester,
  ) async {
    await tester.pumpWidget(host(flow));
    await settleMermaid(tester);
    final light =
        (tester.widget<CustomPaint>(scenePaint).painter as ScenePainter).scene;
    await tester.pumpWidget(host(flow));
    await settleMermaid(tester);
    final same =
        (tester.widget<CustomPaint>(scenePaint).painter as ScenePainter).scene;
    expect(identical(light, same), isTrue);
    await tester.pumpWidget(host(flow, dark: true));
    await settleMermaid(tester);
    await tester.pumpAndSettle();
    final dark =
        (tester.widget<CustomPaint>(scenePaint).painter as ScenePainter).scene;
    expect(identical(light, dark), isFalse);
    expect(light.background, isNot(dark.background));
  });

  testWidgets('source and copy preserve Mermaid; expanded view can close', (
    tester,
  ) async {
    String? copied;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied = (call.arguments as Map)['text'] as String;
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.pumpWidget(host(flow));
    await settleMermaid(tester);
    await tester.tap(find.byTooltip('Copy as Markdown'));
    expect(copied, mermaidMarkdown(flow));
    final originalScene =
        (tester.widget<CustomPaint>(scenePaint).painter as ScenePainter).scene;
    await tester.tap(find.text('Show source'));
    await tester.pump();
    expect(find.text(flow.trimRight()), findsOneWidget);
    expect(scenePaint, findsNothing);
    await tester.tap(find.text('Hide source'));
    await tester.pump();
    expect(find.byType(ChatCodeSource), findsNothing);
    expect(scenePaint, findsOneWidget);
    expect(
      identical(
        (tester.widget<CustomPaint>(scenePaint).painter as ScenePainter).scene,
        originalScene,
      ),
      isTrue,
    );
    await tester.tap(find.text('Expand diagram'));
    await tester.pumpAndSettle();
    expect(find.byType(ChatMermaidViewport), findsOneWidget);
    final viewer = tester.widget<InteractiveViewer>(
      find.byType(InteractiveViewer),
    );
    final fitScale = viewer.transformationController!.value.getMaxScaleOnAxis();
    final expandedScene = tester
        .widget<ChatMermaidViewport>(find.byType(ChatMermaidViewport))
        .result
        .scene;
    final painted = tester.widget<CustomPaint>(
      find.descendant(
        of: find.byType(ChatMermaidViewport),
        matching: scenePaint,
      ),
    );
    expect(
      identical((painted.painter as ScenePainter).scene, expandedScene),
      isTrue,
    );
    await tester.tap(find.byTooltip('Zoom in'));
    await tester.pump();
    expect(
      viewer.transformationController!.value.getMaxScaleOnAxis(),
      greaterThan(fitScale),
    );
    await tester.tap(find.text('Fit diagram'));
    await tester.pump();
    expect(
      viewer.transformationController!.value.getMaxScaleOnAxis(),
      closeTo(fitScale, 0.00001),
    );
    await tester.tap(find.byTooltip('Close diagram'));
    await tester.pumpAndSettle();
    expect(find.byType(ChatMermaidViewport), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('invalid and configurable diagrams retain source', (
    tester,
  ) async {
    for (final source in [
      'not-a-diagram\n',
      'flowchart LR\n A[unfinished\n',
      '%%{init: {"theme":"dark"}}%%\n$flow',
      '---\nconfig:\n  theme: dark\n---\n$flow',
    ]) {
      await tester.pumpWidget(host(source));
      await settleMermaid(tester);
      expect(find.byType(ChatCodeSource), findsOneWidget);
      expect(find.text(source.trimRight()), findsOneWidget);
      expect(scenePaint, findsNothing);
      expect(find.byTooltip('Copy as Markdown'), findsOneWidget);
      expect(tester.takeException(), isNull);
    }
  });

  testWidgets(
    'streaming fence stays code until closed, then becomes a diagram',
    (tester) async {
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: AssistantMessageContent(text: '```mermaid\n$flow'),
          ),
        ),
      );
      expect(find.byType(ChatMermaidBlock), findsNothing);
      expect(find.byType(ChatCodeSource), findsOneWidget);
      await tester.pumpWidget(host(flow));
      await settleMermaid(tester);
      await settleMermaid(tester);
      expect(scenePaint, findsOneWidget);
    },
  );

  testWidgets('embedded links are inert and wheel scrolls the transcript', (
    tester,
  ) async {
    final controller = ScrollController();
    addTearDown(controller.dispose);
    var linkCalls = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.dark(),
        home: Scaffold(
          body: SingleChildScrollView(
            controller: controller,
            child: Column(
              children: [
                SizedBox(
                  width: 360,
                  child: AssistantMessageContent(
                    text:
                        '```mermaid\n${flow}click A "javascript:alert(1)"\n```',
                    openLink: (_) async {
                      linkCalls++;
                      return true;
                    },
                    revealFile: (_) async {
                      linkCalls++;
                    },
                  ),
                ),
                const SizedBox(height: 1000),
              ],
            ),
          ),
        ),
      ),
    );
    await settleMermaid(tester);
    expect(scenePaint, findsOneWidget);
    final position = tester.getCenter(scenePaint);
    await tester.tapAt(position);
    expect(linkCalls, 0);
    await tester.sendEventToBinding(
      PointerScrollEvent(position: position, scrollDelta: const Offset(0, 150)),
    );
    await tester.pump();
    expect(controller.offset, greaterThan(0));
    expect(tester.takeException(), isNull);
  });
}
