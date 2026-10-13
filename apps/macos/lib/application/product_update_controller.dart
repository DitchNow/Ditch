import 'dart:async';
import 'dart:math';

import 'package:flutter/foundation.dart';

import '../data/commercial_models.dart';
import '../data/runtime_models.dart';
import '../data/runtime_transport.dart';

typedef ProductUpdateContext = ({
  RuntimeStatusDto runtime,
  DitchCurrentLicense license,
});

/// Availability is independent of notification dismissal and download sessions.
/// Only the runtime's verified discovery responses may populate this controller.
class ProductUpdateController extends ChangeNotifier {
  ProductUpdateController({
    required this.loadContext,
    required this.fetchRelease,
    DateTime Function()? now,
    Duration? pollInterval,
    this.staleAfter = const Duration(minutes: 5),
  }) : _now = now ?? DateTime.now,
       pollInterval =
           pollInterval ?? Duration(minutes: 15, seconds: Random().nextInt(61));

  final Future<ProductUpdateContext> Function() loadContext;
  final Future<Map<String, dynamic>> Function(
    DitchCurrentLicense license, {
    required bool forInstallation,
  })
  fetchRelease;
  final DateTime Function() _now;
  final Duration pollInterval;
  final Duration staleAfter;

  ProductUpdateContext? context;
  Map<String, dynamic>? manifest;
  Object? error;
  DateTime? lastCheckedAt;
  bool noCompatibleRelease = false;
  bool checking = false;
  DateTime? _lastAttempt;
  DateTime? _retryAfter;
  int _failures = 0;
  int _generation = 0;
  bool _disposed = false;
  bool _polling = false;
  Timer? _timer;
  Future<Map<String, dynamic>?>? _inFlight;
  Future<Map<String, dynamic>?>? _installation;

  bool get available => manifest != null;
  String? get releaseId => manifest?['release_id'] as String?;

  void startPolling() {
    if (_disposed || _polling) return;
    _polling = true;
    unawaited(check());
    if (_inFlight == null) _schedulePoll();
  }

  Future<void> check({bool force = false}) async {
    if (_disposed) return;
    final pending = _inFlight;
    if (pending == null && !force) {
      final now = _now();
      if ((_lastAttempt != null &&
              now.difference(_lastAttempt!) < staleAfter) ||
          (_retryAfter != null && now.isBefore(_retryAfter!))) {
        return;
      }
    }
    try {
      await (pending ?? _beginRefresh(forInstallation: false));
    } on Object {
      // Background failures are state for the update dialog, not notifications.
    }
  }

  /// Never reuse a discovery bearer when the user asks to install.
  Future<Map<String, dynamic>?> prepareInstallation() {
    final generation = _generation;
    return _installation ??= Future<Map<String, dynamic>?>.microtask(
      () => _prepareInstallation(generation),
    );
  }

  Future<Map<String, dynamic>?> _prepareInstallation(int generation) async {
    // Join any existing discovery before requesting fresh install permission.
    // Do not start an extra discovery merely because the badge is old.
    try {
      final pending = _inFlight;
      if (pending != null) {
        try {
          await pending;
        } on Object {
          // An explicit install can retry a failed background check.
        }
      }
      if (_disposed || generation != _generation) return null;
      return await _beginRefresh(forInstallation: true);
    } finally {
      if (generation == _generation) _installation = null;
    }
  }

  /// Call when an external action can change the license or runtime identity.
  /// In-flight responses from the previous context must not restore its badge.
  void invalidate() {
    if (_disposed) return;
    _generation++;
    _inFlight = null;
    _installation = null;
    context = null;
    manifest = null;
    error = null;
    checking = false;
    noCompatibleRelease = false;
    lastCheckedAt = null;
    _lastAttempt = null;
    _retryAfter = null;
    _failures = 0;
    notifyListeners();
  }

  void reconcileContext(ProductUpdateContext value) {
    if (_disposed) return;
    if (context != null && _contextKey(context!) != _contextKey(value)) {
      invalidate();
    }
    context = value;
  }

  Object _contextKey(ProductUpdateContext value) => (
    value.runtime.instanceId,
    value.runtime.deploymentEnvironment,
    value.runtime.edition,
    value.runtime.releaseSequence,
    value.license.edition,
    value.license.status,
  );

  Future<Map<String, dynamic>?> _beginRefresh({required bool forInstallation}) {
    final generation = _generation;
    // Queue the work so listeners see an assigned in-flight future immediately.
    final future = Future<Map<String, dynamic>?>.microtask(
      () => _refresh(generation, forInstallation: forInstallation),
    );
    _inFlight = future;
    return future;
  }

  Future<Map<String, dynamic>?> _refresh(
    int generation, {
    required bool forInstallation,
  }) async {
    bool current() => !_disposed && generation == _generation;
    if (!current()) return null;
    checking = true;
    error = null;
    _lastAttempt = _now();
    notifyListeners();
    try {
      final freshContext = await loadContext();
      if (!current()) return null;
      if (context != null &&
          _contextKey(context!) != _contextKey(freshContext)) {
        manifest = null;
        lastCheckedAt = null;
        noCompatibleRelease = false;
      }
      context = freshContext;
      notifyListeners();
      final release = await fetchRelease(
        freshContext.license,
        forInstallation: forInstallation,
      );
      if (!current()) return null;
      final raw = release['manifest'];
      if (raw is! Map ||
          raw['release_id'] is! String ||
          (raw['release_id'] as String).isEmpty ||
          raw['release_sequence'] is! int) {
        throw const FormatException(
          'The Relay returned incomplete update metadata.',
        );
      }
      manifest =
          (raw['release_sequence'] as int) >
              freshContext.runtime.releaseSequence
          ? Map<String, dynamic>.unmodifiable(raw.cast<String, dynamic>())
          : null;
      noCompatibleRelease = false;
      lastCheckedAt = _now();
      _failures = 0;
      _retryAfter = null;
      return manifest == null ? null : release;
    } on Object catch (failure) {
      if (!current()) return null;
      if (failure is DitchRuntimeException &&
          failure.code == 'commercial_release_unavailable') {
        manifest = null;
        noCompatibleRelease = true;
        lastCheckedAt = _now();
        _failures = 0;
        _retryAfter = null;
        return null;
      }
      error = failure;
      _failures = min(_failures + 1, 4);
      _retryAfter = _now().add(_pollDelay);
      rethrow;
    } finally {
      if (current()) {
        checking = false;
        _inFlight = null;
        notifyListeners();
        _schedulePoll();
      }
    }
  }

  Duration get _pollDelay => pollInterval * (1 << max(0, _failures - 1));

  void _schedulePoll() {
    _timer?.cancel();
    if (_disposed || !_polling) return;
    _timer = Timer(_pollDelay, () => unawaited(check(force: true)));
  }

  @override
  void dispose() {
    _disposed = true;
    _generation++;
    _timer?.cancel();
    super.dispose();
  }
}
