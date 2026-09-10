import 'dart:math';
import 'package:flutter/foundation.dart';
import '../data/task_models.dart';
import 'skills_controller.dart';

typedef TaskTransport = Future<Map<String, dynamic>> Function(Object request);

class TaskBoardController extends ChangeNotifier {
  TaskBoardController({required this.request});
  final TaskTransport request;
  late final skills = SkillsController(request: request);
  final Map<String, TaskDto> _tasks = {};
  final Map<String, Map<String, dynamic>> agents = {};
  bool hydrated = false, busy = false;
  String? error;
  Map<String, dynamic>? _retry;
  bool _disposed = false;
  int _generation = 0;
  final Set<String> _deleted = {};
  bool get canRetry => _retry != null;
  List<TaskDto> get tasks => _tasks.values.toList()
    ..sort((a, b) {
      final order = a.orderKey.compareTo(b.orderKey);
      return order == 0 ? a.id.compareTo(b.id) : order;
    });
  TaskDto? task(String id) => _tasks[id];
  void _notify() {
    if (!_disposed) notifyListeners();
  }

  void replaceSnapshot(Map<String, dynamic> snapshot) {
    _generation++;
    final parsed = [
      for (final value in snapshot['tasks'] as List? ?? const [])
        TaskDto.fromJson(Map<String, dynamic>.from(value as Map)),
    ];
    _tasks
      ..clear()
      ..addEntries(parsed.map((t) => MapEntry(t.id, t)));
    agents
      ..clear()
      ..addEntries([
        for (final value in snapshot['agents'] as List? ?? const [])
          MapEntry(
            (value as Map)['id'] as String,
            Map<String, dynamic>.from(value),
          ),
      ]);
    hydrated = true;
    _notify();
  }

  void applyEvent(Map<String, dynamic> event) {
    _generation++;
    if (event.containsKey('SkillsChanged')) skills.invalidate();
    if (event['TaskChanged'] case final Map value) {
      _upsert(TaskDto.fromJson(Map<String, dynamic>.from(value)));
    }
    if (event['TaskDeleted'] case final Map value) {
      _tasks.remove(value['task_id']);
      _deleted.add(value['task_id'] as String);
    }
    if (event['AgentChanged'] case final Map value) {
      agents[value['id'] as String] = Map<String, dynamic>.from(value);
    }
    if (event['AgentDeleted'] case final Map value) {
      agents.remove(value['agent_id']);
    }
    if (event['ProjectDeleted'] case final Map value) {
      _tasks.removeWhere((_, t) => t.projectId == value['project_id']);
    }
    _notify();
  }

  void _upsert(TaskDto task) {
    if (_deleted.contains(task.id)) return;
    if ((_tasks[task.id]?.revision ?? 0) <= task.revision) {
      _tasks[task.id] = task;
    }
  }

  static String newRequestId() {
    final random = Random.secure();
    final bytes = List<int>.generate(16, (_) => random.nextInt(256));
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    final hex = bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
    return '${hex.substring(0, 8)}-${hex.substring(8, 12)}-${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}';
  }

  Future<Map<String, dynamic>> _send(Map<String, dynamic> payload) async {
    final response = await request({'TaskRequest': payload});
    if (response['Error'] case final Map failure) {
      final message = failure['message'] as String;
      if (failure['code'] == 'remote_unavailable') {
        throw FormatException(message);
      }
      throw TaskRequestException(failure['code'] as String, message);
    }
    final body = response['TaskResponse'];
    if (body is! Map) {
      throw const FormatException(
        'This runtime does not support the task board. Restart with the matching runtime.',
      );
    }
    if (body['Error'] case final Map failure) {
      throw TaskRequestException(
        failure['code'] as String,
        failure['message'] as String,
      );
    }
    return Map<String, dynamic>.from(body);
  }

  Future<void> refresh({bool preserveError = false}) async {
    final generation = _generation;
    try {
      final response = await _send({
        'project_id': null,
        'request_id': newRequestId(),
        'operation': {
          'List': {'column': null, 'include_archived': true},
        },
      });
      final values = (response['Tasks'] as List)
          .map((v) => TaskDto.fromJson(Map<String, dynamic>.from(v as Map)))
          .toList();
      if (generation == _generation) {
        _tasks
          ..clear()
          ..addEntries(
            values
                .where((t) => !_deleted.contains(t.id))
                .map((t) => MapEntry(t.id, t)),
          );
      } else {
        for (final task in values) {
          _upsert(task);
        }
      }
      hydrated = true;
      if (!preserveError) error = null;
    } on Object catch (e) {
      error = '$e';
    }
    _notify();
  }

  Future<Map<String, dynamic>?> mutate(
    String projectId,
    Map<String, dynamic> operation,
  ) async {
    if (busy) return null;
    return _perform({
      'project_id': projectId,
      'request_id': newRequestId(),
      'operation': operation,
    });
  }

  Future<Map<String, dynamic>?> retry() async {
    final pending = _retry;
    if (pending == null || busy) return null;
    return _perform(pending);
  }

  Future<Map<String, dynamic>?> _perform(Map<String, dynamic> payload) async {
    busy = true;
    error = null;
    _retry = null;
    _notify();
    try {
      final result = await _send(payload);
      if (result['Changed'] case final Map value) {
        _upsert(TaskDto.fromJson(Map<String, dynamic>.from(value)));
      }
      if (result['Deleted'] case final String id) {
        _tasks.remove(id);
        _deleted.add(id);
      }
      return result;
    } on TaskRequestException catch (e) {
      error = e.message;
      if (e.code == 'RevisionConflict') await refresh(preserveError: true);
      return null;
    } on Object catch (e) {
      error = '$e';
      _retry = payload;
      return null;
    } finally {
      busy = false;
      _notify();
    }
  }

  Future<List<TaskAuditDto>> history(TaskDto task) async {
    final result = await _send({
      'project_id': task.projectId,
      'request_id': newRequestId(),
      'operation': {
        'Get': {'task_id': task.id},
      },
    });
    final detail = Map<String, dynamic>.from(result['Detail'] as Map);
    _upsert(TaskDto.fromJson(Map<String, dynamic>.from(detail['task'] as Map)));
    return [
      for (final v in detail['history'] as List)
        TaskAuditDto.fromJson(Map<String, dynamic>.from(v as Map)),
    ];
  }

  @override
  void dispose() {
    skills.dispose();
    _disposed = true;
    super.dispose();
  }
}
