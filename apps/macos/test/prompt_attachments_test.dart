import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/agent_prompt_drafts.dart';
import 'package:the_ditch/data/prompt_attachment.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/widgets/native_file_drop_region.dart';
import 'package:the_ditch/widgets/prompt_attachments.dart';

class DelayedAttachment extends PromptAttachment {
  DelayedAttachment(super.path, this.readable);
  final Completer<bool> readable;
  @override
  Future<bool> get isReadable => readable.future;
}

void main() {
  late Directory directory;
  setUp(
    () => directory = Directory.systemTemp.createTempSync('ditch-attachments-'),
  );
  tearDown(() => directory.deleteSync(recursive: true));

  File file(String name) =>
      File('${directory.path}/$name')..writeAsStringSync('contents');

  test(
    'classifies images and serializes paths without shell interpretation',
    () {
      final paths = [
        'space and ü.JPG',
        'a.png',
        'a.webp',
        'a.jpeg',
        'a.pdf',
        'a.docx',
        'a.md',
        'a.zip',
        'quote"\n\$(cmd).txt',
      ];
      final attachments = paths
          .map((name) => PromptAttachment(file(name).path))
          .toList();
      expect(
        attachments.take(4).map((a) => a.previewKind),
        everyElement(AttachmentPreviewKind.image),
      );
      expect(
        attachments.skip(4).map((a) => a.previewKind),
        everyElement(AttachmentPreviewKind.file),
      );
      final prompt = serializeAttachedPrompt(' Inspect ', attachments);
      expect(prompt, startsWith('Inspect\n\nIncluded local files'));
      expect(
        jsonDecode(prompt.substring(prompt.indexOf('["'))),
        attachments.map((a) => a.path).toList(),
      );
      expect(serializeAttachedPrompt(' plain ', []), 'plain');
    },
  );

  Future<void> mount(
    WidgetTester tester,
    AgentPromptDraft draft,
    List<String> sent, {
    bool enabled = true,
    bool working = false,
    bool stopping = false,
    bool remote = false,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: Scaffold(
          body: AgentComposer(
            initialText: '',
            draft: draft,
            hasSession: true,
            enabled: enabled,
            isWorking: working,
            canStop: stopping,
            isRemote: remote,
            onSubmit: sent.add,
            onStop: () {},
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  Future<void> drop(WidgetTester tester, List<String> paths) async {
    await tester.runAsync(() async {
      tester
          .widget<NativeFileDropRegion>(find.byType(NativeFileDropRegion))
          .onDrop(paths);
      await Future<void>.delayed(const Duration(milliseconds: 100));
    });
    await tester.pumpAndSettle();
  }

  Future<void> send(WidgetTester tester) async {
    await tester.runAsync(
      () =>
          tester.state<AgentComposerState>(find.byType(AgentComposer)).submit(),
    );
    await tester.pumpAndSettle();
  }

  testWidgets(
    'multiple drops deduplicate normalized paths, retain valid files and send once',
    (tester) async {
      final draft = AgentPromptDraft('');
      final sent = <String>[];
      final a = file('a.md'), b = file('b.pdf');
      await mount(tester, draft, sent);
      await drop(tester, [
        a.path,
        '${directory.path}/./a.md',
        directory.path,
        '${directory.path}/missing',
        b.path,
      ]);
      expect(draft.attachments.map((a) => a.path), [a.path, b.path]);
      expect(
        find.textContaining('Choose readable regular files'),
        findsOneWidget,
      );
      expect(
        tester
            .widget<FilledButton>(
              find.ancestor(
                of: find.text('Send'),
                matching: find.byWidgetPredicate(
                  (widget) => widget is FilledButton,
                ),
              ),
            )
            .onPressed,
        isNotNull,
      );
      await send(tester);
      expect(sent, [
        serializeAttachedPrompt('', [
          PromptAttachment(a.path),
          PromptAttachment(b.path),
        ]),
      ]);
      expect(draft.attachments, isEmpty);
      expect(draft.text, isEmpty);
    },
  );

  testWidgets('missing file retains text and attachments until removed', (
    tester,
  ) async {
    final draft = AgentPromptDraft('keep this');
    final sent = <String>[];
    final a = file('a.md');
    await mount(tester, draft, sent);
    await drop(tester, [a.path]);
    a.deleteSync();
    await send(tester);
    expect(sent, isEmpty);
    expect(draft.text, 'keep this');
    expect(draft.attachments, hasLength(1));
    expect(
      find.textContaining('Restore the file or remove it'),
      findsOneWidget,
    );
    await tester.tap(find.byTooltip('Remove a.md'));
    await tester.pumpAndSettle();
    expect(draft.attachments, isEmpty);
    await send(tester);
    expect(sent, ['keep this']);
  });

  testWidgets(
    'working allows preparation; disabled, stopping and SSH guard sending',
    (tester) async {
      final draft = AgentPromptDraft('');
      final sent = <String>[];
      final a = file('a.md');
      await mount(tester, draft, sent, enabled: false);
      await drop(tester, [a.path]);
      expect(draft.attachments, isEmpty);
      await mount(tester, draft, sent, remote: true);
      await drop(tester, [a.path]);
      expect(draft.attachments, isEmpty);
      expect(
        find.textContaining('unavailable for SSH projects'),
        findsOneWidget,
      );
      await mount(tester, draft, sent, working: true);
      await drop(tester, [a.path]);
      await send(tester);
      expect(draft.attachments, hasLength(1));
      expect(sent, isEmpty);
      await mount(tester, draft, sent, stopping: true);
      await send(tester);
      expect(sent, isEmpty);
      await mount(tester, draft, sent, remote: true);
      await send(tester);
      expect(sent, isEmpty);
      await mount(tester, draft, sent);
      await send(tester);
      expect(sent, hasLength(1));
    },
  );

  testWidgets('corrupt images have an icon fallback', (tester) async {
    final attachment = PromptAttachment(file('broken.png').path);
    await tester.runAsync(() async {
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: PromptAttachments(
              attachments: [attachment],
              onRemove: (_) {},
            ),
          ),
        ),
      );
      await Future<void>.delayed(const Duration(milliseconds: 100));
      await tester.pump();
    });
    expect(find.byIcon(Icons.insert_drive_file_outlined), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('conversation changes invalidate pending submit work', (
    tester,
  ) async {
    final first = AgentPromptDraft('first');
    final second = AgentPromptDraft('second');
    final sent = <String>[];
    final readable = Completer<bool>();
    first.attachments.add(DelayedAttachment('/delayed', readable));
    await mount(tester, first, sent);
    final pending = tester
        .state<AgentComposerState>(find.byType(AgentComposer))
        .submit();
    await tester.pump();
    await mount(tester, second, sent);
    readable.complete(true);
    await pending;
    expect(sent, isEmpty);
    expect(first.text, 'first');
    expect(second.text, 'second');
  });
}
