import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/chat_links.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/widgets/assistant_message_content.dart';
import 'package:the_ditch/widgets/chat_link_scope.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('the_ditch/application');

  test(
    'resolves local paths, encoded names and line references without a cwd fallback',
    () {
      final examples = <String, String>{
        'file:///Users/me/My%20Project/caf%C3%A9.dart':
            '/Users/me/My Project/café.dart',
        'file://localhost/Users/me/file.dart': '/Users/me/file.dart',
        '/Users/me/file.dart#L12C3': '/Users/me/file.dart',
        'src/file.dart#L12-L20': '/project/src/file.dart',
        './src/../README.md': '/project/README.md',
        '../shared/file.dart': '/shared/file.dart',
        'notes/a%23b%25c.txt': '/project/notes/a#b%c.txt',
        'notes/%2520.txt': '/project/notes/%20.txt',
        '/Users/me/My Project/file.dart': '/Users/me/My Project/file.dart',
        'README.md:12': '/project/README.md:12',
        'src/file.dart:12:3': '/project/src/file.dart:12:3',
      };
      for (final entry in examples.entries) {
        final target =
            resolveChatLink(entry.key, projectPath: '/project') as ChatFileLink;
        expect(target.path, entry.value, reason: entry.key);
      }
      expect(
        (resolveChatLink('README.md:12', projectPath: '/project')
                as ChatFileLink)
            .withoutLineSuffix,
        '/project/README.md',
      );
      expect(resolveChatLink('src/file.dart'), isA<UnsupportedChatLink>());
      expect(
        resolveChatLink('src/file.dart', projectPath: 'relative'),
        isA<UnsupportedChatLink>(),
      );
    },
  );

  test(
    'rejects executable schemes, nonlocal authorities and invalid destinations',
    () {
      for (final value in [
        '',
        'javascript:alert(1)',
        'command:run',
        'data:text/html,hi',
        'http://example.com',
        'vscode://file/tmp/a',
        'file://server/tmp/a',
        'file://user@localhost/tmp/a',
        'file://localhost:80/tmp/a',
        'file:relative',
        '//server/share/a',
        'https://user:pass@example.com',
        'https:/missing',
        '#heading',
        '/tmp/a?query=yes',
        '/tmp/a#unknown',
        '/tmp/a%00',
        '/tmp/a\n',
        'file:///%2Fserver/share/a',
      ]) {
        expect(
          resolveChatLink(value, projectPath: '/project'),
          isA<UnsupportedChatLink>(),
          reason: value,
        );
      }
      expect(
        resolveChatLink('https://example.com/a?q=x#section'),
        isA<ChatWebLink>(),
      );
    },
  );

  test('remote projects never resolve paths against the local machine', () {
    for (final value in ['file:///tmp/a', '/tmp/a', 'src/a.dart']) {
      final target =
          resolveChatLink(
                value,
                projectPath: '/remote',
                isRemote: true,
                remoteHost: 'build-host',
              )
              as UnsupportedChatLink;
      expect(target.message, contains('build-host'));
    }
    expect(
      resolveChatLink('https://example.com', isRemote: true),
      isA<ChatWebLink>(),
    );
  });

  group('native file dispatch', () {
    late Directory fixture;
    late List<MethodCall> calls;
    setUp(() async {
      fixture = await Directory.systemTemp.createTemp('ditch-finder-test-');
      calls = [];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            calls.add(call);
            return true;
          });
    });
    tearDown(() async {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, null);
      await fixture.delete(recursive: true);
    });

    test(
      'selects an existing exact filename before interpreting a line suffix',
      () async {
        final ordinary = await File(
          '${fixture.path}/note.txt',
        ).writeAsString('fixture');
        final withColon = await File(
          '${fixture.path}/note.txt:12:3',
        ).writeAsString('fixture');
        final link = resolveChatLink(withColon.path) as ChatFileLink;
        await revealChatFile(link);
        expect(calls.single.method, 'revealFileInFinder');
        expect(calls.single.arguments, withColon.path);
        await withColon.delete();
        await revealChatFile(link);
        expect(calls.last.arguments, ordinary.path);
        expect(
          calls.every(
            (call) => call.method != 'openURL' && call.method != 'openFile',
          ),
          isTrue,
        );
      },
    );

    test('missing or invalid paths never reach the native channel', () async {
      for (final path in [
        '${fixture.path}/missing',
        'relative.txt',
        '//server/a',
        '/tmp/a\x00',
      ]) {
        await expectLater(
          revealChatFile(ChatFileLink(path)),
          throwsA(isA<ChatLinkFailure>()),
        );
      }
      expect(calls, isEmpty);
    });

    test('reports native refusal after the file existence check', () async {
      final file = await File(
        '${fixture.path}/file.txt',
      ).writeAsString('fixture');
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (_) async => false);
      await expectLater(
        revealChatFile(ChatFileLink(file.path)),
        throwsA(isA<ChatLinkFailure>()),
      );
    });

    testWidgets('clicking a project file reaches the native Finder channel', (
      tester,
    ) async {
      final file = File('${fixture.path}/My notes.txt')
        ..writeAsStringSync('fixture');
      final dispatched = Completer<void>();
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
        call,
      ) async {
        calls.add(call);
        dispatched.complete();
        return true;
      });
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: ChatLinkScope(
              projectPath: fixture.path,
              child: AgentChatBubble(
                message: AgentChatMessage(
                  role: ChatMessageRole.assistant,
                  text: '[File](My%20notes.txt#L12)',
                  createdAt: DateTime(2026),
                ),
              ),
            ),
          ),
        ),
      );
      expect(
        find.byTooltip('Show file in Finder\n${file.path}'),
        findsOneWidget,
      );
      await tester.tap(find.text('File'));
      for (var i = 0; i < 10 && !dispatched.isCompleted; i++) {
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 20)),
        );
        await tester.pump();
      }
      await tester.pumpAndSettle();
      expect(
        calls,
        hasLength(1),
        reason: tester
            .widgetList<Text>(find.byType(Text))
            .map((w) => w.data)
            .join('\n'),
      );
      expect(calls.single.method, 'revealFileInFinder');
      expect(calls.single.arguments, file.path);
      expect(find.textContaining('Could not'), findsNothing);
    });
  });

  testWidgets('file and HTTPS clicks use distinct injected actions', (
    tester,
  ) async {
    final revealed = <ChatFileLink>[];
    final opened = <Uri>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AssistantMessageContent(
            text: '[File](src/main.dart#L12) [Web](https://example.com)',
            projectPath: '/project',
            revealFile: (file) async => revealed.add(file),
            openLink: (uri) async {
              opened.add(uri);
              return true;
            },
          ),
        ),
      ),
    );
    expect(
      find.byTooltip('Show file in Finder\n/project/src/main.dart'),
      findsOneWidget,
    );
    await tester.tap(find.text('File'));
    await tester.pump();
    expect(revealed.single.path, '/project/src/main.dart');
    expect(opened, isEmpty);
    await tester.tap(find.text('Web'));
    await tester.pump();
    expect(opened.single.toString(), 'https://example.com');
    expect(revealed.length, 1);
  });

  testWidgets(
    'project scope reaches bubbles and updates cached relative-link tooltips',
    (tester) async {
      Widget host(String root, {bool remote = false}) => MaterialApp(
        home: Scaffold(
          body: ChatLinkScope(
            projectPath: root,
            isRemote: remote,
            remoteHost: 'build-host',
            child: AgentChatBubble(
              message: AgentChatMessage(
                role: ChatMessageRole.assistant,
                text: '[File](README.md)',
                createdAt: DateTime(2026),
              ),
            ),
          ),
        ),
      );
      await tester.pumpWidget(host('/first'));
      expect(
        find.byTooltip('Show file in Finder\n/first/README.md'),
        findsOneWidget,
      );
      await tester.pumpWidget(host('/second'));
      expect(
        find.byTooltip('Show file in Finder\n/second/README.md'),
        findsOneWidget,
      );
      expect(
        find.byTooltip('Show file in Finder\n/first/README.md'),
        findsNothing,
      );
      await tester.pumpWidget(host('/remote', remote: true));
      await tester.tap(find.text('File'));
      await tester.pump();
      expect(
        find.textContaining('This file lives on build-host'),
        findsOneWidget,
      );
    },
  );

  testWidgets('file failures and missing context have visible feedback', (
    tester,
  ) async {
    Future<void> host({String? root, ChatFileRevealer? reveal}) =>
        tester.pumpWidget(
          MaterialApp(
            home: Scaffold(
              body: AssistantMessageContent(
                text: '[File](README.md)',
                projectPath: root,
                revealFile: reveal ?? revealChatFile,
              ),
            ),
          ),
        );
    await host();
    await tester.tap(find.text('File'));
    await tester.pump();
    expect(
      find.textContaining('project folder is unavailable'),
      findsOneWidget,
    );
    await host(
      root: '/project',
      reveal: (_) async {
        throw const ChatLinkFailure('File not found: /project/README.md');
      },
    );
    await tester.tap(find.text('File'));
    await tester.pump();
    expect(find.text('File not found: /project/README.md'), findsOneWidget);
    await host(
      root: '/project',
      reveal: (_) async {
        throw MissingPluginException();
      },
    );
    await tester.tap(find.text('File'));
    await tester.pump();
    expect(find.text('Finder integration is unavailable.'), findsOneWidget);
  });
}
