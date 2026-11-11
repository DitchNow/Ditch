import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

const _a = AgentModelOption(
  id: 'qwen:9b',
  displayName: 'Qwen 9B',
  endpointId: 'a',
  endpointName: 'GPU A',
);
const _b = AgentModelOption(
  id: 'qwen:9b',
  displayName: 'Qwen 9B',
  endpointId: 'b',
  endpointName: 'GPU B',
);
const _openAI = AgentModelOption(id: 'gpt-test', displayName: 'GPT Test');
final _search = find.byKey(const ValueKey('model-picker-search'));
final _popover = find.byKey(const ValueKey('model-picker-popover'));

Future<void> mount(
  WidgetTester tester,
  AgentExecutionSettings settings, {
  VoidCallback? onAdd,
  VoidCallback? retry,
  bool dark = true,
}) async {
  await tester.pumpWidget(
    MaterialApp(
      theme: dark ? DitchTheme.dark() : DitchTheme.light(),
      home: Scaffold(
        body: Align(
          alignment: Alignment.bottomLeft,
          child: SizedBox(
            width: 260,
            child: AgentModelPicker(
              settings: settings,
              onAdd: onAdd,
              onRetryOpenAI: retry,
            ),
          ),
        ),
      ),
    ),
  );
  await tester.tap(find.byType(AgentModelPicker));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets(
    'search reveals both groups and distinguishes the same model on two servers',
    (tester) async {
      final settings = AgentExecutionSettings()
        ..setModels([_openAI])
        ..ossModels = [_a, _b];
      await mount(tester, settings);
      expect(find.text('GPU A'), findsNothing); // OSS initially collapsed.
      await tester.enterText(_search, 'qwen');
      await tester.pumpAndSettle();
      expect(find.text('GPU A'), findsOneWidget);
      expect(find.text('GPU B'), findsOneWidget);
      expect(find.text('GPT Test'), findsNothing);
      await tester.enterText(_search, 'qwen:9b gpu b');
      await tester.pumpAndSettle();
      expect(find.text('GPU A'), findsNothing);
      await tester.sendKeyEvent(LogicalKeyboardKey.arrowDown);
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pumpAndSettle();
      expect(_popover, findsNothing);
      expect(settings.model, 'qwen:9b');
      expect(settings.inferenceEndpointId, 'b');
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'clearing search restores accordion state and escape restores trigger focus',
    (tester) async {
      final settings = AgentExecutionSettings()
        ..setModels([_openAI])
        ..ossModels = [_a];
      await mount(tester, settings);
      await tester.tap(find.text('OpenAI models'));
      await tester.tap(find.text('OSS models'));
      await tester.pumpAndSettle();
      expect(find.text('GPT Test'), findsNothing);
      await tester.enterText(_search, 'gpt');
      await tester.pumpAndSettle();
      expect(find.text('GPT Test'), findsOneWidget);
      await tester.tap(find.byTooltip('Clear search'));
      await tester.pumpAndSettle();
      expect(find.text('GPT Test'), findsNothing);
      expect(find.text('GPU A'), findsOneWidget);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pumpAndSettle();
      expect(_popover, findsNothing);
      final trigger = tester.widget<TextButton>(
        find.descendant(
          of: find.byType(AgentModelPicker),
          matching: find.byType(TextButton),
        ),
      );
      expect(trigger.focusNode!.hasFocus, isTrue);
      expect(settings.model, isNull);
    },
  );

  testWidgets(
    'bound agent exposes only its server and preserves missing saved selection',
    (tester) async {
      final settings =
          AgentExecutionSettings(
              profile: {'model': 'unlisted', 'inference_endpoint_id': 'a'},
            )
            ..setModels([_openAI])
            ..ossModels = [_a, _b];
      await mount(tester, settings, onAdd: () {});
      expect(find.text('OpenAI models'), findsNothing);
      expect(find.text('GPU B'), findsNothing);
      expect(find.text('GPU A'), findsOneWidget);
      expect(find.text('Add OSS model…'), findsNothing);
      expect(
        find.text('Saved model is not in the current catalog'),
        findsOneWidget,
      );
      expect(
        find.text('Start a new agent to change inference server.'),
        findsOneWidget,
      );
      await tester.tap(find.byKey(ValueKey('model-option:${_a.selectionKey}')));
      await tester.pumpAndSettle();
      expect(settings.model, 'qwen:9b');
      expect(settings.inferenceEndpointId, 'a');
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'large catalogs keep search and Add fixed in a bounded narrow popover',
    (tester) async {
      tester.view.physicalSize = const Size(320, 600);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.resetPhysicalSize);
      addTearDown(tester.view.resetDevicePixelRatio);
      final settings = AgentExecutionSettings()
        ..ossModels = [
          for (var i = 0; i < 80; i++)
            AgentModelOption(
              id: 'long-model-$i:latest',
              displayName: 'A long model name with a version and size $i',
              endpointId: 'a',
              endpointName: 'A long server name in another region',
            ),
        ];
      settings.selectOption(settings.ossModels.first);
      var adds = 0;
      await mount(tester, settings, onAdd: () => adds++, dark: false);
      final rect = tester.getRect(_popover);
      expect(rect.width, lessThanOrEqualTo(296));
      expect(rect.height, lessThanOrEqualTo(440));
      expect(rect.left, greaterThanOrEqualTo(12));
      expect(rect.top, greaterThanOrEqualTo(12));
      final searchBefore = tester.getRect(_search);
      final addBefore = tester.getRect(find.text('Add OSS model…'));
      await tester.drag(
        find.byType(SingleChildScrollView),
        const Offset(0, -1200),
      );
      await tester.pumpAndSettle();
      expect(tester.getRect(_search), searchBefore);
      expect(tester.getRect(find.text('Add OSS model…')), addBefore);
      await tester.tap(find.text('Add OSS model…'));
      await tester.pumpAndSettle();
      expect(adds, 1);
      expect(_popover, findsNothing);
      expect(tester.takeException(), isNull);
    },
  );

  testWidgets(
    'catalog error is scoped and retry does not discard OSS results',
    (tester) async {
      final settings = AgentExecutionSettings()
        ..ossModels = [_a]
        ..modelError = 'OpenAI catalog unavailable';
      var retries = 0;
      await mount(tester, settings, retry: () => retries++);
      expect(find.text('OpenAI catalog unavailable'), findsOneWidget);
      await tester.tap(find.text('Retry'));
      await tester.tap(find.text('OSS models'));
      await tester.pumpAndSettle();
      expect(retries, 1);
      expect(find.text('GPU A'), findsOneWidget);
      await tester.enterText(_search, 'no-such-model');
      await tester.pumpAndSettle();
      expect(find.text('No matching models'), findsOneWidget);
      await tester.tapAt(const Offset(700, 20));
      await tester.pumpAndSettle();
      expect(_popover, findsNothing);
      expect(settings.model, isNull);
      expect(tester.takeException(), isNull);
    },
  );
}
