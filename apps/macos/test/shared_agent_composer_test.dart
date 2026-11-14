import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/agent_prompt_drafts.dart';
import 'package:the_ditch/data/prompt_attachment.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/widgets/native_file_drop_region.dart';
import 'package:the_ditch/widgets/prompt_attachments.dart';
import 'tool_integration_test.dart' show DesktopClient;

void main() {
  late Directory directory;
  setUp(
    () => directory = Directory.systemTemp.createTempSync('shared-composer-'),
  );
  tearDown(() => directory.deleteSync(recursive: true));

  Future<void> mountDialog(
    WidgetTester tester,
    AgentPromptDraft draft,
    Future<bool> Function(String) submit, {
    bool remote = false,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () => showDialog<String>(
                context: context,
                builder: (_) => StartCodexSessionDialog(
                  initialPrompt: '',
                  promptDraft: draft,
                  onSubmit: submit,
                  isRemote: remote,
                  settings: AgentExecutionSettings(),
                ),
              ),
              child: const Text('New Agent'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('New Agent'));
    await tester.pumpAndSettle();
  }

  Future<void> drop(WidgetTester tester, List<String> paths) async {
    await tester.runAsync(() async {
      tester
          .widget<NativeFileDropRegion>(find.byType(NativeFileDropRegion))
          .onDrop(paths);
      await Future<void>.delayed(const Duration(milliseconds: 50));
    });
    await tester.pumpAndSettle();
  }

  testWidgets(
    'New Agent retains text and deduplicated attachments on cancel and failure',
    (tester) async {
      final draft = AgentPromptDraft('');
      final file = File('${directory.path}/notes.md')
        ..writeAsStringSync('notes');
      final sent = <String>[];
      await mountDialog(tester, draft, (prompt) async {
        sent.add(prompt);
        return false;
      });
      expect(find.byType(AgentComposer), findsOneWidget);
      await tester.enterText(find.byType(TextField), 'Review this');
      await drop(tester, [file.path, file.path]);
      expect(draft.attachments, hasLength(1));
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('New Agent'));
      await tester.pumpAndSettle();
      expect(find.text('Review this'), findsOneWidget);
      expect(find.byType(PromptAttachments), findsOneWidget);
      tester
          .widget<NativeComposerTextView>(find.byType(NativeComposerTextView))
          .onEnlarge!();
      await tester.pumpAndSettle();
      expect(
        tester.getSize(find.byKey(const Key('composer-editor-shell'))).height,
        160,
      );
      tester
          .widget<NativeComposerTextView>(find.byType(NativeComposerTextView))
          .onEscape!();
      await tester.pumpAndSettle();
      expect(
        tester.getSize(find.byKey(const Key('composer-editor-shell'))).height,
        72,
      );
      expect(draft.text, 'Review this');
      await tester.runAsync(
        () => tester
            .state<AgentComposerState>(find.byType(AgentComposer))
            .submit(),
      );
      await tester.pumpAndSettle();
      expect(find.byType(StartCodexSessionDialog), findsOneWidget);
      expect(
        sent.single,
        serializeAttachedPrompt('Review this', draft.attachments),
      );
      expect(draft.text, 'Review this');
      expect(draft.attachments, hasLength(1));
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'attachment-only startup waits for acceptance and starts only once',
    (tester) async {
      final draft = AgentPromptDraft('');
      final file = File('${directory.path}/only.txt')
        ..writeAsStringSync('data');
      final acknowledgement = Completer<bool>();
      var submissions = 0;
      await mountDialog(tester, draft, (_) {
        submissions++;
        return acknowledgement.future;
      });
      await drop(tester, [file.path]);
      final composer = tester.state<AgentComposerState>(
        find.byType(AgentComposer),
      );
      late Future<void> pending;
      await tester.runAsync(() async {
        pending = composer.submit();
        await Future<void>.delayed(const Duration(milliseconds: 50));
        await composer.submit();
      });
      await tester.pumpAndSettle();
      expect(submissions, 1);
      expect(find.byType(StartCodexSessionDialog), findsOneWidget);
      expect(
        tester
            .widget<TextButton>(find.widgetWithText(TextButton, 'Cancel'))
            .onPressed,
        isNull,
      );
      acknowledgement.complete(true);
      await tester.runAsync(() => pending);
      await tester.pumpAndSettle();
      expect(find.byType(StartCodexSessionDialog), findsNothing);
      expect(draft.attachments, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'file picker and image preview use the same New Agent attachments',
    (tester) async {
      final draft = AgentPromptDraft('');
      final image = File('${directory.path}/image.png')
        ..writeAsStringSync('broken image');
      const channel = MethodChannel('the_ditch/project_picker');
      final calls = <String>[];
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        calls.add(call.method);
        return [image.path];
      });
      addTearDown(
        () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          channel,
          null,
        ),
      );
      await mountDialog(tester, draft, (_) async => false);
      expect(
        tester
            .widget<IconButton>(find.byKey(const Key('composer-attach')))
            .onPressed,
        isNotNull,
      );
      late Future<void> picking;
      await tester.runAsync(() async {
        picking = tester
            .state<AgentComposerState>(find.byType(AgentComposer))
            .chooseFiles();
        await Future<void>.delayed(const Duration(milliseconds: 10));
      });
      await tester.pump();
      await tester.runAsync(() => picking);
      await tester.pumpAndSettle();
      expect(calls, ['chooseFiles']);
      expect(draft.attachments.single.path, image.path);
      await tester.tap(find.text('image.png'));
      await tester.pumpAndSettle();
      expect(find.byType(InteractiveViewer), findsOneWidget);
      expect(
        tester
            .widget<NativeFileDropRegion>(
              find.byType(NativeFileDropRegion, skipOffstage: false),
            )
            .enabled,
        isFalse,
      );
      await tester.tap(find.text('Close'));
      await tester.pumpAndSettle();
      await tester.tap(find.byTooltip('Remove image.png'));
      await tester.pumpAndSettle();
      expect(draft.attachments, isEmpty);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets('accepted startup preserves edits made while waiting', (
    tester,
  ) async {
    final draft = AgentPromptDraft('First task');
    final acknowledgement = Completer<bool>();
    await mountDialog(tester, draft, (_) => acknowledgement.future);
    late Future<void> pending;
    await tester.runAsync(() async {
      pending = tester
          .state<AgentComposerState>(find.byType(AgentComposer))
          .submit();
      await Future<void>.delayed(const Duration(milliseconds: 20));
    });
    await tester.pump();
    await tester.enterText(find.byType(TextField), 'Next task');
    acknowledgement.complete(true);
    await tester.runAsync(() => pending);
    await tester.pumpAndSettle();
    expect(draft.text, 'Next task');
    expect(find.byType(StartCodexSessionDialog), findsNothing);
  });

  testWidgets('New Agent dialog fits a narrow window with attachments', (
    tester,
  ) async {
    await tester.binding.setSurfaceSize(const Size(360, 640));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    final draft = AgentPromptDraft('Inspect this file');
    await mountDialog(tester, draft, (_) async => false);
    final file = File('${directory.path}/notes.md')..writeAsStringSync('notes');
    await drop(tester, [file.path]);
    expect(find.byType(PromptAttachments), findsOneWidget);
    expect(
      tester.getRect(find.byKey(const Key('composer-submit'))).right,
      lessThan(360),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('SSH New Agent rejects local files and disables file picking', (
    tester,
  ) async {
    final draft = AgentPromptDraft('remote task');
    await mountDialog(tester, draft, (_) async => false, remote: true);
    await drop(tester, ['/local/file.txt']);
    expect(draft.attachments, isEmpty);
    expect(find.textContaining('unavailable for SSH projects'), findsOneWidget);
    expect(
      tester
          .widget<IconButton>(find.byKey(const Key('composer-attach')))
          .onPressed,
      isNull,
    );
  });

  for (final width in [360.0, 736.0]) {
    testWidgets('compact composer fits $width and keeps text above actions', (
      tester,
    ) async {
      final capture = GlobalKey();
      final settings = AgentExecutionSettings(
        profile: {
          'approval': 'FullAccess',
          'model': 'qwen3.5:9b',
          'inference_endpoint_id': 'local',
        },
      );
      final client = DesktopClient()..ready = true;

      if (Platform.environment['COMPOSER_SCREENSHOTS'] == 'true') {
        await tester.runAsync(() async {
          final font = FontLoader('Avenir Next')
            ..addFont(
              Future.value(
                ByteData.sublistView(
                  File(
                    Platform.environment['COMPOSER_FONT_PATH'] ??
                        '/System/Library/Fonts/Avenir Next.ttc',
                  ).readAsBytesSync(),
                ),
              ),
            );
          await font.load();
          final icons = FontLoader('MaterialIcons')
            ..addFont(rootBundle.load('fonts/MaterialIcons-Regular.otf'));
          await icons.load();
        });
      }
      await tester.binding.setSurfaceSize(Size(width, 420));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: Align(
              alignment: Alignment.topCenter,
              child: RepaintBoundary(
                key: capture,
                child: AgentComposer(
                  initialText:
                      'Review the layout and keep the writing area clear.',
                  draft: AgentPromptDraft(
                    'Review the layout and keep the writing area clear.',
                  )..visualizer = true,
                  executionSettings: settings,
                  runtimeClient: client,
                  hasSession: true,
                  isWorking: false,
                  onSubmit: (_) async => false,
                  onStop: () {},
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final editor = tester.getRect(
        find.byKey(const Key('composer-editor-shell')),
      );
      final send = tester.getRect(find.byKey(const Key('composer-submit')));
      expect(editor.bottom, lessThanOrEqualTo(send.top));
      expect(editor.width, greaterThan(width - 50));
      expect(send.width, lessThanOrEqualTo(36));
      expect(tester.takeException(), isNull);
      // Opt-in visual QA output, excluded from normal test runs.
      if (Platform.environment['COMPOSER_SCREENSHOTS'] == 'true') {
        await tester.runAsync(() async {
          final image =
              await (capture.currentContext!.findRenderObject()!
                      as RenderRepaintBoundary)
                  .toImage(pixelRatio: 2);
          final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
          File(
            '.tmp/composer-tests/composer-${width.toInt()}.png',
          ).writeAsBytesSync(bytes!.buffer.asUint8List());
          image.dispose();
        });
      }
      await tester.pumpWidget(const SizedBox());
    });
  }
}
