import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/application/product_update_controller.dart';
import 'package:the_ditch/data/commercial_models.dart';
import 'package:the_ditch/data/runtime_models.dart';
import 'package:the_ditch/data/runtime_transport.dart';

ProductUpdateContext context({
  int installed = 112,
  String edition = 'commercial',
  String environment = 'production',
}) => (
  runtime: RuntimeStatusDto(
    identity: 'Ditch',
    pid: 1,
    socketPath: '/tmp/test',
    activeSessionCount: 0,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'runtime',
    capabilities: {},
    edition: edition,
    deploymentEnvironment: environment,
    releaseSequence: installed,
  ),
  license: DitchCurrentLicense(
    edition: edition,
    status: 'active',
    displayName: edition,
    plans: const [],
  ),
);

Map<String, dynamic> release({int sequence = 113, String id = 'new'}) => {
  'manifest': {
    'release_id': id,
    'release_sequence': sequence,
    'version': '0.1.0',
    'build': '$sequence',
  },
  'update_session': {'bearer': 'temporary-download-authorization'},
};

void main() {
  for (final environment in ['staging', 'production']) {
    for (final edition in ['community', 'commercial']) {
      test(
        '$environment $edition only advertises a newer installed sequence',
        () async {
          var sequence = 113;
          final updates = ProductUpdateController(
            loadContext: () async =>
                context(edition: edition, environment: environment),
            fetchRelease: (license, {required forInstallation}) async {
              expect(license.edition, edition);
              expect(forInstallation, isFalse);
              return release(sequence: sequence);
            },
          );
          addTearDown(updates.dispose);
          await updates.check();
          expect(updates.available, isTrue);
          expect(updates.manifest!.containsKey('update_session'), isFalse);
          sequence = 112;
          await updates.check(force: true);
          expect(updates.available, isFalse);
          sequence = 111;
          await updates.check(force: true);
          expect(updates.available, isFalse);
        },
      );
    }
  }

  test('joins concurrent checks and refreshes only when stale', () async {
    var now = DateTime(2026);
    var calls = 0;
    final pending = Completer<Map<String, dynamic>>();
    final updates = ProductUpdateController(
      now: () => now,
      loadContext: () async => context(),
      fetchRelease: (_, {required forInstallation}) {
        calls++;
        return pending.future;
      },
    );
    addTearDown(updates.dispose);
    final first = updates.check();
    final second = updates.check(force: true);
    pending.complete(release());
    await Future.wait([first, second]);
    expect(calls, 1);
    await updates.check();
    expect(calls, 1);
    now = now.add(const Duration(minutes: 6));
    await updates.check();
    expect(calls, 2);
  });

  test(
    'offline failures retain availability; release_unavailable clears it',
    () async {
      Object? failure;
      final updates = ProductUpdateController(
        loadContext: () async => context(),
        fetchRelease: (_, {required forInstallation}) async {
          if (failure != null) throw failure;
          return release();
        },
      );
      addTearDown(updates.dispose);
      await updates.check();
      failure = const DitchRuntimeException(
        'commercial_release_network_failed',
        'offline',
      );
      await updates.check(force: true);
      expect(updates.available, isTrue);
      expect(updates.error, same(failure));
      failure = const DitchRuntimeException(
        'commercial_release_unavailable',
        'none',
      );
      await updates.check(force: true);
      expect(updates.available, isFalse);
      expect(updates.error, isNull);
      expect(updates.noCompatibleRelease, isTrue);
    },
  );

  test('invalid metadata cannot light a dot', () async {
    final updates = ProductUpdateController(
      loadContext: () async => context(),
      fetchRelease: (_, {required forInstallation}) async => {
        'manifest': {'release_sequence': 114},
      },
    );
    addTearDown(updates.dispose);
    await updates.check();
    expect(updates.available, isFalse);
    expect(updates.error, isA<FormatException>());
  });

  test(
    'license change clears previous availability even when discovery fails',
    () async {
      var current = context();
      final updates = ProductUpdateController(
        loadContext: () async => current,
        fetchRelease: (license, {required forInstallation}) async {
          if (!license.hasCommercialAccess) throw StateError('offline');
          return release();
        },
      );
      addTearDown(updates.dispose);
      await updates.check();
      current = context(edition: 'community');
      await updates.check(force: true);
      expect(updates.available, isFalse);
      expect(updates.context!.license.edition, 'community');
    },
  );

  test('invalidated in-flight result cannot overwrite a new check', () async {
    final old = Completer<Map<String, dynamic>>();
    final requested = Completer<void>();
    var calls = 0;
    final updates = ProductUpdateController(
      loadContext: () async => context(),
      fetchRelease: (_, {required forInstallation}) async {
        if (++calls == 1) {
          requested.complete();
          return old.future;
        }
        return release(id: 'replacement', sequence: 114);
      },
    );
    addTearDown(updates.dispose);
    final first = updates.check();
    await requested.future;
    updates.invalidate();
    await updates.check();
    old.complete(release());
    await first;
    expect(updates.releaseId, 'replacement');
    expect(updates.checking, isFalse);
  });

  test(
    'install obtains fresh permission and joins simultaneous install checks',
    () async {
      final routes = <bool>[];
      final updates = ProductUpdateController(
        loadContext: () async => context(),
        fetchRelease: (_, {required forInstallation}) async {
          routes.add(forInstallation);
          return {
            ...release(),
            'update_session': {'bearer': 'permission-${routes.length}'},
          };
        },
      );
      addTearDown(updates.dispose);
      await updates.check();
      final results = await Future.wait([
        updates.prepareInstallation(),
        updates.prepareInstallation(),
      ]);
      expect(routes, [false, true]);
      expect(results.first!['update_session']['bearer'], 'permission-2');
      expect(updates.available, isTrue);
    },
  );

  test('disposal ignores a pending response', () async {
    final pending = Completer<Map<String, dynamic>>();
    final requested = Completer<void>();
    final updates = ProductUpdateController(
      loadContext: () async => context(),
      fetchRelease: (_, {required forInstallation}) {
        requested.complete();
        return pending.future;
      },
    );
    final check = updates.check();
    await requested.future;
    updates.dispose();
    pending.complete(release());
    await check;
    expect(updates.available, isFalse);
  });

  testWidgets('polling backs off on repeated failures and stops on disposal', (
    tester,
  ) async {
    var calls = 0;
    var failing = true;
    final updates = ProductUpdateController(
      now: tester.binding.clock.now,
      pollInterval: const Duration(seconds: 10),
      staleAfter: Duration.zero,
      loadContext: () async => context(),
      fetchRelease: (_, {required forInstallation}) async {
        calls++;
        if (failing) throw StateError('offline');
        return release();
      },
    );
    updates.startPolling();
    await tester.pump();
    expect(calls, 1);
    await tester.pump(const Duration(seconds: 10));
    expect(calls, 2);
    await tester.pump(const Duration(seconds: 10));
    expect(calls, 2);
    failing = false;
    await tester.pump(const Duration(seconds: 10));
    expect(calls, 3);
    await tester.pump(const Duration(seconds: 10));
    expect(calls, 4);
    updates.dispose();
    await tester.pump(const Duration(minutes: 5));
    expect(calls, 4);
  });
}
