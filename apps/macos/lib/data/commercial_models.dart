import 'package:flutter/widgets.dart';
import 'package:intl/intl.dart';

enum CommercialOfferKind {
  commercialMonthly,
  commercialLifetime,
  lifetimeExtraPair;

  static CommercialOfferKind parse(Object? value) => switch (value) {
    'commercial_monthly' => commercialMonthly,
    'commercial_lifetime' => commercialLifetime,
    'lifetime_extra_pair' => lifetimeExtraPair,
    _ => throw FormatException('Unknown Commercial offer kind: $value'),
  };
}

enum CommercialBillingType {
  recurring,
  oneTime;

  static CommercialBillingType parse(Object? value) => switch (value) {
    'recurring' => recurring,
    'one_time' => oneTime,
    _ => throw FormatException('Unknown Commercial billing type: $value'),
  };
}

enum CommercialBillingInterval {
  day,
  week,
  month,
  year;

  static CommercialBillingInterval? parseNullable(Object? value) =>
      switch (value) {
        null => null,
        'day' => day,
        'week' => week,
        'month' => month,
        'year' => year,
        _ => throw FormatException(
          'Unknown Commercial billing interval: $value',
        ),
      };

  String label(int count) => count == 1 ? name : '${name}s';
}

enum CommercialPurchaseAction {
  acquire,
  addCapacity,
  upgrade,
  renew;

  static CommercialPurchaseAction parse(Object? value) => switch (value) {
    'acquire' => acquire,
    'add_capacity' => addCapacity,
    'upgrade' => upgrade,
    'renew' => renew,
    _ => throw FormatException('Unknown Commercial purchase action: $value'),
  };
}

class CommercialMoney {
  const CommercialMoney({
    required this.currency,
    required this.amountMinor,
    required this.minorUnitExponent,
  });

  final String currency;
  final BigInt amountMinor;
  final int minorUnitExponent;

  String format(Locale locale) {
    if (minorUnitExponent < 0 || minorUnitExponent > 6) {
      throw const FormatException('Unsupported currency minor-unit exponent.');
    }
    final minorScale = BigInt.from(10).pow(minorUnitExponent);
    final micros = amountMinor * BigInt.from(10).pow(6 - minorUnitExponent);
    if (micros.bitLength > 63) {
      throw const FormatException('Commercial amount is too large to display.');
    }
    final displayDigits = amountMinor.remainder(minorScale) == BigInt.zero
        ? 0
        : minorUnitExponent;
    return NumberFormat.simpleCurrency(
      locale: locale.toLanguageTag(),
      name: currency.toUpperCase(),
      decimalDigits: displayDigits,
    ).format(MicroMoney(micros.toInt()));
  }
}

class CommercialIntroductoryPrice {
  const CommercialIntroductoryPrice({
    required this.amountMinor,
    required this.durationCount,
    required this.durationUnit,
  });

  factory CommercialIntroductoryPrice.fromJson(Map<String, dynamic> json) {
    _requireOnlyKeys(json, const {
      'amount_minor',
      'duration_count',
      'duration_unit',
    }, 'Commercial introductory price');
    return CommercialIntroductoryPrice(
      amountMinor: _minorAmount(json['amount_minor']),
      durationCount: _positiveInt(json['duration_count'], 'duration_count'),
      durationUnit:
          CommercialBillingInterval.parseNullable(json['duration_unit']) ??
          (throw const FormatException('Missing introductory duration unit.')),
    );
  }

  final BigInt amountMinor;
  final int durationCount;
  final CommercialBillingInterval durationUnit;
}

class CommercialOfferEntitlement {
  const CommercialOfferEntitlement({
    required this.macSlots,
    required this.iPhoneSlots,
    required this.sshHostsUnlimited,
  });

  factory CommercialOfferEntitlement.fromJson(Map<String, dynamic> json) {
    _requireOnlyKeys(json, const {
      'mac_slots',
      'iphone_slots',
      'ssh_hosts_unlimited',
    }, 'Commercial entitlement');
    if (json['ssh_hosts_unlimited'] != true) {
      throw const FormatException(
        'Commercial SSH execution hosts must remain unlimited.',
      );
    }
    return CommercialOfferEntitlement(
      macSlots: _nonNegativeInt(json['mac_slots'], 'mac_slots'),
      iPhoneSlots: _nonNegativeInt(json['iphone_slots'], 'iphone_slots'),
      sshHostsUnlimited: json['ssh_hosts_unlimited'] == true,
    );
  }

  final int macSlots;
  final int iPhoneSlots;
  final bool sshHostsUnlimited;
}

