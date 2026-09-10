import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/main.dart';
import 'package:the_ditch/application/command_center_controller.dart';
import 'package:the_ditch/data/runtime_models.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';

final _testAgentHeaderKeys = <String, GlobalKey>{};

GlobalKey _testAgentHeaderKey(String agentId) =>
    _testAgentHeaderKeys.putIfAbsent(agentId, GlobalKey.new);

const _testProject = DitchProject(
  name: 'Ditch',
  path: '/tmp/the-ditch-test-project',
);

class _PendingRemoteSetupClient extends DitchRuntimeClient {
  _PendingRemoteSetupClient()
    : super(socketPath: '/tmp/ditch-pending-remote-setup.sock');

  final pending = Completer<Map<String, dynamic>>();

  @override
  Future<Map<String, dynamic>> checkRemoteSetup({
    required String alias,
    String? password,
    bool rememberPassword = false,
    bool trustUnknownHost = false,
  }) => pending.future;
}

class _CurrentLicenseClient extends DitchRuntimeClient {
  _CurrentLicenseClient({required this.entitlement})
    : super(socketPath: '/tmp/ditch-current-license.sock');

  final Map<String, dynamic> entitlement;
  int communityChecks = 0;

  @override
  Future<Map<String, dynamic>> checkCommunityRelease() async {
    communityChecks++;
    return {
      'manifest': {
        'edition': 'community',
        'release_sequence': 108,
        'version': '0.1.0',
        'build': '108',
      },
    };
  }

  @override
  Future<RuntimeStatusDto> runtimeStatus() async => const RuntimeStatusDto(
    identity: 'The Ditch Runtime',
    pid: 123,
    socketPath: '/tmp/ditch-current-license.sock',
    activeSessionCount: 0,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'test-runtime',
    capabilities: {},
    buildVersion: '0.1.0',
    edition: 'community',
    deploymentEnvironment: 'staging',
    buildIdentifier: 'test-build',
    buildNumber: '107',
    releaseSequence: 107,
  );

  @override
  Future<Map<String, dynamic>> commercialEntitlement() async => entitlement;
}

class _CommercialOffersClient extends DitchRuntimeClient {
  _CommercialOffersClient({
    this.initiallyActive = false,
    this.status,
    this.catalog,
    this.installedEdition = 'community',
    this.environment = 'staging',
    this.activeAgents = 0,
  }) : super(socketPath: '/tmp/ditch-commercial-offers.sock');

  final String installedEdition;
  final String environment;
  final int activeAgents;
  int activationCalls = 0;
  bool failActivation = false;

  @override
  Future<RuntimeStatusDto> runtimeStatus() async => RuntimeStatusDto(
    identity: 'The Ditch Runtime',
    pid: 123,
    socketPath: '/tmp/ditch-license.sock',
    activeSessionCount: activeAgents,
    attentionCount: 0,
    unreadAttentionCount: 0,
    instanceId: 'license-test',
    capabilities: {},
    buildVersion: '0.1.0',
    edition: installedEdition,
    deploymentEnvironment: environment,
    buildIdentifier: 'test-build',
    buildNumber: '112',
    releaseSequence: 112,
  );

  @override
  Future<Map<String, dynamic>> activateCommercialDevice() async {
    activationCalls++;
    if (failActivation) {
      throw const DitchRuntimeException(
        'mac_slot_unavailable',
        'No Mac slot is available.',
      );
    }
    return commercialEntitlement();
  }

  final bool initiallyActive;
  final String? status;
  final CommercialOfferCatalog? catalog;

  @override
  Future<CommercialOfferCatalog> commercialOffers() async =>
      catalog ?? _testCatalog();

  @override
  Future<Map<String, dynamic>> commercialEntitlement() async {
    final entitlementStatus =
        status ?? (initiallyActive ? 'active' : 'inactive');
    return {
      'active': initiallyActive,
      'plan': initiallyActive || entitlementStatus == 'expired'
          ? 'commercial_monthly'
          : null,
      'status': entitlementStatus,
      'expires_at': null,
      'mac_slots': initiallyActive ? 1 : 0,
      'iphone_slots': initiallyActive ? 1 : 0,
      'billing_management_available': initiallyActive,
    };
  }

  @override
  Future<Map<String, dynamic>> commercialBillingManagement() async => {
    'billing_management_url':
        'https://billing.example.test/ditch/customer-session',
  };
}

class _CatalogFailureClient extends _CommercialOffersClient {
  @override
  Future<CommercialOfferCatalog> commercialOffers() async {
    throw StateError('fixture catalog unavailable');
  }
}

class _RecoveringCatalogClient extends _CommercialOffersClient {
  int attempts = 0;

  @override
  Future<CommercialOfferCatalog> commercialOffers() async {
    attempts += 1;
    if (attempts == 1) throw StateError('fixture catalog unavailable');
    return _testCatalog();
  }
}

class _RelayUpgradeClient extends _CommercialOffersClient {
  _RelayUpgradeClient({
    this.entitlementActivates = true,
    this.failCheckout = false,
    super.initiallyActive = false,
    super.installedEdition,
    super.environment,
    super.activeAgents,
  }) : _active = initiallyActive;

  final bool entitlementActivates;
  final bool failCheckout;
  bool _active;
  bool _checkoutCreated = false;
  final selectedOffers = <String>[];
  String? redeemedLicense;
  int commercialReleaseCalls = 0;

  @override
  Future<CommercialOfferCatalog> commercialOffers() async {
    if (_checkoutCreated && !_active && !entitlementActivates) {
      return _testCatalog(
        offers: [
          _testOffer(
            id: 'offer_monthly_standard',
            kind: 'commercial_monthly',
            title: 'Commercial Monthly',
            amountMinor: '1500',
            billingType: 'recurring',
            interval: 'month',
            macSlots: 1,
            iPhoneSlots: 1,
            eligible: false,
            ineligibleReason: 'checkout_in_progress',
          ),
          _testOffer(
            id: 'offer_lifetime',
            kind: 'commercial_lifetime',
            title: 'Commercial Lifetime',
            amountMinor: '25000',
            billingType: 'one_time',
            macSlots: 2,
            iPhoneSlots: 2,
          ),
        ],
      );
    }
    return super.commercialOffers();
  }

  @override
  Future<Map<String, dynamic>> createCommercialCheckout(String offerId) async {
    selectedOffers.add(offerId);
    if (failCheckout) throw StateError('Ditch checkout was not created.');
    _checkoutCreated = true;
    return {
      'id': '00000000-0000-4000-8000-000000000001',
      'hosted_url': 'https://payments.example.test/ditch/$offerId',
      'expires_at': DateTime.now()
          .toUtc()
          .add(const Duration(minutes: 35))
          .toIso8601String(),
    };
  }

  @override
  Future<Map<String, dynamic>> commercialEntitlement() async {
    if (_checkoutCreated && entitlementActivates) _active = true;
    return {
      'active': _active,
      'plan': _active ? 'commercial_monthly' : null,
      'status': _active ? 'active' : 'inactive',
      'expires_at': null,
      'mac_slots': _active ? 1 : 0,
      'iphone_slots': _active ? 1 : 0,
      'billing_management_available': _active,
    };
  }

  @override
  Future<Map<String, dynamic>> redeemCommercialLicense(
    String licenseKey,
  ) async {
    redeemedLicense = licenseKey;
    _active = true;
    return commercialEntitlement();
  }

  @override
  Future<Map<String, dynamic>> commercialBillingManagement() async => {
    'billing_management_url':
        'https://billing.example.test/ditch/customer-session',
  };

  @override
  Future<Map<String, dynamic>> currentCommercialRelease() async {
    commercialReleaseCalls++;
    return {
      'manifest': {
        'edition': 'commercial',
        'release_id': '11111111-1111-4111-8111-111111111111',
        'appcast_url':
            'https://relay.ditchnow.nl/v1/commercial/releases/11111111-1111-4111-8111-111111111111/appcast',
        'artifact_url':
            'https://relay.ditchnow.nl/v1/commercial/releases/11111111-1111-4111-8111-111111111111/artifact/ditch.dmg',
        'artifact_size': 4096,
        'version': '1.2.3',
        'build': '123',
        'channel': 'stable',
      },
      'signature': 'test-signature',
      'update_session': {
        'bearer': 'a-secure-test-bearer-with-at-least-32-characters',
        'expires_at': DateTime.now()
            .toUtc()
            .add(const Duration(minutes: 5))
            .toIso8601String(),
      },
    };
  }
}

class _ReleasedCheckoutClient extends _RelayUpgradeClient {
  _ReleasedCheckoutClient() : super(entitlementActivates: false);

  var _postCheckoutCatalogCalls = 0;

  @override
  Future<CommercialOfferCatalog> commercialOffers() async {
    if (!_checkoutCreated) return super.commercialOffers();
    _postCheckoutCatalogCalls += 1;
    if (_postCheckoutCatalogCalls == 1) return super.commercialOffers();
    return _testCatalog();
  }
}

class _DeferredCommercialReleaseClient extends _RelayUpgradeClient {
  _DeferredCommercialReleaseClient() : super(initiallyActive: true);

  var releaseRequests = 0;

  @override
  Future<Map<String, dynamic>> currentCommercialRelease() async {
    releaseRequests += 1;
    if (releaseRequests == 1) {
      throw const DitchRuntimeException(
        'commercial_upgrade_deferred',
        'Commercial installation is waiting for 1 active agent to finish. Ditch will not interrupt running work.',
      );
    }
    return super.currentCommercialRelease();
  }
}

class _UnavailableCommercialReleaseClient extends _RelayUpgradeClient {
  _UnavailableCommercialReleaseClient() : super(initiallyActive: true);

  @override
  Future<Map<String, dynamic>> currentCommercialRelease() async {
    throw const DitchRuntimeException(
      'commercial_release_unavailable',
      'Ditch Relay rejected the request (release_unavailable)',
    );
  }
}

CommercialOfferCatalog _testCatalog({
  bool stale = false,
  List<Map<String, dynamic>>? offers,
}) {
  if (offers == null && !stale) {
    return CommercialOfferCatalog.fromJson(_canonicalCatalogJson());
  }
  return CommercialOfferCatalog.fromJson({
    'protocol_version': 1,
    'stale': stale,
    'refreshed_at': '2026-08-30T08:00:00Z',
    'offers':
        offers ??
        [
          _testOffer(
            id: 'offer_monthly_standard',
            kind: 'commercial_monthly',
            title: 'Commercial Monthly',
            amountMinor: '1500',
            billingType: 'recurring',
            interval: 'month',
            macSlots: 1,
            iPhoneSlots: 1,
          ),
          _testOffer(
            id: 'offer_monthly_intro',
            kind: 'commercial_monthly',
            title: 'Commercial Monthly · Introductory offer',
            amountMinor: '1500',
            billingType: 'recurring',
            interval: 'month',
            macSlots: 1,
            iPhoneSlots: 1,
            introductory: const {
              'amount_minor': '750',
              'duration_count': 3,
              'duration_unit': 'month',
            },
          ),
          _testOffer(
            id: 'offer_lifetime',
            kind: 'commercial_lifetime',
            title: 'Commercial Lifetime',
            amountMinor: '25000',
            billingType: 'one_time',
            macSlots: 2,
            iPhoneSlots: 2,
          ),
        ],
  });
}

Map<String, dynamic> _canonicalCatalogJson() =>
    (jsonDecode(
              File(
                '../../docs/contracts/commercial-offers-v1.json',
              ).readAsStringSync(),
            )
            as Map)
        .cast<String, dynamic>();

Map<String, dynamic> _testOffer({
  required String id,
  required String kind,
  required String title,
  required String amountMinor,
  required String billingType,
  required int macSlots,
  required int iPhoneSlots,
  String? interval,
  Map<String, dynamic>? introductory,
  String purchaseAction = 'acquire',
  bool eligible = true,
  String? ineligibleReason,
}) => {
  'offer_id': id,
  'kind': kind,
  'title': title,
  'description': 'Ditch Commercial Remote Control',
  'currency': 'EUR',
  'base_amount_minor': amountMinor,
  'minor_unit_exponent': 2,
  'billing_type': billingType,
  'recurring_interval': interval,
  'recurring_interval_count': interval == null ? null : 1,
  'introductory_price': introductory,
  'purchase_action': purchaseAction,
  'eligible': eligible,
  'ineligible_reason': eligible ? null : (ineligibleReason ?? 'not_eligible'),
  'entitlement': {
    'mac_slots': macSlots,
    'iphone_slots': iPhoneSlots,
    'ssh_hosts_unlimited': true,
  },
};

class _RelayContractClient extends DitchRuntimeClient {
  _RelayContractClient() : super(socketPath: '/tmp/ditch-relay-contract.sock');

  Object? lastRequest;

  @override
  Future<Map<String, dynamic>> request(Object body) async {
    lastRequest = body;
    return {
      'CommercialCheckout': {
        'id': '00000000-0000-4000-8000-000000000002',
        'hosted_url': 'https://payments.example.test/ditch/add-on',
        'expires_at': DateTime.now()
            .toUtc()
            .add(const Duration(minutes: 35))
            .toIso8601String(),
      },
    };
  }
}

