import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/github_inbox.dart';

void main() {
  testWidgets(
    'credential coexistence blocker disables Connect and stays visible',
    (tester) async {
      final calls = <Object>[];
      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: GitHubSettingsDialog(
              request: (request) async {
                calls.add(request);
                return {
                  'GitHub': {
                    'installed': true,
                    'connected': false,
                    'connection_state': 'unsupported_coexistence',
                    'detail':
                        'GitHub connection is unavailable: shared Keychain entries.',
                  },
                };
              },
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.textContaining('shared Keychain entries'), findsOneWidget);
      final button = tester.widget<FilledButton>(
        find.widgetWithText(FilledButton, 'Connect GitHub'),
      );
      expect(button.onPressed, isNull);
      expect(calls, [
        {'GitHub': 'Status'},
      ]);
      expect(tester.takeException(), isNull);
    },
  );
  test('GitHub errors retain their useful runtime message', () async {
    await expectLater(
      githubRequest(
        (_) async => {
          'Error': {'message': 'Repository access unavailable'},
        },
        'Status',
      ),
      throwsA(
        isA<StateError>().having(
          (e) => e.message,
          'message',
          contains('Repository access unavailable'),
        ),
      ),
    );
  });
}