class CommercialOffer {
  const CommercialOffer({
    required this.offerId,
    required this.kind,
    required this.title,
    required this.description,
    required this.currency,
    required this.baseAmountMinor,
    required this.minorUnitExponent,
    required this.billingType,
    required this.recurringInterval,
    required this.recurringIntervalCount,
    required this.introductoryPrice,
    required this.purchaseAction,
    required this.eligible,
    required this.ineligibleReason,
    required this.entitlement,
  });

  factory CommercialOffer.fromJson(Map<String, dynamic> json) {
    _requireOnlyKeys(json, const {
      'offer_id',
      'kind',
      'title',
      'description',
      'currency',
      'base_amount_minor',
      'minor_unit_exponent',
      'billing_type',
      'recurring_interval',
      'recurring_interval_count',
      'introductory_price',
      'purchase_action',
      'eligible',
      'ineligible_reason',
      'entitlement',
    }, 'Commercial offer');
    final offerId = _requiredString(json['offer_id'], 'offer_id');
    final title = _requiredString(json['title'], 'title');
    if (offerId.trim() != offerId || offerId.length > 512) {
      throw const FormatException('Invalid Commercial offer identifier.');
    }
    if (title.length > 200) {
      throw const FormatException('Commercial offer title is too long.');
    }
    final descriptionValue = json['description'];
    if (descriptionValue != null && descriptionValue is! String) {
      throw const FormatException('Invalid Commercial offer description.');
    }
    final description = descriptionValue as String?;
    if (description != null && description.length > 1000) {
      throw const FormatException('Commercial offer description is too long.');
    }
    final currency = json['currency'];
    if (currency is! String || !RegExp(r'^[A-Z]{3}$').hasMatch(currency)) {
      throw const FormatException('Invalid Commercial offer currency.');
    }
    final exponent = _nonNegativeInt(
      json['minor_unit_exponent'],
      'minor_unit_exponent',
    );
    if (exponent > 6) {
      throw const FormatException('Unsupported currency minor-unit exponent.');
    }
    final billingType = CommercialBillingType.parse(json['billing_type']);
    final recurringInterval = CommercialBillingInterval.parseNullable(
      json['recurring_interval'],
    );
    final recurringCount = json['recurring_interval_count'] == null
        ? null
        : _positiveInt(
            json['recurring_interval_count'],
            'recurring_interval_count',
          );
    if (billingType == CommercialBillingType.recurring &&
        (recurringInterval == null || recurringCount == null)) {
      throw const FormatException('Recurring offer is missing its interval.');
    }
    if (billingType == CommercialBillingType.oneTime &&
        (recurringInterval != null || recurringCount != null)) {
      throw const FormatException('One-time offer has a recurring interval.');
    }
    final introductoryJson = json['introductory_price'];
    if (introductoryJson != null && introductoryJson is! Map) {
      throw const FormatException('Invalid Commercial introductory price.');
    }
    final introductory = introductoryJson == null
        ? null
        : CommercialIntroductoryPrice.fromJson(
            (introductoryJson as Map).cast<String, dynamic>(),
          );
    final baseAmount = _minorAmount(json['base_amount_minor']);
    if (introductory != null &&
        (billingType != CommercialBillingType.recurring ||
            introductory.amountMinor >= baseAmount)) {
      throw const FormatException('Invalid Commercial introductory price.');
    }
    if (json['eligible'] is! bool) {
      throw const FormatException('Invalid Commercial offer eligibility.');
    }
    final eligible = json['eligible'] as bool;
    final ineligibleValue = json['ineligible_reason'];
    if (ineligibleValue != null && ineligibleValue is! String) {
      throw const FormatException('Invalid Commercial ineligibility reason.');
    }
    final ineligibleReason = ineligibleValue as String?;
    if ((eligible && ineligibleReason != null) ||
        (!eligible &&
            (ineligibleReason == null ||
                ineligibleReason.trim().isEmpty ||
                ineligibleReason.length > 200))) {
      throw const FormatException('Inconsistent Commercial offer eligibility.');
    }
    final entitlementJson = json['entitlement'];
    if (entitlementJson is! Map) {
      throw const FormatException('Missing Commercial offer entitlement.');
    }
    return CommercialOffer(
      offerId: offerId,
      kind: CommercialOfferKind.parse(json['kind']),
      title: title,
      description: description,
      currency: currency,
      baseAmountMinor: baseAmount,
      minorUnitExponent: exponent,
      billingType: billingType,
      recurringInterval: recurringInterval,
      recurringIntervalCount: recurringCount,
      introductoryPrice: introductory,
      purchaseAction: CommercialPurchaseAction.parse(json['purchase_action']),
      eligible: eligible,
      ineligibleReason: ineligibleReason,
      entitlement: CommercialOfferEntitlement.fromJson(
        entitlementJson.cast<String, dynamic>(),
      ),
    );
  }

