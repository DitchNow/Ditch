import 'package:flutter/foundation.dart';

@immutable
class RuntimeStatusDto {
  const RuntimeStatusDto({
    required this.identity,
    required this.pid,
    required this.socketPath,
    required this.activeSessionCount,
    required this.attentionCount,
    required this.instanceId,
    required this.capabilities,
    this.codexHome,
    this.buildVersion = '',
  });

  factory RuntimeStatusDto.fromResponse(Map<String, dynamic> response) {
    final body = response['RuntimeStatus'];
    if (body is! Map<String, dynamic>) {
      throw const FormatException(
        'runtime status response was missing RuntimeStatus',
      );
    }
    int integer(String key) {
      final value = body[key];
      if (value is int) return value;
      throw FormatException('runtime status field $key was not an integer');
    }

    String string(String key) {
      final value = body[key]?.toString();
      if (value == null || value.isEmpty) {
        throw FormatException('runtime status field $key was missing');
      }
      return value;
    }

    final rawCapabilities = body['capabilities'];
    return RuntimeStatusDto(
      identity: string('identity'),
      pid: integer('pid'),
      socketPath: string('socket_path'),
      activeSessionCount: integer('active_session_count'),
      attentionCount: integer('attention_count'),
      instanceId: string('instance_id'),
      codexHome: body['codex_home']?.toString(),
      buildVersion: body['build_version']?.toString() ?? '',
      capabilities: rawCapabilities is List
          ? rawCapabilities.map((value) => value.toString()).toSet()
          : const {},
    );
  }

  final String identity;
  final int pid;
  final String socketPath;
  final int activeSessionCount;
  final int attentionCount;
  final String instanceId;
  final String? codexHome;
  final String buildVersion;
  final Set<String> capabilities;

  bool get supportsPersistentSessions =>
      capabilities.contains('persistent_sessions_v1');
}