Widget _testApp({
  String deploymentEnvironment = 'production',
  String relayOrigin = 'https://relay.ditchnow.nl',
}) => TheDitchApp(
  connectRuntimeOnStart: false,
  initialProjects: const [_testProject],
  deploymentEnvironment: deploymentEnvironment,
  relayOrigin: relayOrigin,
);

Widget _testToolbar(RuntimeConnectionPhase connection) => MaterialApp(
  home: Scaffold(
    body: DitchToolbar(
      projectName: 'Ditch',
      connection: connection,
      notifications: const [],
      unreadNotificationCount: 0,
      sidebarVisible: true,
      inspectorVisible: true,
      onToggleSidebar: () {},
      onToggleInspector: () {},
      onNotificationsViewed: () {},
      onOpenNotification: (_) {},
      onDismissNotification: (_) {},
      onDismissAllNotifications: () {},
      onOpenCodexSettings: () {},
      codexAvailable: true,
    ),
  ),
);

void main() {
  testWidgets('toolbar omits healthy connection status', (tester) async {
    await tester.pumpWidget(_testToolbar(RuntimeConnectionPhase.connected));

    expect(find.text('Connected'), findsNothing);

    await tester.pumpWidget(_testToolbar(RuntimeConnectionPhase.reconnecting));

    expect(find.text('Reconnecting'), findsOneWidget);
  });

  testWidgets('settings shows the installed app version at the bottom', (
    tester,
  ) async {
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('the_ditch/application'),
      (call) async {
        calls.add(call);
        return switch (call.method) {
          'appVersion' => {'version': '0.1.0', 'build': '106'},
          'getThemeMode' => 'system',
          'getPaneWidths' => <String, double>{},
          _ => null,
        };
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('the_ditch/application'),
        null,
      ),
    );

    await tester.pumpWidget(_testApp());
    await tester.tap(find.byKey(const Key('app-settings-button')));
    await tester.pumpAndSettle();

    expect(find.byKey(const Key('settings-version')), findsOneWidget);
    expect(find.text('v0.1.0.106'), findsOneWidget);
    expect(find.byKey(const Key('settings-upgrade-ditch')), findsOneWidget);
    expect(find.text('App Updates'), findsOneWidget);
    expect(find.text('License & Plans'), findsOneWidget);
    expect(calls.where((call) => call.method == 'appVersion'), hasLength(1));
  });

  testWidgets('staging settings keeps test mode visible', (tester) async {
    const relayOrigin =
        'https://ditch-remote-relay-staging.matin-1a7.workers.dev';
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('the_ditch/application'),
      (call) async => switch (call.method) {
        'appVersion' => {'version': '0.1.0', 'build': '108'},
        'getThemeMode' => 'system',
        'getPaneWidths' => <String, double>{},
        _ => null,
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('the_ditch/application'),
        null,
      ),
    );

    await tester.pumpWidget(
      _testApp(deploymentEnvironment: 'staging', relayOrigin: relayOrigin),
    );
    await tester.tap(find.byKey(const Key('app-settings-button')));
    await tester.pumpAndSettle();

    expect(
      find.byKey(const Key('settings-staging-environment')),
      findsOneWidget,
    );
    expect(find.text('TEST MODE'), findsOneWidget);
    expect(find.text('Relay'), findsOneWidget);
    expect(find.text(relayOrigin), findsOneWidget);
  });

  testWidgets('Upgrade Ditch displays the Relay-provided license name', (
    tester,
  ) async {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('the_ditch/application'),
      (call) async => switch (call.method) {
        'appVersion' => {'version': '0.1.0', 'build': '107'},
        _ => null,
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('the_ditch/application'),
        null,
      ),
    );
    final client = _CurrentLicenseClient(
      entitlement: {
        'active': true,
        'plan': 'commercial_monthly',
        'status': 'active',
        'current_license': {
          'edition': 'commercial',
          'status': 'active',
          'display_name': 'Founders Monthly',
          'plans': [
            {
              'kind': 'commercial_monthly',
              'display_name': 'Founders Monthly',
              'billing_type': 'recurring',
              'status': 'active',
              'valid_until': null,
            },
          ],
        },
      },
    );

    await tester.pumpWidget(
      MaterialApp(home: DitchUpdateDialog(client: client)),
    );
    await tester.pumpAndSettle();

    expect(find.text('Current license: Founders Monthly'), findsOneWidget);
    expect(find.text('Installed v0.1.0.107'), findsOneWidget);
    expect(find.byKey(const Key('ditch-current-license')), findsOneWidget);
  });

  testWidgets('Community update checks discover a release through Relay', (
    tester,
  ) async {
    final nativeCalls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('the_ditch/application'),
      (call) async {
        nativeCalls.add(call);
        return switch (call.method) {
          'appVersion' => {'version': '0.1.0', 'build': '107'},
          _ => null,
        };
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('the_ditch/application'),
        null,
      ),
    );
    final client = _CurrentLicenseClient(
      entitlement: {
        'active': false,
        'plan': 'community',
        'status': 'inactive',
        'current_license': {
          'edition': 'community',
          'status': 'active',
          'display_name': 'Ditch Community',
          'plans': <Object>[],
        },
      },
    );

    await tester.pumpWidget(
      MaterialApp(home: DitchUpdateDialog(client: client)),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('check-ditch-update')));
    await tester.pumpAndSettle();

    expect(
      nativeCalls.where((call) => call.method == 'checkCommunityUpdate'),
      isEmpty,
    );
    expect(client.communityChecks, 1);
    expect(find.text('Ditch 0.1.0.108 is available.'), findsOneWidget);
  });

  test('authorized update binds every signed release field to native code', () {
    final expiresAt = DateTime.utc(2026, 1, 1).toIso8601String();
    final arguments = authorizedCommercialUpdateArguments({
      'manifest': {
        'edition': 'commercial',
        'release_id': '11111111-1111-4111-8111-111111111111',
        'appcast_url': 'https://relay.example.test/release/appcast',
        'artifact_url': 'https://relay.example.test/release/artifact/ditch.dmg',
        'artifact_size': 4096,
        'version': '1.2.3',
        'build': '123',
        'channel': 'stable',
      },
      'update_session': {'bearer': 'test-bearer', 'expires_at': expiresAt},
    });

    expect(arguments, {
      'edition': 'commercial',
      'release_id': '11111111-1111-4111-8111-111111111111',
      'appcast_url': 'https://relay.example.test/release/appcast',
      'artifact_url': 'https://relay.example.test/release/artifact/ditch.dmg',
      'artifact_size': 4096,
      'version': '1.2.3',
      'build': '123',
      'channel': 'stable',
      'authorization_bearer': 'test-bearer',
      'expires_at': expiresAt,
    });
  });

  test('authorized update rejects incomplete release metadata', () {
    expect(
      () => authorizedCommercialUpdateArguments({
        'manifest': {'appcast_url': 'https://relay.example.test/appcast'},
        'update_session': {
          'bearer': 'test-bearer',
          'expires_at': '2026-01-01T00:00:00Z',
        },
      }),
      throwsFormatException,
    );
  });

  test('production upgrade surfaces contain no fallback price catalog', () {
    final flutterSource = File('lib/main.dart').readAsStringSync();
    final moneySource = File(
      'lib/data/commercial_models.dart',
    ).readAsStringSync();
    final rustSource = File(
      '../../crates/ditch_upgrade/src/lib.rs',
    ).readAsStringSync();

    for (final source in [flutterSource, moneySource, rustSource]) {
      expect(source, isNot(contains('price_eur_cents')));
      expect(source, isNot(contains('fallback_offers')));
    }
    expect(RegExp(r'€\s*\d').hasMatch(flutterSource), isFalse);
    expect(RegExp(r'€\s*\d').hasMatch(moneySource), isFalse);
  });

  test('canonical Commercial fixture parses exact money and UTC freshness', () {
    final catalog = CommercialOfferCatalog.fromJson(_canonicalCatalogJson());

    expect(catalog.refreshedAt, DateTime.utc(2026, 8, 30, 8));
    expect(catalog.offers, hasLength(3));
    expect(catalog.offers[1].baseAmountMinor, BigInt.from(1500));
    expect(catalog.offers[1].introductoryPrice?.amountMinor, BigInt.from(750));
    expect(catalog.offers[2].entitlement.macSlots, 2);
  });

  test(
    'catalog rejects numeric, malformed, and non-UTC refresh timestamps',
    () {
      for (final invalid in [
        1788085651552,
        'not-a-timestamp',
        '2026-08-30T10:00:00+02:00',
      ]) {
        final json = _canonicalCatalogJson()..['refreshed_at'] = invalid;
        expect(
          () => CommercialOfferCatalog.fromJson(json),
          throwsA(isA<FormatException>()),
        );
      }
    },
  );

  test('catalog rejects obsolete Relay fields and malformed offer entries', () {
    final obsolete = _canonicalCatalogJson();
    final first = (obsolete['offers'] as List).first as Map<String, dynamic>;
    first['normal_unit_amount'] = first.remove('base_amount_minor');
    expect(
      () => CommercialOfferCatalog.fromJson(obsolete),
      throwsA(isA<FormatException>()),
    );

    final malformed = _canonicalCatalogJson();
    (malformed['offers'] as List).add('not-an-offer');
    expect(
      () => CommercialOfferCatalog.fromJson(malformed),
      throwsA(isA<FormatException>()),
    );
  });

  test('money formatting obeys Relay currency exponent without floats', () {
    expect(
      CommercialMoney(
        currency: 'EUR',
        amountMinor: BigInt.from(1500),
        minorUnitExponent: 2,
      ).format(const Locale('en', 'US')),
      '€15',
    );
    expect(
      CommercialMoney(
        currency: 'JPY',
        amountMinor: BigInt.from(1500),
        minorUnitExponent: 0,
      ).format(const Locale('en', 'US')),
      '¥1,500',
    );
  });

  test('offer validation rejects malformed currency, exponent, and intro', () {
    final fixture = _canonicalCatalogJson();
    final first = (fixture['offers'] as List).first as Map<String, dynamic>;
    first['currency'] = 'euro';
    expect(
      () => CommercialOfferCatalog.fromJson(fixture),
      throwsA(isA<FormatException>()),
    );

    final exponent = _canonicalCatalogJson();
    ((exponent['offers'] as List).first
            as Map<String, dynamic>)['minor_unit_exponent'] =
        7;
    expect(
      () => CommercialOfferCatalog.fromJson(exponent),
      throwsA(isA<FormatException>()),
    );

    final intro = _canonicalCatalogJson();
    (((intro['offers'] as List)[1]
                as Map<String, dynamic>)['introductory_price']
            as Map<String, dynamic>)['amount_minor'] =
        '1500';
    expect(
      () => CommercialOfferCatalog.fromJson(intro),
      throwsA(isA<FormatException>()),
    );
  });

  testWidgets('Community presents restrained Commercial upgrade choices', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: CommercialUpgradeDialog(client: _CommercialOffersClient()),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('License & Plans'), findsOneWidget);
    expect(find.text('TEST MODE'), findsNothing);
    expect(
      find.byKey(const Key('commercial-staging-relay-link')),
      findsNothing,
    );
    expect(find.text('€15/month'), findsWidgets);
    expect(find.text('€7.50/month'), findsOneWidget);
    expect(find.text('+ VAT'), findsNWidgets(3));
    expect(
      find.text('VAT is calculated at checkout for your billing location.'),
      findsNWidgets(3),
    );
    expect(find.text('for the first 3 months'), findsOneWidget);
    expect(find.text('Then €15/month + VAT'), findsOneWidget);
    expect(find.text('€250 once'), findsOneWidget);
    expect(
      find.byKey(const Key('commercial-offer-action-offer_monthly_standard')),
      findsOneWidget,
    );
    expect(
      find.byKey(const Key('commercial-offer-action-offer_lifetime')),
      findsOneWidget,
    );
    expect(find.byKey(const Key('commercial-license-key')), findsOneWidget);
    expect(
      find.textContaining('SSH execution hosts are unlimited'),
      findsOneWidget,
    );
    expect(find.textContaining('remain Community features'), findsOneWidget);

    final basePrice = tester.widget<Text>(
      find.byKey(const Key('commercial-base-price-offer_monthly_intro')),
    );
    expect(basePrice.style?.decoration, TextDecoration.lineThrough);
    expect(find.textContaining('free trial'), findsNothing);
  });

  testWidgets('staging pricing identifies test mode and its Relay', (
    tester,
  ) async {
    const relayOrigin =
        'https://ditch-remote-relay-staging.matin-1a7.workers.dev';
    final calls = <MethodCall>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('the_ditch/application'),
          (call) async {
            calls.add(call);
            return true;
          },
        );
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            null,
          ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: CommercialUpgradeDialog(
          client: _CommercialOffersClient(),
          deploymentEnvironment: 'staging',
          relayOrigin: relayOrigin,
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('TEST MODE'), findsOneWidget);
    expect(find.text('Relay'), findsOneWidget);
    expect(find.text(relayOrigin), findsOneWidget);

    await tester.tap(find.byKey(const Key('commercial-staging-relay-link')));
    await tester.pump();

    expect(
      calls.where((call) => call.method == 'openURL').single.arguments,
      relayOrigin,
    );
  });

  testWidgets(
    'active subscriber sees status and billing without acquisition cards',
    (tester) async {
      final client = _CommercialOffersClient(initiallyActive: true);
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            (call) async {
              calls.add(call);
              return true;
            },
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('the_ditch/application'),
              null,
            ),
      );

      await tester.pumpWidget(
        MaterialApp(home: CommercialUpgradeDialog(client: client)),
      );
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('commercial-active-status')), findsOneWidget);
      expect(find.text('€15/month'), findsNothing);
      expect(find.text('€250 once'), findsNothing);
      expect(find.byKey(const Key('commercial-license-key')), findsNothing);
      expect(find.text('Available upgrade'), findsNothing);

      await tester.tap(find.byKey(const Key('manage-commercial-billing')));
      await tester.pumpAndSettle();
      expect(
        calls.where((call) => call.method == 'openURL').single.arguments,
        'https://billing.example.test/ditch/customer-session',
      );
    },
  );

  testWidgets('active Lifetime owner sees capacity separately from plans', (
    tester,
  ) async {
    final catalog = _testCatalog(
      offers: [
        _testOffer(
          id: 'offer_extra_pair',
          kind: 'lifetime_extra_pair',
          title: 'Lifetime extra pair',
          amountMinor: '5000',
          billingType: 'one_time',
          macSlots: 1,
          iPhoneSlots: 1,
          purchaseAction: 'add_capacity',
        ),
        _testOffer(
          id: 'offer_monthly_hidden',
          kind: 'commercial_monthly',
          title: 'Commercial Monthly',
          amountMinor: '1500',
          billingType: 'recurring',
          interval: 'month',
          macSlots: 1,
          iPhoneSlots: 1,
        ),
      ],
    );
    final client = _CommercialOffersClient(
      initiallyActive: true,
      catalog: catalog,
    );

    await tester.pumpWidget(
      MaterialApp(home: CommercialUpgradeDialog(client: client)),
    );
    await tester.pumpAndSettle();

    expect(find.text('Add capacity'), findsOneWidget);
    expect(find.text('Lifetime extra pair'), findsOneWidget);
    expect(find.text('€50 once'), findsOneWidget);
    expect(
      find.byKey(const Key('commercial-offer-offer_monthly_hidden')),
      findsNothing,
    );
    expect(find.text('Choose Commercial'), findsNothing);
  });

  testWidgets('catalog failure shows Retry without fabricated prices', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: CommercialUpgradeDialog(client: _CatalogFailureClient()),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Pricing is temporarily unavailable.'), findsOneWidget);
    expect(find.byKey(const Key('commercial-pricing-retry')), findsOneWidget);
    expect(find.textContaining('€'), findsNothing);
    expect(
      find.text('Commercial status is temporarily unavailable.'),
      findsNothing,
    );
  });

  testWidgets('Retry replaces a transient catalog error with Relay offers', (
    tester,
  ) async {
    final client = _RecoveringCatalogClient();
    await tester.pumpWidget(
      MaterialApp(home: CommercialUpgradeDialog(client: client)),
    );
    await tester.pumpAndSettle();

    expect(find.text('Pricing is temporarily unavailable.'), findsOneWidget);
    await tester.tap(find.byKey(const Key('commercial-pricing-retry')));
    await tester.pumpAndSettle();

    expect(client.attempts, 2);
    expect(find.text('Pricing is temporarily unavailable.'), findsNothing);
    expect(find.text('€15/month'), findsWidgets);
    expect(find.text('€250 once'), findsOneWidget);
  });

  testWidgets('stale catalog is identified without replacing Relay prices', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: CommercialUpgradeDialog(
          client: _CommercialOffersClient(catalog: _testCatalog(stale: true)),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('€15/month'), findsWidgets);
    expect(find.textContaining('last verified pricing'), findsOneWidget);
  });

  testWidgets('future upgrade is rendered only in the upgrade section', (
    tester,
  ) async {
    final catalog = _testCatalog(
      offers: [
        _testOffer(
          id: 'offer_future_upgrade',
          kind: 'commercial_lifetime',
          title: 'Future Commercial tier',
          amountMinor: '30000',
          billingType: 'one_time',
          macSlots: 3,
          iPhoneSlots: 3,
          purchaseAction: 'upgrade',
        ),
        _testOffer(
          id: 'offer_acquisition_hidden',
          kind: 'commercial_lifetime',
          title: 'First-purchase Lifetime',
          amountMinor: '25000',
          billingType: 'one_time',
          macSlots: 2,
          iPhoneSlots: 2,
        ),
      ],
    );

    await tester.pumpWidget(
      MaterialApp(
        home: CommercialUpgradeDialog(
          client: _CommercialOffersClient(
            initiallyActive: true,
            catalog: catalog,
          ),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Available upgrade'), findsOneWidget);
    expect(find.text('Future Commercial tier'), findsOneWidget);
    expect(find.text('First-purchase Lifetime'), findsNothing);
  });

  testWidgets('ineligible introductory offer is not displayed', (tester) async {
    final catalog = _testCatalog(
      offers: [
        _testOffer(
          id: 'offer_intro_ineligible',
          kind: 'commercial_monthly',
          title: 'Introductory offer',
          amountMinor: '1500',
          billingType: 'recurring',
          interval: 'month',
          macSlots: 1,
          iPhoneSlots: 1,
          introductory: const {
            'amount_minor': '750',
            'duration_count': 3,
            'duration_unit': 'month',
          },
          eligible: false,
        ),
        _testOffer(
          id: 'offer_standard_eligible',
          kind: 'commercial_monthly',
          title: 'Standard Monthly',
          amountMinor: '1500',
          billingType: 'recurring',
          interval: 'month',
          macSlots: 1,
          iPhoneSlots: 1,
        ),
      ],
    );

    await tester.pumpWidget(
      MaterialApp(
        home: CommercialUpgradeDialog(
          client: _CommercialOffersClient(catalog: catalog),
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Introductory offer'), findsNothing);
    expect(find.text('€7.50/month'), findsNothing);
    expect(find.text('Standard Monthly'), findsOneWidget);
  });

  testWidgets(
    'pending checkout keeps its package visible with explicit progress',
    (tester) async {
      final catalog = _testCatalog(
        offers: [
          _testOffer(
            id: 'offer_monthly_pending',
            kind: 'commercial_monthly',
            title: 'Commercial Monthly',
            amountMinor: '1500',
            billingType: 'recurring',
            interval: 'month',
            macSlots: 1,
            iPhoneSlots: 1,
            eligible: false,
            ineligibleReason: 'checkout_in_progress',
          ),
          _testOffer(
            id: 'offer_lifetime',
            kind: 'commercial_lifetime',
            title: 'Commercial Lifetime',
            amountMinor: '25000',
            billingType: 'one_time',
            macSlots: 2,
            iPhoneSlots: 2,
          ),
        ],
      );

      await tester.pumpWidget(
        MaterialApp(
          home: CommercialUpgradeDialog(
            client: _CommercialOffersClient(catalog: catalog),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.text('Commercial Monthly'), findsOneWidget);
      expect(
        find.text('Checkout in progress — payment has not yet been confirmed.'),
        findsOneWidget,
      );
      expect(find.text('Check status'), findsOneWidget);
      expect(find.text('Commercial Lifetime'), findsOneWidget);
    },
  );

  testWidgets(
    'expired subscription preserves Community and offers Relay renewal',
    (tester) async {
      final catalog = _testCatalog(
        offers: [
          _testOffer(
            id: 'offer_renew_monthly',
            kind: 'commercial_monthly',
            title: 'Commercial Monthly',
            amountMinor: '1500',
            billingType: 'recurring',
            interval: 'month',
            macSlots: 1,
            iPhoneSlots: 1,
            purchaseAction: 'renew',
          ),
        ],
      );
      final client = _CommercialOffersClient(
        status: 'expired',
        catalog: catalog,
      );

      await tester.pumpWidget(
        MaterialApp(home: CommercialUpgradeDialog(client: client)),
      );
      await tester.pumpAndSettle();

      expect(find.textContaining('expired.'), findsOneWidget);
      expect(
        find.text('Local and SSH Ditch continue to work.'),
        findsOneWidget,
      );
      expect(find.text('Renew'), findsWidgets);
    },
  );

  testWidgets('remote setup progress is pinned above the scrolling content', (
    tester,
  ) async {
    final client = _PendingRemoteSetupClient();
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => showDialog<void>(
              context: context,
              builder: (context) => AddRemoteProjectDialog(
                client: client,
                initialAlias: 'dev-box',
                repairOnly: true,
              ),
            ),
            child: const Text('Open setup'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Open setup'));
    await tester.pump();

    final progress = find.byKey(const Key('remote-setup-progress'));
    expect(progress, findsOneWidget);
    expect(
      find.ancestor(of: progress, matching: find.byType(ListView)),
      findsNothing,
    );
    final dialogMaterial = find.byWidgetPredicate(
      (widget) => widget is Material && widget.type == MaterialType.card,
    );
    expect(dialogMaterial, findsOneWidget);
    expect(
      tester.getTopLeft(progress).dy,
      closeTo(tester.getTopLeft(dialogMaterial).dy, 1),
    );

    client.pending.complete({'ready': false, 'checks': <Object>[]});
    await tester.pump();
  });

  test('agent execution settings expose model and approval controls', () {
    final settings = AgentExecutionSettings();

    expect(settings.approval, AgentApprovalPreset.approveForMe);
    expect(settings.protocolValue['approval'], 'ApproveForMe');
    expect(settings.protocolValue.containsKey('network_access'), isFalse);
  });

  test('remote approve-for-me setup requires a ready workspace sandbox', () {
    final base = <String, dynamic>{
      'ready': true,
      'checks': <Map<String, dynamic>>[
        {'key': 'runtime', 'state': 'ready'},
        {'key': 'codex_sandbox', 'state': 'install_available'},
      ],
    };

    expect(
      remoteSetupReadyForExecution(base, requireCodexSandbox: true),
      isFalse,
    );
    expect(
      remoteSetupReadyForExecution(base, requireCodexSandbox: false),
      isTrue,
    );
    expect(
      remoteSetupShouldConfigureSandbox(
        base,
        repairOnly: false,
        requireCodexSandbox: true,
        alreadyOffered: false,
      ),
      isTrue,
    );
    expect(
      remoteSetupShouldConfigureSandbox(
        base,
        repairOnly: true,
        requireCodexSandbox: true,
        alreadyOffered: false,
      ),
      isFalse,
    );

    final sandboxed = <String, dynamic>{
      ...base,
      'checks': <Map<String, dynamic>>[
        {'key': 'runtime', 'state': 'ready'},
        {'key': 'codex_sandbox', 'state': 'ready'},
      ],
    };
    expect(
      remoteSetupReadyForExecution(sandboxed, requireCodexSandbox: true),
      isTrue,
    );
  });

  for (final checkout in [
    (
      'offer_monthly_standard',
      'commercial-offer-action-offer_monthly_standard',
    ),
    ('offer_lifetime', 'commercial-offer-action-offer_lifetime'),
  ]) {
    testWidgets('${checkout.$1} opens the Relay-hosted checkout URL', (
      tester,
    ) async {
      final client = _RelayUpgradeClient();
      final calls = <MethodCall>[];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            (call) async {
              calls.add(call);
              return true;
            },
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('the_ditch/application'),
              null,
            ),
      );

      await tester.pumpWidget(
        MaterialApp(home: CommercialUpgradeDialog(client: client)),
      );
      await tester.pumpAndSettle();
      final action = find.byKey(Key(checkout.$2));
      await tester.ensureVisible(action);
      await tester.pump();
      await tester.tap(action);
      await tester.pumpAndSettle();

      expect(client.selectedOffers, [checkout.$1]);
      expect(client.activationCalls, 1);
      expect(client.commercialReleaseCalls, 1);
      expect(
        calls.where((call) => call.method == 'openURL').single.arguments,
        'https://payments.example.test/ditch/${checkout.$1}',
      );
      expect(
        calls.where((call) => call.method == 'installCommercialUpdate'),
        hasLength(1),
      );
    });
  }

  for (final environment in ['staging', 'production']) {
    for (final redeem in [false, true]) {
      testWidgets(
        '$environment Commercial app ${redeem ? 'redemption' : 'purchase'} activates without replacing the app or stopping agents',
        (tester) async {
          final client = _RelayUpgradeClient(
            installedEdition: 'commercial',
            environment: environment,
            activeAgents: 2,
          );
          final calls = <MethodCall>[];
          tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            (call) async {
              calls.add(call);
              return true;
            },
          );
          addTearDown(
            () =>
                tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
                  const MethodChannel('the_ditch/application'),
                  null,
                ),
          );
          await tester.pumpWidget(
            MaterialApp(
              home: CommercialUpgradeDialog(
                client: client,
                deploymentEnvironment: environment,
              ),
            ),
          );
          await tester.pumpAndSettle();
          expect(find.text('Installed app: Commercial'), findsOneWidget);
          if (redeem) {
            final field = find.byKey(const Key('commercial-license-key'));
            await tester.ensureVisible(field);
            await tester.enterText(field, 'existing-license');
            final button = find.byKey(const Key('activate-commercial-license'));
            await tester.ensureVisible(button);
            await tester.tap(button);
          } else {
            final button = find.byKey(
              const Key('commercial-offer-action-offer_monthly_standard'),
            );
            await tester.ensureVisible(button);
            await tester.tap(button);
          }
          await tester.pumpAndSettle();
          expect(client.activationCalls, 1);
          expect(client.commercialReleaseCalls, 0);
          expect(
            calls.where((c) => c.method == 'installCommercialUpdate'),
            isEmpty,
          );
          expect(
            find.text(
              'Commercial is active on this Mac. Remote Control is ready.',
            ),
            findsOneWidget,
          );
          expect(
            find.byKey(const Key('install-commercial-build')),
            findsNothing,
          );
        },
      );
    }
  }

  testWidgets(
    'activation slot failure preserves the paid license and offers retry without installing',
    (tester) async {
      final client = _RelayUpgradeClient(
        initiallyActive: true,
        installedEdition: 'commercial',
      )..failActivation = true;
      await tester.pumpWidget(
        MaterialApp(home: CommercialUpgradeDialog(client: client)),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('activate-commercial-device')));
      await tester.pumpAndSettle();
      expect(
        find.textContaining('Check the available Mac slots'),
        findsOneWidget,
      );
      expect(find.text('Commercial active'), findsOneWidget);
      client.failActivation = false;
      await tester.tap(find.byKey(const Key('activate-commercial-device')));
      await tester.pumpAndSettle();
      expect(client.activationCalls, 2);
      expect(
        find.text('Commercial is active on this Mac. Remote Control is ready.'),
        findsOneWidget,
      );
    },
  );

  test('lifetime add-on checkout remains a Ditch Relay offer', () async {
    final client = _RelayContractClient();
    final checkout = await client.createCommercialCheckout(
      'commercial-lifetime-extra-pair',
    );

    expect(client.lastRequest, {
      'CreateCommercialCheckout': {
        'offer_id': 'commercial-lifetime-extra-pair',
      },
    });
    expect(
      checkout['hosted_url'],
      'https://payments.example.test/ditch/add-on',
    );
  });

  testWidgets('pending checkout waits for Relay entitlement', (tester) async {
    final client = _RelayUpgradeClient(entitlementActivates: false);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('the_ditch/application'),
          (_) async => true,
        );
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            null,
          ),
    );

    await tester.pumpWidget(
      MaterialApp(home: CommercialUpgradeDialog(client: client)),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const Key('commercial-offer-action-offer_monthly_standard')),
    );
    await tester.pump();

    expect(find.text('Finishing upgrade…'), findsOneWidget);
    await tester.pumpWidget(const SizedBox.shrink());
    await tester.pump(const Duration(seconds: 2));
  });

  testWidgets('failed checkout does not claim activation', (tester) async {
    final client = _RelayUpgradeClient(failCheckout: true);
    await tester.pumpWidget(
      MaterialApp(home: CommercialUpgradeDialog(client: client)),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const Key('commercial-offer-action-offer_monthly_standard')),
    );
    await tester.pump();

    expect(
      find.textContaining('Checkout could not be started'),
      findsOneWidget,
    );
    expect(find.text('Finishing upgrade…'), findsNothing);
  });

  testWidgets('released pending checkout restores the package for retry', (
    tester,
  ) async {
    final client = _ReleasedCheckoutClient();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('the_ditch/application'),
          (_) async => true,
        );
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            null,
          ),
    );

    await tester.pumpWidget(
      MaterialApp(home: CommercialUpgradeDialog(client: client)),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const Key('commercial-offer-action-offer_monthly_standard')),
    );
    await tester.pump();
    await tester.pump(const Duration(seconds: 4));
    await tester.pumpAndSettle();

    expect(
      find.text(
        'Payment was not completed. The plan is available to try again.',
      ),
      findsOneWidget,
    );
    expect(find.text('Finishing upgrade…'), findsNothing);
    expect(find.text('Commercial Monthly'), findsWidgets);
  });

  testWidgets(
    'confirmed payment identifies a missing native verification key',
    (tester) async {
      final client = _RelayUpgradeClient();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            (call) async {
              if (call.method == 'openURL') return true;
              if (call.method == 'installCommercialUpdate') {
                throw PlatformException(
                  code: 'update_verification_not_configured',
                );
              }
              return false;
            },
          );
      addTearDown(
        () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .setMockMethodCallHandler(
              const MethodChannel('the_ditch/application'),
              null,
            ),
      );

      await tester.pumpWidget(
        MaterialApp(home: CommercialUpgradeDialog(client: client)),
      );
      await tester.pumpAndSettle();
      final action = find.byKey(
        const Key('commercial-offer-action-offer_monthly_standard'),
      );
      await tester.ensureVisible(action);
      await tester.tap(action);
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('commercial-active-status')), findsOneWidget);
      expect(
        find.textContaining('missing its public Sparkle verification key'),
        findsOneWidget,
      );
      expect(
        find.textContaining('Checkout could not be started'),
        findsNothing,
      );
      expect(find.byKey(const Key('install-commercial-build')), findsOneWidget);
    },
  );

  testWidgets('Commercial install retries after active agents finish', (
    tester,
  ) async {
    final client = _DeferredCommercialReleaseClient();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('the_ditch/application'),
          (call) async => call.method == 'installCommercialUpdate',
        );
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            null,
          ),
    );

    await tester.pumpWidget(
      MaterialApp(home: CommercialUpgradeDialog(client: client)),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const Key('install-commercial-build')));
    await tester.pump();

    expect(
      find.textContaining('Installation will retry automatically'),
      findsOneWidget,
    );
    expect(find.text('Check again'), findsOneWidget);

    await tester.pump(const Duration(seconds: 3));
    await tester.pumpAndSettle();

    expect(client.releaseRequests, 2);
    expect(
      find.textContaining('secure Commercial installer is ready'),
      findsOneWidget,
    );
    expect(client.activationCalls, 1);
  });

  testWidgets(
    'missing staging release leaves entitlement active and explains why',
    (tester) async {
      final client = _UnavailableCommercialReleaseClient();
      await tester.pumpWidget(
        MaterialApp(home: CommercialUpgradeDialog(client: client)),
      );
      await tester.pumpAndSettle();
      await tester.tap(find.byKey(const Key('install-commercial-build')));
      await tester.pumpAndSettle();

      expect(find.byKey(const Key('commercial-active-status')), findsOneWidget);
      expect(
        find.textContaining(
          'no compatible Commercial build has been published',
        ),
        findsOneWidget,
      );
      expect(find.textContaining('Your purchase is safe'), findsOneWidget);
    },
  );

  testWidgets('license redemption accepts an opaque Ditch-issued key', (
    tester,
  ) async {
    final client = _RelayUpgradeClient();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(
          const MethodChannel('the_ditch/application'),
          (_) async => true,
        );
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('the_ditch/application'),
            null,
          ),
    );

    await tester.pumpWidget(
      MaterialApp(home: CommercialUpgradeDialog(client: client)),
    );
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const Key('commercial-license-key')),
      'ditch_key_with_no_provider_format',
    );
    final activate = find.byKey(const Key('activate-commercial-license'));
    await tester.ensureVisible(activate);
    await tester.pump();
    await tester.tap(activate);
    await tester.pumpAndSettle();

    expect(client.redeemedLicense, 'ditch_key_with_no_provider_format');
    expect(find.byKey(const Key('commercial-license-key')), findsNothing);
    expect(find.byKey(const Key('commercial-active-status')), findsOneWidget);
  });

  test('presentation controller publishes immutable connection states', () {
    final controller = CommandCenterController();
    addTearDown(controller.dispose);
    final observed = <RuntimeConnectionPhase>[];
    controller.addListener(() => observed.add(controller.value.connection));

    controller.connected();
    controller.connecting(reconnecting: true);
    controller.unavailable('fixture offline');

    expect(observed, [
      RuntimeConnectionPhase.connected,
      RuntimeConnectionPhase.reconnecting,
      RuntimeConnectionPhase.unavailable,
    ]);
    expect(controller.value.connectionError, contains('fixture offline'));
  });

  test('presentation controller keeps panel state independent of runtime', () {
    final controller = CommandCenterController();
    addTearDown(controller.dispose);

    controller.toggleSidebar();
    controller.toggleInspector();

    expect(controller.value.sidebarVisible, isFalse);
    expect(controller.value.inspectorVisible, isFalse);
    expect(controller.value.connection, RuntimeConnectionPhase.connecting);
  });

  test('runtime parser accepts unit Accepted responses', () {
    final parsed = parseRuntimeResponseLine(
      '{"protocol_version":1,"id":"00000000-0000-4000-8000-000000000000","sent_at":"2026-01-01T00:00:00Z","body":"Accepted"}',
    );

    expect(parsed, {'Accepted': true});
  });

  test('runtime status DTO validates and types protocol fields', () {
    final status = RuntimeStatusDto.fromResponse({
      'RuntimeStatus': {
        'identity': 'The Ditch Runtime',
        'pid': 42,
        'socket_path': '/tmp/ditchd.sock',
        'active_session_count': 2,
        'attention_count': 1,
        'unread_attention_count': 1,
        'instance_id': 'instance-1',
        'codex_home': '/tmp/codex',
        'codex_binary': '/opt/homebrew/bin/codex',
        'build_version': '1.0.0',
        'capabilities': ['persistent_sessions_v1', 'always_on_web_access_v1'],
      },
    });

    expect(status.pid, 42);
    expect(status.activeSessionCount, 2);
    expect(status.unreadAttentionCount, 1);
    expect(status.codexBinary, '/opt/homebrew/bin/codex');
    expect(status.supportsPersistentSessions, isTrue);
    expect(status.supportsAlwaysOnWebAccess, isTrue);
  });

  test('Codex readiness requires compatibility and authentication', () {
    final report = CodexReadinessReport.fromResponse({
      'CodexReadiness': {
        'path': '/opt/homebrew/bin/codex',
        'version': 'codex-cli 1.2.3',
        'compatible': true,
        'authenticated': false,
        'update_supported': true,
        'doctor_supported': true,
        'issues': ['Codex is not signed in for this user.'],
        'diagnostics': '{}',
      },
    });

    expect(report.ready, isFalse);
    expect(report.updateSupported, isTrue);
    expect(report.issues, hasLength(1));
  });

  test('runtime project parser restores path and Git policy', () {
    final project = parseRuntimeProject({
      'name': 'Recovered',
      'root': '/tmp/recovered',
      'git_policy': 'AllowOutsideGit',
    });

    expect(project, isNotNull);
    expect(project!.name, 'Recovered');
    expect(project.path, '/tmp/recovered');
    expect(project.gitPolicy, ProjectGitPolicy.allowOutsideGit);
  });

  test('runtime project parser preserves remote execution identity', () {
    final project = parseRuntimeProject({
      'id': 'remote-project',
      'name': 'FieldOps',
      'root': '/home/mtn/fieldops',
      'git_policy': 'RequireRepository',
      'execution_target': {
        'kind': 'remote',
        'remote_machine_id': '11111111-1111-4111-8111-111111111111',
        'ssh_host_alias': 'dev-box',
      },
    });

    expect(project, isNotNull);
    expect(project!.isRemote, isTrue);
    expect(project.sshHostAlias, 'dev-box');
    expect(project.path, '/home/mtn/fieldops');
  });

  test('project reconciliation replaces duplicate ids and paths', () {
    final projects = <DitchProject>[
      const DitchProject(
        id: 'project-11',
        name: 'Old',
        path: '/tmp/project-11',
      ),
    ];

    upsertProject(
      projects,
      const DitchProject(
        id: 'project-11',
        name: 'Project#11',
        path: '/tmp/project-11',
      ),
    );

    expect(projects, hasLength(1));
    expect(projects.single.name, 'Project#11');
  });

  test('agent reconciliation removes duplicate runtime ids', () {
    final first = AgentSession(
      localId: 'agent-1',
      provider: AgentProvider.codex,
      status: AgentStatus.failed,
      messages: const [],
    );
    final duplicate = AgentSession(
      localId: 'agent-1',
      provider: AgentProvider.codex,
      status: AgentStatus.starting,
      messages: const [],
    );
    final sessions = [first, duplicate];
    final incoming = AgentSession(
      localId: 'agent-1',
      provider: AgentProvider.codex,
      status: AgentStatus.working,
      messages: const [],
      currentPrompt: 'try again',
    );

    reconcileAgentSession(sessions, incoming);

    expect(sessions, hasLength(1));
    expect(sessions.single.status, AgentStatus.working);
    expect(sessions.single.currentPrompt, 'try again');
  });

  test('live and persisted copies of one prompt reconcile exactly once', () {
    final createdAt = DateTime.utc(2026, 8, 19, 20, 43);
    final live = AgentChatMessage(
      identity: 'agent-1:$createdAt:user',
      role: ChatMessageRole.user,
      text: 'Run the task',
      createdAt: createdAt,
    );
    final persisted = AgentChatMessage(
      identity: 'persisted:agent-1:1',
      role: ChatMessageRole.user,
      text: 'Run the task',
      createdAt: createdAt,
    );
    final deliberatelyRepeated = AgentChatMessage(
      identity: 'persisted:agent-1:2',
      role: ChatMessageRole.user,
      text: 'Run the task',
      createdAt: createdAt.add(const Duration(seconds: 1)),
    );

    expect(uniqueRuntimeMessages([live], [persisted]), isEmpty);
    expect(uniqueRuntimeMessages([live], [persisted, deliberatelyRepeated]), [
      deliberatelyRepeated,
    ]);
  });

  test('groups tool messages by user turn with stable visible identity', () {
    final messages = [
      AgentChatMessage(
        identity: 'user-1',
        role: ChatMessageRole.user,
        text: 'First task',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'tool-1',
        role: ChatMessageRole.tool,
        text: 'command one',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'tool-2',
        role: ChatMessageRole.tool,
        text: 'command two',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'assistant-1',
        role: ChatMessageRole.assistant,
        text: 'Finished',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'user-2',
        role: ChatMessageRole.user,
        text: 'Second task',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'tool-3',
        role: ChatMessageRole.tool,
        text: 'command three',
        createdAt: DateTime(2026),
      ),
    ];

    final items = buildConversationItems(messages, isWorking: true);

    expect(items, hasLength(5));
    expect(items[1].identity, 'tool-activity:tool-1');
    expect(items[1].toolMessages, hasLength(2));
    expect(items[1].isActiveToolGroup, isFalse);
    expect(items[4].identity, 'tool-activity:tool-3');
    expect(items[4].isActiveToolGroup, isTrue);
  });

  testWidgets('tool activity collapses when the active turn finishes', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    addTearDown(viewport.dispose);
    var working = true;
    late StateSetter rebuild;
    final messages = [
      AgentChatMessage(
        identity: 'group-user',
        role: ChatMessageRole.user,
        text: 'Run checks',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'group-tool-1',
        role: ChatMessageRole.tool,
        text: 'cargo test',
        createdAt: DateTime(2026),
      ),
      AgentChatMessage(
        identity: 'group-tool-2',
        role: ChatMessageRole.tool,
        text: 'flutter test',
        createdAt: DateTime(2026),
      ),
    ];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return ConversationTranscript(
                messages: messages,
                viewport: viewport,
                isWorking: working,
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();

    expect(find.text('Tool activity · 2 actions'), findsOneWidget);
    expect(find.text('cargo test'), findsOneWidget);
    expect(find.text('flutter test'), findsOneWidget);

    rebuild(() => working = false);
    await tester.pump();
    expect(find.text('cargo test'), findsNothing);
    expect(find.text('flutter test'), findsNothing);

    await tester.tap(find.byKey(const Key('tool-activity-toggle')));
    await tester.pump();
    expect(find.text('cargo test'), findsOneWidget);
    expect(find.text('flutter test'), findsOneWidget);
  });

  test('sessions and attention are scoped to their project', () {
    final sessions = [
      AgentSession(
        localId: 'agent-a',
        projectId: 'project-a',
        provider: AgentProvider.codex,
        status: AgentStatus.failed,
        messages: const [],
      ),
      AgentSession(
        localId: 'agent-b',
        projectId: 'project-b',
        provider: AgentProvider.codex,
        status: AgentStatus.failed,
        messages: const [],
      ),
    ];
    final attention = [
      AttentionEvent(
        id: 'global',
        kind: AttentionKind.failed,
        icon: Icons.error_outline,
        title: 'Runtime',
        body: 'Global failure',
        createdAt: DateTime(2026),
      ),
      AttentionEvent(
        id: 'project-a-alert',
        projectId: 'project-a',
        kind: AttentionKind.failed,
        icon: Icons.error_outline,
        title: 'Codex',
        body: 'Project failure',
        createdAt: DateTime(2026),
      ),
    ];

    expect(
      sessionsForProject(sessions, 'project-a').map((item) => item.localId),
      ['agent-a'],
    );
    expect(attentionForProject(attention, 'project-b').map((item) => item.id), [
      'global',
    ]);
  });

  test('project summaries count agents and unread terminal results', () {
    final sessions = [
      AgentSession(
        localId: 'working',
        projectId: 'project-a',
        provider: AgentProvider.codex,
        status: AgentStatus.working,
        messages: const [],
      ),
      AgentSession(
        localId: 'completed',
        projectId: 'project-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
      AgentSession(
        localId: 'other',
        projectId: 'project-b',
        provider: AgentProvider.codex,
        status: AgentStatus.failed,
        messages: const [],
      ),
    ];
    final attention = [
      AttentionEvent(
        id: 'finished-a',
        sessionLocalId: 'completed',
        projectId: 'project-a',
        kind: AttentionKind.completed,
        icon: Icons.check_circle_outline,
        title: 'Finished',
        body: 'Done',
        createdAt: DateTime(2026),
      ),
    ];

    final unread = summarizeProjectAgents(
      sessions: sessions,
      attention: attention,
      projectId: 'project-a',
    );
    expect(unread.runningCount, 1);
    expect(unread.stoppedCount, 1);
    expect(unread.hasUnreadResult, isTrue);

    final read = summarizeProjectAgents(
      sessions: sessions,
      attention: attention,
      projectId: 'project-a',
      readAttentionIds: {'finished-a'},
    );
    expect(read.hasUnreadResult, isFalse);

    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'completed',
      ),
      {'finished-a'},
    );
    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'working',
      ),
      isEmpty,
    );
  });

  test('reading one agent result leaves other project results unread', () {
    final attention = [
      AttentionEvent(
        id: 'result-a',
        sessionLocalId: 'agent-a',
        projectId: 'project-a',
        kind: AttentionKind.completed,
        icon: Icons.check_circle_outline,
        title: 'A finished',
        body: 'Done',
        createdAt: DateTime(2026),
      ),
      AttentionEvent(
        id: 'result-b',
        sessionLocalId: 'agent-b',
        projectId: 'project-a',
        kind: AttentionKind.failed,
        icon: Icons.error_outline,
        title: 'B failed',
        body: 'Failed',
        createdAt: DateTime(2026),
      ),
    ];

    expect(
      summarizeProjectAgents(
        sessions: const [],
        attention: attention,
        projectId: 'project-a',
        readAttentionIds: {'result-a'},
      ).hasUnreadResult,
      isTrue,
    );
    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'agent-a',
        readAttentionIds: {'result-a'},
      ),
      isEmpty,
    );
    expect(
      unreadResultAttentionIdsForAgent(
        attention: attention,
        agentId: 'agent-b',
        readAttentionIds: {'result-a'},
      ),
      {'result-b'},
    );
  });

  test('notification navigation preserves exact project and agent ids', () {
    final target = AgentNotificationTarget.fromArguments({
      'projectId': 'project-a',
      'agentId': 'agent-b',
      'attentionId': 'attention-c',
    });

    expect(target, isNotNull);
    expect(target!.projectId, 'project-a');
    expect(target.agentId, 'agent-b');
    expect(target.attentionId, 'attention-c');
    expect(
      AgentNotificationTarget.fromArguments({'projectId': 'project-a'}),
      isNull,
    );
  });

  testWidgets('renders command center shell', (tester) async {
    await tester.pumpWidget(_testApp());

    expect(find.text('Ditch'), findsWidgets);
    expect(find.text('PROJECTS'), findsOneWidget);
    expect(find.text('Agents'), findsOneWidget);
    expect(find.text('Attention'), findsNothing);
    expect(find.byKey(const Key('notification-bell')), findsOneWidget);
    expect(find.text('New Agent'), findsOneWidget);
  });

  testWidgets('fresh install starts with Codex readiness onboarding', (
    tester,
  ) async {
    await tester.pumpWidget(const TheDitchApp(connectRuntimeOnStart: false));
    await tester.pumpAndSettle();

    expect(find.text('Welcome to Ditch'), findsOneWidget);
    expect(find.byKey(const Key('onboarding-continue')), findsOneWidget);
    expect(find.byKey(const Key('first-project-add')), findsNothing);
    expect(find.widgetWithText(AlertDialog, 'Add Project'), findsNothing);
    expect(find.text('/Users/tester/Projects/example'), findsNothing);
    expect(find.text('PROJECTS'), findsNothing);
  });

  testWidgets('first project action remains gated until Codex is ready', (
    tester,
  ) async {
    var addCalls = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: CodexOnboardingView(
          introduced: true,
          checking: false,
          updating: false,
          readiness: const CodexReadinessReport(
            path: '/opt/homebrew/bin/codex',
            version: 'codex-cli 1.2.3',
            compatible: true,
            authenticated: true,
            updateSupported: true,
            doctorSupported: true,
            issues: [],
            diagnostics: '{}',
          ),
          error: null,
          notificationReadiness: const NotificationReadiness(
            authorization: NotificationAuthorizationState.authorized,
            alertsEnabled: true,
            notificationCenterEnabled: true,
            soundsEnabled: true,
          ),
          notificationChecking: false,
          notificationError: null,
          onContinue: () {},
          onCheckAgain: () {},
          onChooseInstallation: () {},
          onUpdate: () {},
          onSignIn: () {},
          onOpenInstallInstructions: () {},
          onManageNotifications: () {},
          onCheckNotifications: () {},
          onAddProject: () => addCalls += 1,
        ),
      ),
    );

    expect(find.text('Codex is ready'), findsOneWidget);
    await tester.tap(find.byKey(const Key('first-project-add')));
    expect(addCalls, 1);
  });

  testWidgets('first project remains gated until notifications are enabled', (
    tester,
  ) async {
    var notificationCalls = 0;
    await tester.pumpWidget(
      MaterialApp(
        theme: DitchTheme.light(),
        home: CodexOnboardingView(
          introduced: true,
          checking: false,
          updating: false,
          readiness: const CodexReadinessReport(
            path: '/opt/homebrew/bin/codex',
            version: 'codex-cli 1.2.3',
            compatible: true,
            authenticated: true,
            updateSupported: true,
            doctorSupported: true,
            issues: [],
            diagnostics: '{}',
          ),
          error: null,
          notificationReadiness: const NotificationReadiness(
            authorization: NotificationAuthorizationState.notDetermined,
            alertsEnabled: false,
            notificationCenterEnabled: false,
            soundsEnabled: false,
          ),
          notificationChecking: false,
          notificationError: null,
          onContinue: () {},
          onCheckAgain: () {},
          onChooseInstallation: () {},
          onUpdate: () {},
          onSignIn: () {},
          onOpenInstallInstructions: () {},
          onManageNotifications: () => notificationCalls += 1,
          onCheckNotifications: () {},
          onAddProject: () {},
        ),
      ),
    );

    expect(find.byKey(const Key('first-project-add')), findsNothing);
    await tester.tap(find.byKey(const Key('enable-notifications')));
    expect(notificationCalls, 1);
  });

  testWidgets('runtime failure has a dedicated recovery surface', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: RuntimeRecoveryView(
          socketPath: '/tmp/ditchd.sock',
          error: 'connection refused',
          onRetry: () {},
          onOpenActivityMonitor: () async => true,
          onQuit: () async => true,
        ),
      ),
    );

    expect(find.text('Ditch Runtime is not responding'), findsOneWidget);
    expect(find.text('Retry Connection'), findsOneWidget);
    expect(find.text('Open Activity Monitor'), findsOneWidget);
    expect(find.text('Quit UI'), findsOneWidget);
    expect(find.text('/tmp/ditchd.sock'), findsOneWidget);
  });

  testWidgets('notification center starts empty without activity feed noise', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.byKey(const Key('notification-bell')));
    await tester.pump();
    expect(find.text('No notifications'), findsOneWidget);
    expect(find.text('Bells enabled'), findsNothing);
    expect(find.text('Codex prompted'), findsNothing);
    expect(find.text('Codex started'), findsNothing);
  });

  testWidgets('a project without saved sessions shows a ready agent card', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: const [],
            expandedAgentLocalId: null,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: 'Start here',
            onStartCodex: () {},
            onStartPrompt: (_) {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byKey(const Key('ready-agent-card')), findsOneWidget);
    expect(find.text('Ready for a new prompt'), findsOneWidget);
  });

  testWidgets('failed session without a Codex thread is read-only', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    var submitted = false;
    final session = AgentSession(
      localId: 'failed-agent',
      projectId: 'project-a',
      provider: AgentProvider.codex,
      status: AgentStatus.failed,
      messages: [
        AgentChatMessage(
          role: ChatMessageRole.system,
          text: 'Codex exited with code 1',
          createdAt: DateTime(2026),
        ),
      ],
      exitCode: 1,
      finishedAt: DateTime(2026),
      resumeBlockReason: 'NoCodexThread',
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: 'Retry',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) => submitted = true,
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.textContaining('Codex never created a thread'), findsOneWidget);
    expect(
      tester
          .widget<NativeComposerTextView>(find.byType(NativeComposerTextView))
          .enabled,
      isFalse,
    );
    expect(submitted, isFalse);
    expect(find.text('Codex exited with code 1'), findsOneWidget);
  });

  testWidgets('notification bell exposes global session actions', (
    tester,
  ) async {
    var opened = false;
    var dismissed = false;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.topRight,
            child: NotificationCenterButton(
              notifications: [
                AttentionEvent(
                  id: 'attention-test',
                  kind: AttentionKind.failed,
                  icon: Icons.error_outline,
                  title: 'Codex failed',
                  body: 'Exit code: 1.',
                  sessionLocalId: 'agent-0',
                  projectName: 'The Ditch',
                  agentName: 'Build agent',
                  createdAt: DateTime(2026),
                ),
              ],
              unreadCount: 1,
              onViewed: () {},
              onOpen: (_) => opened = true,
              onDismiss: (_) => dismissed = true,
              onDismissAll: () {},
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(const Key('notification-bell')));
    await tester.pump();
    expect(find.text('Codex failed'), findsOneWidget);
    expect(find.text('The Ditch · Build agent'), findsOneWidget);
    expect(find.text('Open'), findsOneWidget);
    expect(find.byTooltip('Dismiss notification'), findsOneWidget);

    await tester.tap(find.text('Open'));
    await tester.pump();
    expect(opened, isTrue);

    await tester.tap(find.byKey(const Key('notification-bell')));
    await tester.pump();
    await tester.tap(find.byTooltip('Dismiss notification'));
    await tester.pump();
    expect(dismissed, isTrue);
  });

  testWidgets('Commercial product update is installable from the bell', (
    tester,
  ) async {
    AttentionEvent? opened;
    final update = AttentionEvent(
      id: 'attention-product-update-120',
      kind: AttentionKind.completed,
      icon: Icons.system_update_alt,
      title: 'Ditch 1.2.0 is available',
      body: 'A verified Commercial update is ready.',
      action: AttentionAction.installProductUpdate,
      createdAt: DateTime(2026),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Align(
            alignment: Alignment.topRight,
            child: NotificationCenterButton(
              notifications: [update],
              unreadCount: 1,
              onViewed: () {},
              onOpen: (event) => opened = event,
              onDismiss: (_) {},
              onDismissAll: () {},
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(const Key('notification-bell')));
    await tester.pump();
    expect(find.text('Ditch 1.2.0 is available'), findsOneWidget);
    expect(find.text('Install'), findsOneWidget);
    expect(find.text('Open'), findsNothing);
    await tester.tap(find.text('Install'));
    expect(opened, same(update));
  });

  testWidgets('chat messages expose copy actions', (tester) async {
    final session = AgentSession(
      localId: 'copy-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: [
        AgentChatMessage(
          role: ChatMessageRole.assistant,
          text: 'Copy this response',
          createdAt: DateTime(2026),
        ),
      ],
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byTooltip('Copy message'), findsOneWidget);
    expect(find.byTooltip('Copy conversation'), findsOneWidget);
    expect(find.byType(SelectionArea), findsWidgets);
  });

  testWidgets('focused agent view exposes return and delete controls', (
    tester,
  ) async {
    final session = AgentSession(
      localId: 'focus-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: const [],
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: session.localId,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byTooltip('Return to agents (Esc)'), findsOneWidget);
    expect(find.byTooltip('Delete agent permanently'), findsOneWidget);
  });

  testWidgets('long conversation retains native scrolling', (tester) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final chatController = ScrollController();
    final agentListController = ScrollController();
    final session = AgentSession(
      localId: 'scroll-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: List.generate(
        30,
        (index) => AgentChatMessage(
          role: ChatMessageRole.assistant,
          text: 'Message $index with enough text to occupy a chat row.',
          createdAt: DateTime(2026),
        ),
      ),
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: [session],
            expandedAgentLocalId: session.localId,
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(
              scrollController: chatController,
            ),
            agentListController: agentListController,
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(chatController.position.maxScrollExtent, greaterThan(0));
    expect(chatController.offset, chatController.position.minScrollExtent);
    expect(
      find.text('Message 29 with enough text to occupy a chat row.'),
      findsOneWidget,
    );
    expect(
      find.text('Message 0 with enough text to occupy a chat row.'),
      findsNothing,
    );
    final header = find.byType(InkWell).first;
    final composer = find.byType(AgentComposer);
    final headerTop = tester.getTopLeft(header);
    final composerTop = tester.getTopLeft(composer);
    await tester.drag(find.byType(ListView).last, const Offset(0, 300));
    await tester.pumpAndSettle();
    expect(chatController.offset, greaterThan(0));
    expect(tester.getTopLeft(header), headerTop);
    expect(tester.getTopLeft(composer), composerTop);
  });

  testWidgets('detached conversation counts new messages without jumping', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    final messages = List.generate(
      30,
      (index) => AgentChatMessage(
        identity: 'message-$index',
        role: ChatMessageRole.assistant,
        text: 'Message $index with enough text to occupy a row.',
        createdAt: DateTime(2026),
      ),
    );
    String? historyError;
    late StateSetter rebuild;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return AgentsSurface(
                sessions: [
                  AgentSession(
                    localId: 'detached-agent',
                    provider: AgentProvider.codex,
                    status: AgentStatus.completed,
                    messages: messages,
                    hasOlderMessages: true,
                    historyError: historyError,
                  ),
                ],
                expandedAgentLocalId: 'detached-agent',
                focusedAgentLocalId: null,
                chatViewport: viewport,
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                onFocusAgent: (_) {},
                onToggleExpanded: (_) {},
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, 300));
    await tester.pumpAndSettle();
    expect(viewport.mode, ConversationViewportMode.detached);
    final detachedOffset = viewport.scrollController.offset;

    rebuild(() {
      messages.add(
        AgentChatMessage(
          identity: 'message-30',
          role: ChatMessageRole.assistant,
          text: 'Newest message',
          createdAt: DateTime(2026),
        ),
      );
    });
    await tester.pump();
    await tester.pump();

    expect(
      viewport.scrollController.offset,
      greaterThanOrEqualTo(detachedOffset),
    );
    expect(find.text('1 new message'), findsOneWidget);
    await tester.tap(find.byKey(const Key('conversation-new-messages')));
    await tester.pumpAndSettle();
    expect(viewport.mode, ConversationViewportMode.following);
    expect(viewport.unseenCount, 0);
    expect(viewport.scrollController.offset, 0);

    await tester.drag(find.byType(ListView), const Offset(0, 300));
    await tester.pumpAndSettle();
    final beforeOlderHistory = viewport.scrollController.offset;
    rebuild(() {
      messages.insert(
        0,
        AgentChatMessage(
          identity: 'message-older',
          role: ChatMessageRole.assistant,
          text: 'Older history',
          createdAt: DateTime(2025),
        ),
      );
    });
    await tester.pump();
    expect(viewport.unseenCount, 0);
    expect(
      viewport.scrollController.offset,
      moreOrLessEquals(beforeOlderHistory),
    );

    viewport.scrollController.jumpTo(
      viewport.scrollController.position.maxScrollExtent,
    );
    await tester.pump();
    final beforeHistoryFailure = viewport.scrollController.offset;
    rebuild(() => historyError = 'Could not load message history.');
    await tester.pump();
    expect(find.byKey(const Key('conversation-history-retry')), findsOneWidget);
    expect(
      viewport.scrollController.offset,
      moreOrLessEquals(beforeHistoryFailure),
    );
  });

  testWidgets('focused mode preserves the active agent reading position', (
    tester,
  ) async {
    final viewport = ConversationViewportController();
    final session = AgentSession(
      localId: 'focus-scroll-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.completed,
      messages: List.generate(
        30,
        (index) => AgentChatMessage(
          identity: 'focus-message-$index',
          role: ChatMessageRole.assistant,
          text: 'Message $index with enough text to occupy a row.',
          createdAt: DateTime(2026),
        ),
      ),
    );
    var focused = false;
    late StateSetter rebuild;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) {
              rebuild = setState;
              return AgentsSurface(
                sessions: [session],
                expandedAgentLocalId: session.localId,
                focusedAgentLocalId: focused ? session.localId : null,
                chatViewport: viewport,
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                onFocusAgent: (_) {},
                onToggleExpanded: (_) {},
              );
            },
          ),
        ),
      ),
    );
    await tester.pump();
    await tester.drag(find.byType(ListView), const Offset(0, 300));
    await tester.pumpAndSettle();
    final detachedOffset = viewport.scrollController.offset;

    rebuild(() => focused = true);
    await tester.pump();

    expect(viewport.mode, ConversationViewportMode.detached);
    expect(viewport.scrollController.offset, moreOrLessEquals(detachedOffset));
  });

  testWidgets('opens add project dialog', (tester) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();

    expect(find.widgetWithText(AlertDialog, 'Add Project'), findsOneWidget);
    expect(find.text('Local Project'), findsOneWidget);
    expect(find.text('Remote Project'), findsOneWidget);
    await tester.tap(find.text('Local Project'));
    await tester.pumpAndSettle();
    expect(find.text('Browse Folder…'), findsOneWidget);
    expect(find.text('Project name'), findsOneWidget);
    expect(find.text('Selected folder'), findsOneWidget);
    expect(find.text('Add & Configure'), findsOneWidget);
    expect(find.textContaining('.ditch/hooks'), findsOneWidget);
  });

  testWidgets('add project modal steps dismiss when the backdrop is clicked', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();
    await tester.tapAt(const Offset(4, 4));
    await tester.pumpAndSettle();
    expect(find.text('Local Project'), findsNothing);

    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Local Project'));
    await tester.pumpAndSettle();
    expect(find.text('Browse Folder…'), findsOneWidget);
    await tester.tapAt(const Offset(4, 4));
    await tester.pumpAndSettle();
    expect(find.text('Browse Folder…'), findsNothing);
  });

  testWidgets('folder picker fills the project path and inferred name', (
    tester,
  ) async {
    const channel = MethodChannel('the_ditch/project_picker');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      (call) async => '/tmp/My Project',
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );

    await tester.pumpWidget(_testApp());
    await tester.tap(find.text('Add Project'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Local Project'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Browse Folder…'));
    await tester.pumpAndSettle();

    expect(find.text('/tmp/My Project'), findsOneWidget);
    expect(find.text('My Project'), findsOneWidget);
    expect(find.text('Choose how Codex should run'), findsOneWidget);
    expect(
      find.text('Choose how this project should handle Git.'),
      findsOneWidget,
    );
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Add & Configure'),
          )
          .onPressed,
      isNull,
    );
    await tester.tap(find.byType(DropdownButtonFormField<ProjectGitPolicy>));
    await tester.pumpAndSettle();
    expect(find.text('Initialize Git Repository'), findsOneWidget);
    expect(find.text('Allow Codex Outside Git'), findsOneWidget);
    expect(find.textContaining('--skip-git-repo-check'), findsOneWidget);
    await tester.tap(find.text('Initialize Git Repository'));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<FilledButton>(
            find.widgetWithText(FilledButton, 'Add & Configure'),
          )
          .onPressed,
      isNotNull,
    );
  });

  testWidgets('start codex opens an initial prompt dialog', (tester) async {
    await tester.pumpWidget(_testApp());

    await tester.tap(find.text('New Agent'));
    await tester.pumpAndSettle();

    expect(
      find.widgetWithText(AlertDialog, 'Start Codex Session'),
      findsOneWidget,
    );
    expect(find.text('Initial prompt'), findsOneWidget);
    final dialog = find.byType(StartCodexSessionDialog);
    expect(
      tester
          .widget<TextField>(
            find.descendant(of: dialog, matching: find.byType(TextField)),
          )
          .controller
          ?.text,
      isEmpty,
    );

    await tester.enterText(
      find.descendant(of: dialog, matching: find.byType(TextField)),
      'new initial prompt',
    );
    await tester.pumpAndSettle();

    expect(
      find.descendant(of: dialog, matching: find.text('new initial prompt')),
      findsOneWidget,
    );
  });

  testWidgets('new agent action lives in the Agents header', (tester) async {
    await tester.pumpWidget(_testApp());

    expect(find.byKey(const Key('agents-new-agent-button')), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(DitchToolbar),
        matching: find.text('New Agent'),
      ),
      findsNothing,
    );
  });

  testWidgets('terminal supports horizontal and vertical workspace expansion', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(_testApp());

    await tester.tap(find.byKey(const Key('terminal-expand-horizontal')));
    await tester.pump();
    expect(find.byType(ProjectTerminalSurface), findsOneWidget);
    expect(
      tester.getTopLeft(find.byType(ProjectTerminalSurface)).dy,
      lessThan(tester.getTopLeft(find.text('Agents')).dy),
    );

    await tester.tap(find.byKey(const Key('terminal-expand-vertical')));
    await tester.pump();
    expect(find.byType(ProjectTerminalSurface), findsOneWidget);
    expect(find.text('Attention'), findsNothing);
  });

  testWidgets('project file tree opens directories and text files', (
    tester,
  ) async {
    final files = ProjectFilesState()
      ..directories[''] = const [
        ProjectFileEntry(
          name: 'lib',
          relativePath: 'lib',
          kind: ProjectFileEntryKind.directory,
          size: 0,
        ),
        ProjectFileEntry(
          name: 'README.md',
          relativePath: 'README.md',
          kind: ProjectFileEntryKind.file,
          size: 10,
        ),
      ];
    ProjectFileEntry? toggled;
    ProjectFileEntry? opened;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectFilesBody(
            files: files,
            presentation: TerminalPresentation.docked,
            onToggleDirectory: (entry) => toggled = entry,
            onOpenFile: (entry) => opened = entry,
            onRevealFile: (_) {},
            onBack: () {},
            onSave: () {},
            onReload: () {},
            onOverwrite: () {},
            onPresentationChanged: (_) {},
          ),
        ),
      ),
    );

    await tester.tap(find.text('lib'));
    await tester.tap(find.text('README.md'));
    expect(toggled?.relativePath, 'lib');
    expect(opened?.relativePath, 'README.md');
    files.dispose();
  });

  testWidgets('project editor edits saves and exposes every presentation', (
    tester,
  ) async {
    final document = ProjectEditorDocument(
      relativePath: 'lib/main.dart',
      content: 'before',
      revision: 'revision-1',
    );
    document.controller.text = 'after';
    final presentations = <TerminalPresentation>[];
    var saves = 0;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectFileEditorBody(
            document: document,
            presentation: TerminalPresentation.docked,
            onBack: () {},
            onSave: () => saves++,
            onReload: () {},
            onOverwrite: () {},
            onPresentationChanged: presentations.add,
          ),
        ),
      ),
    );

    expect(document.dirty, isTrue);
    await tester.tap(find.byKey(const Key('editor-save')));
    await tester.tap(find.byKey(const Key('editor-expand-horizontal')));
    await tester.tap(find.byKey(const Key('editor-expand-vertical')));
    await tester.tap(find.byKey(const Key('editor-maximize')));
    expect(saves, 1);
    expect(presentations, [
      TerminalPresentation.horizontal,
      TerminalPresentation.vertical,
      TerminalPresentation.maximized,
    ]);
    document.dispose();
  });

  testWidgets('remote project editor is read-only', (tester) async {
    final document = ProjectEditorDocument(
      relativePath: 'README.md',
      content: 'remote source',
      revision: 'revision-remote',
      readOnly: true,
    );
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectFileEditorBody(
            document: document,
            presentation: TerminalPresentation.docked,
            onBack: () {},
            onSave: () {},
            onReload: () {},
            onOverwrite: () {},
            onPresentationChanged: (_) {},
          ),
        ),
      ),
    );

    final editor = tester.widget<TextField>(
      find.byKey(const Key('project-file-editor')),
    );
    expect(editor.readOnly, isTrue);
    expect(
      tester.widget<IconButton>(find.byKey(const Key('editor-save'))).onPressed,
      isNull,
    );
    document.dispose();
  });

  testWidgets('maximized terminal restores with close or Escape', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1200, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(_testApp());

    await tester.tap(find.byKey(const Key('terminal-maximize')));
    await tester.pump();
    expect(find.byKey(const Key('terminal-maximize-close')), findsOneWidget);
    expect(find.byType(AgentsSurface), findsNothing);

    await tester.tap(find.byKey(const Key('terminal-maximize-close')));
    await tester.pump();
    expect(find.byType(AgentsSurface), findsOneWidget);

    await tester.tap(find.byKey(const Key('terminal-maximize')));
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await tester.pump();
    expect(find.byType(AgentsSurface), findsOneWidget);
  });

  testWidgets('workspace splitters resize both side panes', (tester) async {
    tester.view.physicalSize = const Size(1400, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    await tester.pumpWidget(_testApp());

    final projectsBefore = tester.getSize(find.byType(ProjectSidebar)).width;
    final inspectorBefore = tester
        .getSize(find.byType(ProjectToolsPanel))
        .width;

    await tester.drag(
      find.byKey(const Key('projects-resize-handle')),
      const Offset(60, 0),
    );
    await tester.pump();
    expect(
      tester.getSize(find.byType(ProjectSidebar)).width,
      greaterThan(projectsBefore),
    );

    await tester.drag(
      find.byKey(const Key('inspector-resize-handle')),
      const Offset(-60, 0),
    );
    await tester.pump();
    expect(
      tester.getSize(find.byType(ProjectToolsPanel)).width,
      greaterThan(inspectorBefore),
    );
    await tester.pump(const Duration(milliseconds: 50));
  });

  testWidgets('project reveal button does not select the project row', (
    tester,
  ) async {
    var selected = false;
    var revealed = false;
    const path = '/tmp/the-ditch-project';

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'The Ditch',
            path: path,
            selected: false,
            onTap: () => selected = true,
            onReveal: () => revealed = true,
            onCopyPath: () {},
            onDelete: () {},
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(const ValueKey('reveal-project-$path')));
    await tester.pump();

    expect(revealed, isTrue);
    expect(selected, isFalse);
  });

  testWidgets('remote project uses its trailing icon to reconnect', (
    tester,
  ) async {
    var selected = false;
    var reconnected = false;
    const path = '/srv/remote-project';

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'Remote Project',
            path: path,
            selected: false,
            isRemote: true,
            sshHostAlias: 'dev-box',
            remoteStatus: 'offline',
            onTap: () => selected = true,
            onReveal: () {},
            onCopyPath: () {},
            onReconnect: () => reconnected = true,
            onDelete: () {},
          ),
        ),
      ),
    );

    expect(find.byKey(const ValueKey('reveal-project-$path')), findsNothing);
    expect(find.byIcon(Icons.sync), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('reconnect-project-$path')));
    await tester.pump();

    expect(reconnected, isTrue);
    expect(selected, isFalse);
  });

  testWidgets('project tile renders agent counts and an unread result dot', (
    tester,
  ) async {
    const path = '/tmp/project-summary';
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'Summary Project',
            path: path,
            selected: false,
            runningCount: 2,
            stoppedCount: 3,
            hasUnreadResult: true,
            onTap: () {},
            onReveal: () {},
            onCopyPath: () {},
            onDelete: () {},
          ),
        ),
      ),
    );

    expect(find.text('2 running · 3 stopped'), findsOneWidget);
    expect(find.byKey(const ValueKey('project-unread-$path')), findsOneWidget);
  });

  testWidgets('agent terminal states render as colored status chips', (
    tester,
  ) async {
    await tester.pumpWidget(
      const MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              AgentStatusChip(status: AgentStatus.working),
              AgentStatusChip(status: AgentStatus.stopping),
              AgentStatusChip(status: AgentStatus.completed),
              AgentStatusChip(status: AgentStatus.failed),
            ],
          ),
        ),
      ),
    );

    expect(find.text('Running'), findsOneWidget);
    expect(find.text('Stopping'), findsOneWidget);
    expect(find.text('Completed'), findsOneWidget);
    expect(find.text('Failed'), findsOneWidget);
    expect(
      find.byKey(const ValueKey('agent-status-completed')),
      findsOneWidget,
    );
    expect(find.byKey(const ValueKey('agent-status-failed')), findsOneWidget);
  });

  testWidgets('stopping agent cannot be reprompted until shutdown finishes', (
    tester,
  ) async {
    final session = AgentSession(
      localId: 'stopping-agent',
      provider: AgentProvider.codex,
      status: AgentStatus.stopping,
      messages: const [],
      codexThreadId: 'thread-stopping',
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ExpandableAgentPanel(
            headerKey: const ValueKey('stopping-agent-header'),
            session: session,
            expanded: true,
            enlarged: false,
            chatViewport: ConversationViewportController(),
            composerKey: GlobalKey<AgentComposerState>(),
            initialPrompt: '',
            onTap: () {},
            onEnlarge: () {},
            onDelete: () {},
            onRename: (_) {},
            onSubmitPrompt: (_) {},
            onStopCodex: () {},
          ),
        ),
      ),
    );

    expect(find.textContaining('waiting for the thread'), findsOneWidget);
    expect(
      tester
          .widget<NativeComposerTextView>(find.byType(NativeComposerTextView))
          .enabled,
      isFalse,
    );
    expect(find.byKey(const ValueKey('agent-status-stopping')), findsOneWidget);
  });

  testWidgets(
    'unread result dot identifies the exact agent and clears on open',
    (tester) async {
      final sessions = [
        AgentSession(
          localId: 'new-result',
          provider: AgentProvider.codex,
          status: AgentStatus.completed,
          messages: const [],
        ),
        AgentSession(
          localId: 'old-result',
          provider: AgentProvider.codex,
          status: AgentStatus.completed,
          messages: const [],
        ),
      ];
      final unreadAgents = {'new-result'};

      await tester.pumpWidget(
        MaterialApp(
          home: Scaffold(
            body: StatefulBuilder(
              builder: (context, setState) => AgentsSurface(
                sessions: sessions,
                expandedAgentLocalId: null,
                focusedAgentLocalId: null,
                chatViewport: ConversationViewportController(),
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                hasUnreadResult: (session) =>
                    unreadAgents.contains(session.localId),
                onFocusAgent: (_) {},
                onToggleExpanded: (session) {
                  setState(() => unreadAgents.remove(session.localId));
                },
              ),
            ),
          ),
        ),
      );

      expect(
        find.byKey(const ValueKey('agent-unread-new-result')),
        findsOneWidget,
      );
      expect(
        find.byKey(const ValueKey('agent-unread-old-result')),
        findsNothing,
      );

      await tester.tap(find.byKey(_testAgentHeaderKey('new-result')));
      await tester.pump();
      expect(
        find.byKey(const ValueKey('agent-unread-new-result')),
        findsNothing,
      );
    },
  );

  testWidgets('collapsing an expanded agent preserves the element tree', (
    tester,
  ) async {
    final sessions = [
      AgentSession(
        localId: 'collapse-regression-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
      AgentSession(
        localId: 'collapse-regression-b',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
    ];
    String? expandedAgentId = sessions.first.localId;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: StatefulBuilder(
            builder: (context, setState) => AgentsSurface(
              sessions: sessions,
              expandedAgentLocalId: expandedAgentId,
              focusedAgentLocalId: null,
              chatViewport: ConversationViewportController(),
              agentListController: ScrollController(),
              composerKey: GlobalKey<AgentComposerState>(),
              headerKeyForAgent: _testAgentHeaderKey,
              initialPrompt: '',
              onStartCodex: () {},
              onSubmitPrompt: (_, _) {},
              onStopCodex: (_) {},
              onDeleteAgent: (_) {},
              onRenameAgent: (_, _) {},
              hasUnreadResult: (_) => true,
              onFocusAgent: (_) {},
              onToggleExpanded: (_) {
                setState(() => expandedAgentId = null);
              },
            ),
          ),
        ),
      ),
    );

    await tester.tap(find.byKey(_testAgentHeaderKey('collapse-regression-a')));
    await tester.pump();

    expect(tester.takeException(), isNull);
    expect(find.byKey(const ValueKey('collapse-regression-a')), findsOneWidget);
    expect(find.byKey(const ValueKey('collapse-regression-b')), findsOneWidget);
  });

  testWidgets(
    'switching projects shows the new project agent list with none open',
    (tester) async {
      final projectA = AgentSession(
        localId: 'project-switch-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      );
      final projectB = AgentSession(
        localId: 'project-switch-b',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      );
      var sessions = [projectA];
      String? expandedAgentId = projectA.localId;
      String? focusedAgentId = projectA.localId;
      final composerKeys = <String, GlobalKey<AgentComposerState>>{};

      await tester.pumpWidget(
        MaterialApp(
          home: StatefulBuilder(
            builder: (context, setState) => Scaffold(
              appBar: AppBar(
                actions: [
                  TextButton(
                    key: const Key('switch-project-regression'),
                    onPressed: () {
                      setState(() {
                        sessions = [projectB];
                        expandedAgentId = null;
                        focusedAgentId = null;
                      });
                    },
                    child: const Text('Switch'),
                  ),
                ],
              ),
              body: AgentsSurface(
                sessions: sessions,
                expandedAgentLocalId: expandedAgentId,
                focusedAgentLocalId: focusedAgentId,
                chatViewport: ConversationViewportController(),
                agentListController: ScrollController(),
                composerKey: GlobalKey<AgentComposerState>(),
                composerKeyForAgent: (agentId) => composerKeys.putIfAbsent(
                  agentId,
                  GlobalKey<AgentComposerState>.new,
                ),
                headerKeyForAgent: _testAgentHeaderKey,
                initialPrompt: '',
                onStartCodex: () {},
                onSubmitPrompt: (_, _) {},
                onStopCodex: (_) {},
                onDeleteAgent: (_) {},
                onRenameAgent: (_, _) {},
                onFocusAgent: (_) {},
                onToggleExpanded: (_) {},
              ),
            ),
          ),
        ),
      );

      await tester.tap(find.byKey(const Key('switch-project-regression')));
      await tester.pump();

      expect(tester.takeException(), isNull);
      expect(find.byKey(const ValueKey('project-switch-a')), findsNothing);
      expect(find.byKey(const ValueKey('project-switch-b')), findsOneWidget);
      expect(find.text('Agents'), findsOneWidget);
      expect(find.byType(AgentChatPanel), findsNothing);
    },
  );

  testWidgets('project right click exposes copy path and delete actions', (
    tester,
  ) async {
    var copied = false;
    var deleted = false;
    const path = '/tmp/context-project';

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'Context Project',
            path: path,
            selected: false,
            onTap: () {},
            onReveal: () {},
            onCopyPath: () => copied = true,
            onDelete: () => deleted = true,
          ),
        ),
      ),
    );

    final tile = find.byKey(const ValueKey('project-tile-$path'));
    await tester.tap(tile, buttons: kSecondaryMouseButton);
    await tester.pumpAndSettle();
    expect(find.text('Copy Project Path'), findsOneWidget);
    expect(find.text('Delete'), findsOneWidget);

    await tester.tap(find.text('Copy Project Path'));
    await tester.pumpAndSettle();
    expect(copied, isTrue);
    expect(deleted, isFalse);

    await tester.tap(tile, buttons: kSecondaryMouseButton);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(deleted, isTrue);
  });

  testWidgets('remote project right click exposes reconnect', (tester) async {
    var reconnected = false;
    const path = '/srv/context-project';

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: ProjectTile(
            name: 'Remote Context Project',
            path: path,
            selected: false,
            isRemote: true,
            sshHostAlias: 'dev-box',
            remoteStatus: 'offline',
            onTap: () {},
            onReveal: () {},
            onCopyPath: () {},
            onReconnect: () => reconnected = true,
            onDelete: () {},
          ),
        ),
      ),
    );

    await tester.tap(
      find.byKey(const ValueKey('project-tile-$path')),
      buttons: kSecondaryMouseButton,
    );
    await tester.pumpAndSettle();
    expect(find.text('Reconnect'), findsOneWidget);

    await tester.tap(find.text('Reconnect'));
    await tester.pumpAndSettle();
    expect(reconnected, isTrue);
  });

  testWidgets('expanded agent has persistent prompt composer', (tester) async {
    await tester.pumpWidget(_testApp());

    expect(find.byType(AgentComposer), findsOneWidget);
    expect(find.byType(ThinkingStatusStrip), findsOneWidget);
    expect(find.textContaining('Thinking'), findsNothing);
    expect(find.byType(TextField), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).controller?.text,
      isEmpty,
    );
    expect(find.text('Start'), findsOneWidget);
    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isTrue,
    );
  });

  testWidgets('composer accepts typed replacement text', (tester) async {
    await tester.pumpWidget(_testApp());

    await tester.enterText(find.byType(TextField), 'hello');
    await tester.pumpAndSettle();

    expect(find.text('hello'), findsOneWidget);
  });

  testWidgets('composer starts compact and keeps controls below the editor', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    final editorShell = tester.widget<AnimatedContainer>(
      find.byKey(const Key('composer-editor-shell')),
    );
    expect(editorShell.constraints?.maxHeight, 48);

    final editorTop = tester.getTopLeft(
      find.byKey(const Key('composer-editor-shell')),
    );
    final approvalTop = tester.getTopLeft(find.text('Approve for me'));
    expect(approvalTop.dy, greaterThan(editorTop.dy));
  });

  testWidgets('whole composer surface focuses the editor', (tester) async {
    final outsideFocus = FocusNode(debugLabel: 'outside-composer-focus');
    addTearDown(outsideFocus.dispose);
    String? submitted;

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              Focus(
                focusNode: outsideFocus,
                child: const SizedBox(width: 40, height: 40),
              ),
              AgentComposer(
                initialText: '',
                hasSession: true,
                isWorking: false,
                onSubmit: (value) => submitted = value,
                onStop: () {},
              ),
            ],
          ),
        ),
      ),
    );

    outsideFocus.requestFocus();
    await tester.pump();
    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isFalse,
    );

    final surface = tester.getRect(
      find.byKey(const Key('composer-focus-surface')),
    );
    await tester.tapAt(surface.topLeft + const Offset(4, 4));
    await tester.pump();

    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isTrue,
    );

    await tester.enterText(find.byType(TextField), 'still a button');
    await tester.pump();
    await tester.tap(find.text('Send'));
    await tester.pump();
    expect(submitted, 'still a button');
  });

  testWidgets('native composer focus evicts stale Flutter widget focus', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);

    int? platformViewId;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (call) async {
        if (call.method == 'create') {
          final arguments = call.arguments as Map<dynamic, dynamic>;
          platformViewId = arguments['id'] as int;
        }
        return null;
      },
    );
    addTearDown(() {
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform_views,
        null,
      );
    });

    final staleTerminalFocus = FocusNode(debugLabel: 'stale-terminal-focus');
    addTearDown(staleTerminalFocus.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: Column(
          children: [
            const SizedBox(
              width: 320,
              height: 80,
              child: AppKitView(
                viewType: 'the_ditch/composer_text_view',
                layoutDirection: TextDirection.ltr,
              ),
            ),
            Focus(
              focusNode: staleTerminalFocus,
              child: const SizedBox(width: 100, height: 40),
            ),
          ],
        ),
      ),
    );
    await tester.pump();

    final platformViewFocus = tester
        .widget<Focus>(
          find.descendant(
            of: find.byType(AppKitView),
            matching: find.byType(Focus),
          ),
        )
        .focusNode!;
    staleTerminalFocus.requestFocus();
    await tester.pump();
    expect(staleTerminalFocus.hasFocus, isTrue);
    expect(platformViewFocus.hasFocus, isFalse);
    expect(platformViewId, isNotNull);

    final message = SystemChannels.platform_views.codec.encodeMethodCall(
      MethodCall('viewFocused', platformViewId),
    );
    tester.binding.defaultBinaryMessenger.handlePlatformMessage(
      SystemChannels.platform_views.name,
      message,
      (_) {},
    );
    await tester.pump();

    expect(staleTerminalFocus.hasFocus, isFalse);
    expect(platformViewFocus.hasFocus, isTrue);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('composer sends with Enter and keeps focus', (tester) async {
    String? submitted;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentComposer(
            initialText: '',
            hasSession: true,
            isWorking: false,
            onSubmit: (value) => submitted = value,
            onStop: () {},
          ),
        ),
      ),
    );

    await tester.enterText(find.byType(TextField), 'hello');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();

    expect(submitted, 'hello');
    expect(find.text('hello'), findsNothing);
    expect(
      tester.widget<TextField>(find.byType(TextField)).focusNode?.hasFocus,
      isTrue,
    );
  });

  testWidgets('composer keeps an editable draft while agent is working', (
    tester,
  ) async {
    String? submitted;

    Widget buildComposer({required bool isWorking}) => MaterialApp(
      home: Scaffold(
        body: AgentComposer(
          initialText: '',
          hasSession: true,
          isWorking: isWorking,
          onSubmit: (value) => submitted = value,
          onStop: () {},
        ),
      ),
    );

    await tester.pumpWidget(buildComposer(isWorking: true));
    expect(tester.widget<TextField>(find.byType(TextField)).enabled, isTrue);

    await tester.enterText(find.byType(TextField), 'draft next turn');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();

    expect(submitted, isNull);
    expect(find.text('draft next turn'), findsOneWidget);

    await tester.pumpWidget(buildComposer(isWorking: false));
    await tester.pump();
    expect(find.text('draft next turn'), findsOneWidget);

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    expect(submitted, 'draft next turn');
  });

  testWidgets('agent title supports inline rename', (tester) async {
    String? renamed;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: EditableAgentTitle(
            title: 'Codex session title',
            hasOverride: false,
            onRename: (value) => renamed = value,
          ),
        ),
      ),
    );

    await tester.tap(find.text('Codex session title'));
    await tester.pump();
    await tester.enterText(find.byKey(const Key('agent-title-editor')), 'Plan');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await tester.pump();

    expect(renamed, 'Plan');
  });

  testWidgets('agent title draft survives live agent updates', (tester) async {
    final headerKey = GlobalKey();
    final session = AgentSession(
      localId: 'agent-live-update',
      provider: AgentProvider.codex,
      status: AgentStatus.working,
      messages: [],
      codexTitle: 'Working agent',
    );

    Widget buildPanel() => MaterialApp(
      home: Scaffold(
        body: ExpandableAgentPanel(
          headerKey: headerKey,
          session: session,
          expanded: false,
          enlarged: false,
          chatViewport: null,
          composerKey: null,
          initialPrompt: '',
          onTap: () {},
          onEnlarge: () {},
          onDelete: () {},
          onRename: (_) {},
          onSubmitPrompt: (_) {},
          onStopCodex: () {},
        ),
      ),
    );

    await tester.pumpWidget(buildPanel());
    await tester.tap(find.text('Working agent'));
    await tester.pump();
    await tester.enterText(
      find.byKey(const Key('agent-title-editor')),
      'Partial rename',
    );

    session.lastVisibleAction = 'Received another tool update';
    session.messages.add(
      AgentChatMessage(
        role: ChatMessageRole.tool,
        text: 'Tool finished',
        createdAt: DateTime(2026),
      ),
    );
    await tester.pumpWidget(buildPanel());
    await tester.pump();

    final editor = tester.widget<TextField>(
      find.byKey(const Key('agent-title-editor')),
    );
    expect(editor.controller?.text, 'Partial rename');
    expect(
      tester.widget<EditableText>(find.byType(EditableText)).focusNode.hasFocus,
      isTrue,
    );
  });

  testWidgets('empty state does not expose a meaningless stop action', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    expect(find.widgetWithText(OutlinedButton, 'Stop'), findsNothing);
  });

  testWidgets('conversation uses native chat surface instead of terminal', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(find.textContaining('[39m'), findsNothing);
    expect(find.textContaining('[?2026h'), findsNothing);
  });

  testWidgets('empty project does not create a synthetic agent session', (
    tester,
  ) async {
    await tester.pumpWidget(_testApp());

    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(find.byType(ExpandableAgentPanel), findsNothing);
    expect(find.byKey(const Key('ready-agent-card')), findsOneWidget);
  });

  testWidgets('expanded agent owns one bounded conversation scrollable', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1200, 1400);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final sessions = [
      AgentSession(
        localId: 'agent-a',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
      AgentSession(
        localId: 'agent-b',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: const [],
      ),
    ];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: sessions,
            expandedAgentLocalId: 'agent-a',
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: 'Start here',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    expect(find.byType(DropdownButton<String>), findsNothing);
    expect(find.byType(ExpandableAgentPanel), findsOneWidget);
    expect(find.byType(AgentChatPanel), findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(AgentChatPanel),
        matching: find.byType(ListView),
      ),
      findsOneWidget,
    );
    expect(find.byType(CustomScrollView), findsNothing);
    expect(find.byType(ListView), findsOneWidget);
  });

  testWidgets('later expanded agent scrolls to a pinned visible composer', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(900, 600);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final sessions = List.generate(
      3,
      (index) => AgentSession(
        localId: 'agent-$index',
        provider: AgentProvider.codex,
        status: AgentStatus.completed,
        messages: index == 2
            ? List.generate(
                30,
                (messageIndex) => AgentChatMessage(
                  identity: 'agent-2-message-$messageIndex',
                  role: ChatMessageRole.assistant,
                  text: 'Message $messageIndex',
                  createdAt: DateTime(2026),
                ),
              )
            : const [],
      ),
    );

    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: AgentsSurface(
            sessions: sessions,
            expandedAgentLocalId: 'agent-2',
            focusedAgentLocalId: null,
            chatViewport: ConversationViewportController(),
            agentListController: ScrollController(),
            composerKey: GlobalKey<AgentComposerState>(),
            headerKeyForAgent: _testAgentHeaderKey,
            initialPrompt: '',
            onStartCodex: () {},
            onSubmitPrompt: (_, _) {},
            onStopCodex: (_) {},
            onDeleteAgent: (_) {},
            onRenameAgent: (_, _) {},
            onFocusAgent: (_) {},
            onToggleExpanded: (_) {},
          ),
        ),
      ),
    );

    await tester.pump();
    final transcript = find.byType(ListView);
    final header = find.byType(InkWell).first;
    final composer = find.byType(AgentComposer);
    final headerTop = tester.getTopLeft(header);
    final composerTop = tester.getTopLeft(composer);
    await tester.drag(transcript, const Offset(0, 300));
    await tester.pumpAndSettle();
    expect(tester.getTopLeft(header), headerTop);
    expect(tester.getTopLeft(composer), composerTop);
  });

  testWidgets('chat messages alternate clearly between left and right', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: Column(
            children: [
              AgentChatBubble(
                message: AgentChatMessage(
                  role: ChatMessageRole.user,
                  text: 'User message',
                  createdAt: DateTime(2026),
                ),
              ),
              AgentChatBubble(
                message: AgentChatMessage(
                  role: ChatMessageRole.assistant,
                  text: 'Assistant message',
                  createdAt: DateTime(2026),
                ),
              ),
            ],
          ),
        ),
      ),
    );

    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is Align && widget.alignment == Alignment.centerRight,
      ),
      findsOneWidget,
    );
    expect(
      find.byWidgetPredicate(
        (widget) => widget is Align && widget.alignment == Alignment.centerLeft,
      ),
      findsOneWidget,
    );
  });
}