  final String offerId;
  final CommercialOfferKind kind;
  final String title;
  final String? description;
  final String currency;
  final BigInt baseAmountMinor;
  final int minorUnitExponent;
  final CommercialBillingType billingType;
  final CommercialBillingInterval? recurringInterval;
  final int? recurringIntervalCount;
  final CommercialIntroductoryPrice? introductoryPrice;
  final CommercialPurchaseAction purchaseAction;
  final bool eligible;
  final String? ineligibleReason;
  final CommercialOfferEntitlement entitlement;

  CommercialMoney get basePrice => CommercialMoney(
    currency: currency,
    amountMinor: baseAmountMinor,
    minorUnitExponent: minorUnitExponent,
  );

  CommercialMoney? get introductoryMoney {
    final introductory = introductoryPrice;
    if (introductory == null) return null;
    return CommercialMoney(
      currency: currency,
      amountMinor: introductory.amountMinor,
      minorUnitExponent: minorUnitExponent,
    );
  }
}

class CommercialOfferCatalog {
  const CommercialOfferCatalog({
    required this.offers,
    required this.stale,
    required this.refreshedAt,
  });

  factory CommercialOfferCatalog.fromJson(Map<String, dynamic> json) {
    _requireOnlyKeys(json, const {
      'protocol_version',
      'stale',
      'refreshed_at',
      'offers',
    }, 'Commercial catalog');
    if (json['protocol_version'] != 1) {
      throw const FormatException('Unsupported Commercial catalog protocol.');
    }
    if (json['stale'] is! bool) {
      throw const FormatException('Invalid Commercial catalog stale state.');
    }
    final refreshedAt = _rfc3339Utc(json['refreshed_at']);
    final rawOffers = json['offers'];
    if (rawOffers is! List) {
      throw const FormatException('Commercial catalog has no offers list.');
    }
    if (rawOffers.length > 100) {
      throw const FormatException('Commercial catalog has too many offers.');
    }
    final offers = <CommercialOffer>[];
    final offerIds = <String>{};
    for (final rawOffer in rawOffers) {
      if (rawOffer is! Map) {
        throw const FormatException('Invalid Commercial catalog offer.');
      }
      final offer = CommercialOffer.fromJson(rawOffer.cast<String, dynamic>());
      if (!offerIds.add(offer.offerId)) {
        throw const FormatException(
          'Commercial catalog has duplicate offer identifiers.',
        );
      }
      offers.add(offer);
    }
    return CommercialOfferCatalog(
      offers: List.unmodifiable(offers),
      stale: json['stale'] as bool,
      refreshedAt: refreshedAt,
    );
  }

  final List<CommercialOffer> offers;
  final bool stale;
  final DateTime? refreshedAt;
}

String _requiredString(Object? value, String field) {
  if (value is! String || value.trim().isEmpty) {
    throw FormatException('Missing Commercial offer $field.');
  }
  return value;
}

BigInt _minorAmount(Object? value) {
  if (value is! String || !RegExp(r'^\d{1,38}$').hasMatch(value)) {
    throw const FormatException('Invalid exact Commercial amount.');
  }
  return BigInt.parse(value);
}

int _positiveInt(Object? value, String field) {
  final parsed = _nonNegativeInt(value, field);
  if (parsed == 0) throw FormatException('$field must be positive.');
  return parsed;
}

int _nonNegativeInt(Object? value, String field) {
  if (value is! int || value < 0) {
    throw FormatException('$field must be a non-negative integer.');
  }
  return value;
}

DateTime? _rfc3339Utc(Object? value) {
  if (value == null) return null;
  if (value is! String ||
      !RegExp(
        r'^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z$',
      ).hasMatch(value)) {
    throw const FormatException(
      'Commercial catalog refreshed_at must be an RFC 3339 UTC string.',
    );
  }
  final parsed = DateTime.tryParse(value);
  if (parsed == null || !parsed.isUtc) {
    throw const FormatException(
      'Commercial catalog refreshed_at must be an RFC 3339 UTC string.',
    );
  }
  return parsed;
}

void _requireOnlyKeys(
  Map<String, dynamic> json,
  Set<String> allowed,
  String objectName,
) {
  final unknown = json.keys.where((key) => !allowed.contains(key)).toList();
  if (unknown.isNotEmpty) {
    throw FormatException(
      '$objectName contains unknown field ${unknown.first}.',
    );
  }
}
