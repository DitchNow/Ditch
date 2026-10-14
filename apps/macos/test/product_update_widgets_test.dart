import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/product_update_controller.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/widgets/update_available_dot.dart';

import 'product_update_controller_test.dart' as fixtures;

const application = MethodChannel('the_ditch/application');

class UpdateClient extends DitchRuntimeClient {
  UpdateClient() : super(socketPath: '/tmp/update-widget-test');

  @override
  Future<Map<String, dynamic>> commercialEntitlement() async => {
    'active': true,
    'plan': 'commercial_monthly',
    'status': 'active',
  };

  @override
  runtimeStatus() async => fixtures.context().runtime;
}

void main() {
  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          application,
          (call) async => switch (call.method) {
            'appVersion' => {'version': '0.1.0', 'build': '112'},
            'getPaneWidths' => <String, double>{},
            _ => null,
          },
        );
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(application, null);
  });

  testWidgets(
    'both dots appear while Settings is open and dismissal stays separate',
    (tester) async {
      var available = true;
      final updates = ProductUpdateController(
        loadContext: () async => fixtures.context(),
        fetchRelease: (_, {required forInstallation}) async {
          if (!available) {
            throw const DitchRuntimeException(
              'commercial_release_unavailable',
              'none',
            );
          }
          return fixtures.release();
        },
      );
      addTearDown(updates.dispose);
      await tester.pumpWidget(
        MaterialApp(
          theme: DitchTheme.light(),
          home: CommandCenterScreen(
            updates: updates,
            runtimeClient: UpdateClient(),
            connectRuntimeOnStart: false,
            initialProjects: const [
              DitchProject(name: 'Test', path: '/tmp/test'),
            ],
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('toolbar-update-dot')), findsNothing);
      await tester.tap(find.byKey(const Key('app-settings-button')));
      await tester.pumpAndSettle();
      expect(find.byKey(const Key('settings-update-dot')), findsNothing);
      await updates.check();
      await tester.pump();
      expect(find.byKey(const Key('toolbar-update-dot')), findsOneWidget);
      expect(find.byKey(const Key('settings-update-dot')), findsOneWidget);

      // The dialog displays the already discovered release without another click.
      await tester.tap(find.byKey(const Key('settings-upgrade-ditch')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.text('Ditch 0.1.0.113 is available.'), findsOneWidget);
      expect(find.byKey(const Key('install-ditch-update')), findsOneWidget);
      await tester.tap(find.text('Close'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      await tester.tap(find.byKey(const Key('notification-bell')));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      await tester.tap(find.byTooltip('Dismiss notification'));
      await tester.pump();
      expect(find.byKey(const Key('toolbar-update-dot')), findsOneWidget);
      await updates.check(force: true);
      await tester.pump();
      final toolbar = tester.widget<DitchToolbar>(find.byType(DitchToolbar));
      expect(
        toolbar.notifications.where(
          (e) => e.action == AttentionAction.installProductUpdate,
        ),
        isEmpty,
      );
      expect(toolbar.updateAvailable, isTrue);

      available = false;
      await updates.check(force: true);
      await tester.pump();
      expect(find.byKey(const Key('toolbar-update-dot')), findsNothing);
      expect(updates.available, isFalse);
      await tester.pumpWidget(const SizedBox.shrink());
    },
  );

  testWidgets('dot pulses orange and becomes steady with reduced motion', (
    tester,
  ) async {
    Widget app(bool reduced) => MaterialApp(
      theme: DitchTheme.light(),
      home: MediaQuery(
        data: MediaQueryData(disableAnimations: reduced),
        child: const Center(child: UpdateAvailableDot()),
      ),
    );
    await tester.pumpWidget(app(false));
    final fade = find.descendant(
      of: find.byType(UpdateAvailableDot),
      matching: find.byType(FadeTransition),
    );
    final before = tester.widget<FadeTransition>(fade).opacity.value;
    await tester.pump(const Duration(milliseconds: 450));
    expect(tester.widget<FadeTransition>(fade).opacity.value, isNot(before));
    final dot = tester.widget<Container>(
      find.descendant(
        of: find.byType(UpdateAvailableDot),
        matching: find.byType(Container),
      ),
    );
    expect((dot.decoration! as BoxDecoration).color, DitchTokens.light.accent);
    await tester.pumpWidget(app(true));
    await tester.pumpAndSettle();
    expect(tester.widget<FadeTransition>(fade).opacity.value, 1);
    expect(find.bySemanticsLabel('Update available'), findsWidgets);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
