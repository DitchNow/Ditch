import 'dart:async';
import 'dart:math';
import 'package:flutter/foundation.dart';
import '../data/project_clone_models.dart';
import '../data/runtime_transport.dart';

class ProjectCloneController extends ChangeNotifier {
  ProjectCloneController(
    this.client, {
    this.pollInterval = const Duration(milliseconds: 500),
  });

  final ProjectCloneClient client;
  final Duration pollInterval;
  ProjectCloneStatus? status;
  String? error;
  String? operationId;
  bool active = false;
  bool cancelling = false;
  bool connectionLost = false;
  bool _disposed = false;
  bool _polling = false;
  Timer? _timer;

  String _newId() {
    final random = Random.secure();
    final bytes = List.generate(16, (_) => random.nextInt(256));
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    final hex = bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
    return '${hex.substring(0, 8)}-${hex.substring(8, 12)}-${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}';
  }

  Future<void> start({
    required String url,
    required String parent,
    required String folder,
    required String name,
  }) async {
    if (active || _disposed) return;
    error = validateCloneFields(url, parent, folder, name);
    if (error != null) {
      notifyListeners();
      return;
    }
    active = true;
    cancelling = false;
    connectionLost = false;
    status = null;
    operationId = _newId();
    notifyListeners();
    try {
      _accept(
        await client.startProjectClone(
          operationId: operationId!,
          url: url,
          parent: parent,
          folder: folder,
          name: name,
        ),
      );
    } on Object catch (e) {
      _failure(e);
    }
    _schedule();
  }

  Future<void> retryRegistration() async {
    if (active || operationId == null || status?.checkoutReady != true) return;
    active = true;
    error = null;
    cancelling = false;
    notifyListeners();
    try {
      _accept(await client.retryProjectCloneRegistration(operationId!));
    } on Object catch (e) {
      _failure(e);
    }
    _schedule();
  }

  Future<void> cancel() async {
    if (!active || operationId == null || cancelling) return;
    cancelling = true;
    notifyListeners();
    try {
      _accept(await client.cancelProjectClone(operationId!));
    } on Object catch (e) {
      cancelling = false;
      _failure(e);
    }
    _schedule();
  }

  void _accept(ProjectCloneStatus next) {
    if (_disposed) return;
    if (next.operationId != operationId) {
      _failure(
        const FormatException(
          'Runtime returned a different clone operation. Update and restart it.',
        ),
      );
      return;
    }
    // An overlapping status response must not regress a terminal result.
    if (status?.terminal == true && !active) return;
    status = next;
    active = !next.terminal;
    error = next.error;
    connectionLost = false;
    if (!active) _timer?.cancel();
    notifyListeners();
  }

  void _failure(Object failure) {
    if (_disposed) return;
    if (failure is DitchRuntimeException || failure is FormatException) {
      active = false;
      connectionLost = false;
      error = failure.toString();
    } else {
      // Start/cancel may have reached the daemon. Keep the ID and poll instead
      // of starting another clone or pretending cancellation succeeded.
      connectionLost = true;
      error =
          'Connection to Ditch was lost. Reconnecting to check this clone; it may still be running.';
    }
    notifyListeners();
  }

  void _schedule() {
    _timer?.cancel();
    if (_disposed || !active) return;
    _timer = Timer(
      connectionLost ? const Duration(seconds: 2) : pollInterval,
      poll,
    );
  }

  Future<void> poll() async {
    if (_disposed || !active || _polling || operationId == null) return;
    _polling = true;
    try {
      _accept(await client.projectCloneStatus(operationId!));
    } on Object catch (e) {
      _failure(e);
    } finally {
      _polling = false;
      _schedule();
    }
  }

  @override
  void dispose() {
    _disposed = true;
    _timer?.cancel();
    super.dispose();
  }
}
