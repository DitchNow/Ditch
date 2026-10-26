import 'dart:async';
import 'dart:convert';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/widgets/assistant_message_content.dart';
import 'package:the_ditch/widgets/assistant_message_image.dart';
import 'package:the_ditch/widgets/chat_visual_block.dart';

Widget host(Widget child, {bool dark = false, double width = 320}) =>
    MaterialApp(
      theme: dark ? DitchTheme.dark() : DitchTheme.light(),
      home: Scaffold(
        body: SingleChildScrollView(
          child: SizedBox(width: width, child: child),
        ),
      ),
    );

String renderedText(WidgetTester tester) => tester
    .widgetList<RichText>(find.byType(RichText))
    .map((text) => text.text.toPlainText())
    .join('\n');

const chartJson =
    '{"title":"Signups","type":"line","labels":["Mon","Tue"],'
    '"series":[{"name":"Users","values":[-2,5]}]}';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  for (final dark in [false, true]) {
    testWidgets(
      'formats GFM with selection in ${dark ? 'dark' : 'light'} theme',
      (tester) async {
        await tester.pumpWidget(
          host(
            const AssistantMessageContent(
              text: '''
# Heading
**Bold** *Italic* ~~Removed~~ and `a | **b**`

1. Ordered
2. Second

- Unordered
- [x] Complete
- [ ] Pending

> Quotation

| Name | Value |
| --- | --- |
| Alpha | 123 |

\\*literal stars\\* and \\| pipe
''',
            ),
            dark: dark,
          ),
        );
        final text = renderedText(tester);
        expect(text, contains('Heading'));
        expect(text, contains('Bold'));
        expect(text, isNot(contains('**Bold**')));
        expect(text, contains('a | **b**'));
        expect(text, contains('*literal stars* and | pipe'));
        expect(find.byType(Table), findsOneWidget);
        expect(find.byType(SelectionArea), findsOneWidget);
        expect(
          tester
              .widgetList<RichText>(find.byType(RichText))
              .where((text) => text.selectionRegistrar != null),
          isNotEmpty,
        );
        final spans = tester
            .widgetList<RichText>(find.byType(RichText))
            .expand((text) => allSpans(text.text));
        expect(
          spans.any(
            (s) => s.text == 'Bold' && s.style?.fontWeight == FontWeight.bold,
          ),
          isTrue,
        );
        expect(
          spans.any(
            (s) => s.text == 'Italic' && s.style?.fontStyle == FontStyle.italic,
          ),
          isTrue,
        );
        expect(
          spans.any(
            (s) =>
                s.text == 'Removed' &&
                s.style?.decoration == TextDecoration.lineThrough,
          ),
          isTrue,
        );
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets('wide tables/code stay horizontal; code copies literal content', (
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
    final source = 'const text = "${'a|' * 100} ** \\ <tag>";';
    await tester.pumpWidget(
      host(
        AssistantMessageContent(
          text:
              '| ${'wide' * 40} | B |\n| --- | --- |\n| one | two |\n\n```unknown-lang\n$source\n```',
        ),
        width: 240,
      ),
    );
    expect(find.text('unknown-lang'), findsOneWidget);
    final horizontal = find.byWidgetPredicate(
      (w) => w is SingleChildScrollView && w.scrollDirection == Axis.horizontal,
    );
    expect(horizontal, findsNWidgets(2));
    await tester.drag(horizontal.first, const Offset(-150, 0));
    await tester.pumpAndSettle();
    await tester.tap(find.byTooltip('Copy code'));
    expect(copied, '$source\n');
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'partial Markdown and unclosed or invalid visuals stay readable',
    (tester) async {
      for (final source in [
        '**unfinished [link](',
        '```ditch-chart\n$chartJson',
        '```ditch-chart\n{"type":\n```',
        '```mermaid\ngraph TD; A-->B\n```',
      ]) {
        await tester.pumpWidget(host(AssistantMessageContent(text: source)));
        expect(find.byType(ChatVisualBlock), findsNothing);
        expect(renderedText(tester), isNotEmpty);
        expect(tester.takeException(), isNull);
      }
      await tester.pumpWidget(
        host(
          const AssistantMessageContent(
            text: '```ditch-chart\n$chartJson\n```',
          ),
        ),
      );
      expect(find.byType(ChatVisualBlock), findsOneWidget);
      expect(find.text('Mon: -2.0; Tue: 5.0'), findsOneWidget);
      expect(find.byTooltip('Copy code'), findsOneWidget);
    },
  );

  testWidgets(
    'unchanged messages retain Markdown widget on unrelated rebuilds',
    (tester) async {
      final rebuild = ValueNotifier(0);
      addTearDown(rebuild.dispose);
      await tester.pumpWidget(
        host(
          ValueListenableBuilder<int>(
            valueListenable: rebuild,
            builder: (_, value, child) =>
                AssistantMessageContent(text: '**same**'),
          ),
        ),
      );
      final before = tester.widget<MarkdownBody>(find.byType(MarkdownBody));
      rebuild.value++;
      await tester.pump();
      expect(
        tester.widget<MarkdownBody>(find.byType(MarkdownBody)),
        same(before),
      );
    },
  );

  testWidgets('formatted text can be selected and copied', (tester) async {
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
    await tester.pumpWidget(
      host(const AssistantMessageContent(text: 'Select **these** words')),
    );
    await tester.longPressAt(
      tester.getTopLeft(find.byType(RichText).first) + const Offset(15, 8),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Copy').last);
    await tester.pumpAndSettle();
    expect(copied, isNotEmpty);
    expect(copied, isNot(contains('**')));
    expect(tester.takeException(), isNull);
  });

  testWidgets('formatted links and linked images remain readable and opt-in', (
    tester,
  ) async {
    var fetches = 0;
    var opens = 0;
    await tester.pumpWidget(
      host(
        AssistantMessageContent(
          text:
              '[**Bold** and `code`](https://example.com)\n\n[![Preview](https://example.com/img.png)](https://example.com/full)',
          openLink: (_) async {
            opens++;
            return true;
          },
          fetchImage: (_) async {
            fetches++;
            throw const FormatException('test');
          },
        ),
        width: 220,
      ),
    );
    expect(find.text('Bold and code'), findsOneWidget);
    expect(find.byType(AssistantMessageImage), findsOneWidget);
    expect(fetches, 0);
    await tester.tap(find.text('Bold and code'));
    await tester.pump();
    expect(opens, 1);
    await tester.tap(find.text('Load image preview'));
    await tester.pumpAndSettle();
    expect(fetches, 1);
    expect(opens, 1);
    expect(tester.takeException(), isNull);
  });

  testWidgets('links show destinations, reject schemes, and report failures', (
    tester,
  ) async {
    final opened = <Uri>[];
    await tester.pumpWidget(
      host(
        AssistantMessageContent(
          text:
              '[Good](https://example.com/path) [Bad](file:///etc/passwd) [Script](javascript:alert) [Command](command:run)',
          openLink: (uri) async {
            opened.add(uri);
            return false;
          },
        ),
      ),
    );
    expect(find.byTooltip('https://example.com/path'), findsOneWidget);
    for (final name in ['Bad', 'Script', 'Command']) {
      await tester.tap(find.text(name));
      await tester.pump();
      expect(find.text('Only HTTPS links can be opened.'), findsOneWidget);
    }
    expect(opened, isEmpty);
    await tester.tap(find.text('Good'));
    await tester.pump();
    expect(opened.single.toString(), 'https://example.com/path');
    expect(find.text('Could not open this link.'), findsOneWidget);
  });

  testWidgets(
    'native link helper validates before invoking the application channel',
    (tester) async {
      final calls = <MethodCall>[];
      const channel = MethodChannel('the_ditch/application');
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        calls.add(call);
        return true;
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      for (final url in [
        'file:///tmp/a',
        'http://example.com',
        'https://u:p@example.com',
        'javascript:alert(1)',
        'https:/missing',
      ]) {
        expect(await openChatLink(Uri.parse(url)), isFalse);
      }
      expect(calls, isEmpty);
      expect(await openChatLink(Uri.parse('https://example.com')), isTrue);
      expect(calls.single.method, 'openURL');
      expect(calls.single.arguments, 'https://example.com');
    },
  );

  testWidgets('raw HTML and local images never fetch or execute', (
    tester,
  ) async {
    var fetches = 0;
    var opens = 0;
    await tester.pumpWidget(
      host(
        AssistantMessageContent(
          text: '''
<script>alert(1)</script>

<img src="https://example.com/tracker.png">

![local](file:///tmp/image.png)

![asset](assets/image.png)

![data](data:image/png;base64,AAAA)
''',
          openLink: (_) async {
            opens++;
            return true;
          },
          fetchImage: (_) async {
            fetches++;
            return Uint8List(0);
          },
        ),
      ),
    );
    expect(fetches, 0);
    expect(opens, 0);
    expect(find.byType(Image), findsNothing);
    expect(find.text('Load image preview'), findsNothing);
    expect(renderedText(tester), contains('<script>'));
    expect(tester.takeException(), isNull);
  });

  testWidgets(
    'images load only after a click, reserve space, and report errors',
    (tester) async {
      var fetches = 0;
      final pending = Completer<Uint8List>();
      await tester.pumpWidget(
        host(
          AssistantMessageContent(
            text: '![Mockup](https://example.com/mockup.png)',
            fetchImage: (_) {
              fetches++;
              return pending.future;
            },
          ),
          width: 220,
        ),
      );
      final preview = find.byType(AssistantMessageImage);
      final before = tester.getSize(preview);
      expect(before.width, lessThanOrEqualTo(220));
      expect(before.height, 240);
      expect(fetches, 0);
      expect(find.text('Mockup'), findsOneWidget);
      await tester.tap(find.text('Load image preview'));
      await tester.pump();
      expect(fetches, 1);
      expect(find.byType(CircularProgressIndicator), findsOneWidget);
      pending.completeError(const FormatException('bad data'));
      await tester.pumpAndSettle();
      expect(find.text('Retry preview'), findsOneWidget);
      expect(tester.getSize(preview), before);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('static image decoding succeeds without changing preview bounds', (
    tester,
  ) async {
    final bytes = base64Decode(
      'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAACklEQVR4nGMAAQAABQABDQottAAAAABJRU5ErkJggg==',
    );
    await tester.pumpWidget(
      host(
        AssistantMessageImage(
          destination: 'https://example.com/test.png',
          alt: 'Tiny mockup',
          fetcher: (_) async => bytes,
        ),
      ),
    );
    final before = tester.getSize(find.byType(AssistantMessageImage));
    await tester.runAsync(() async {
      await tester.tap(find.text('Load image preview'));
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await tester.pumpAndSettle();
    expect(tester.widget<RawImage>(find.byType(RawImage)).image, isNotNull);
    expect(tester.getSize(find.byType(AssistantMessageImage)), before);
    expect(tester.takeException(), isNull);
  });

  test(
    'image decoder rejects non-raster, oversized bytes and dimensions',
    () async {
      for (final bytes in [
        Uint8List.fromList(utf8.encode('<svg/>')),
        Uint8List(chatImageMaxBytes + 1),
      ]) {
        await expectLater(decodeChatImage(bytes), throwsFormatException);
      }
      final recorder = ui.PictureRecorder();
      Canvas(recorder);
      final picture = recorder.endRecording();
      final image = await picture.toImage(4097, 1);
      final data = await image.toByteData(format: ui.ImageByteFormat.png);
      await expectLater(
        decodeChatImage(data!.buffer.asUint8List()),
        throwsFormatException,
      );
      image.dispose();
      picture.dispose();
    },
  );
}

Iterable<TextSpan> allSpans(InlineSpan span) sync* {
  if (span is TextSpan) {
    yield span;
    for (final child in span.children ?? <InlineSpan>[]) {
      yield* allSpans(child);
    }
  }
}
