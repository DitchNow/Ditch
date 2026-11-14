import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/main.dart';

void main() {
  for (final scenario in [
    (name: 'local OSS', oss: true, remote: false),
    (name: 'OpenAI', oss: false, remote: false),
    (name: 'remote OSS', oss: true, remote: true),
  ]) {
    testWidgets('Full Access confirmation is compact for ${scenario.name}', (
      tester,
    ) async {
      await tester.binding.setSurfaceSize(const Size(2000, 1000));
      addTearDown(() => tester.binding.setSurfaceSize(null));
      final settings = AgentExecutionSettings(
        profile: {
          'approval': 'Ask',
          if (scenario.oss) 'inference_endpoint_id': 'endpoint',
        },
      );
      addTearDown(settings.dispose);
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: Scaffold(
            body: AgentExecutionControls(
              settings: settings,
              isRemote: scenario.remote,
            ),
          ),
        ),
      );

      Future<void> openConfirmation() async {
        await tester.tap(find.byType(DropdownButton<AgentApprovalPreset>));
        await tester.pumpAndSettle();
        await tester.tap(find.text('Full Access').last);
        await tester.pumpAndSettle();
      }

      await openConfirmation();
      final surface = find
          .descendant(of: find.byType(Dialog), matching: find.byType(Material))
          .first;
      expect(tester.getSize(surface).width, lessThanOrEqualTo(480));
      expect(tester.getSize(surface).height, lessThan(400));
      expect(settings.approval, AgentApprovalPreset.ask);
      expect(
        find.textContaining('Observations are sent to your selected model'),
        scenario.oss && !scenario.remote ? findsOneWidget : findsNothing,
      );
      await tester.tap(find.text('Cancel'));
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      expect(settings.approval, AgentApprovalPreset.ask);

      await openConfirmation();
      // The warning remains usable when the window is resized while open.
      await tester.binding.setSurfaceSize(const Size(400, 320));
      await tester.pumpAndSettle();
      expect(tester.getSize(surface).width, lessThanOrEqualTo(352));
      expect(tester.takeException(), isNull);
      final confirm = find.widgetWithText(FilledButton, 'Enable Full Access');
      expect(confirm.hitTestable(), findsOneWidget);
      await tester.tap(confirm);
      await tester.pumpAndSettle();
      expect(find.byType(AlertDialog), findsNothing);
      expect(settings.approval, AgentApprovalPreset.fullAccess);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });
  }
}
