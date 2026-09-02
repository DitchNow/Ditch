import 'package:flutter/foundation.dart';

@immutable
class RuntimeStatusDto {
  const RuntimeStatusDto({
    required this.identity,
    required this.pid,
    required this.socketPath,
    required this.activeSessionCount,
    required this.attentionCount,
    required this.unreadAttentionCount,
    required this.instanceId,
    required this.capabilities,
    this.codexHome,
    this.codexBinary,
    this.buildVersion = '',
    this.edition = '',
    this.deploymentEnvironment = '',
    this.buildIdentifier = '',
    this.buildNumber = '',
    this.releaseSequence = 0,
    this.communityRevision,
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
      unreadAttentionCount: body['unread_attention_count'] is int
          ? body['unread_attention_count'] as int
          : integer('attention_count'),
      instanceId: string('instance_id'),
      codexHome: body['codex_home']?.toString(),
      codexBinary: body['codex_binary']?.toString(),
      buildVersion: body['build_version']?.toString() ?? '',
      edition: body['edition']?.toString() ?? '',
      deploymentEnvironment: body['deployment_environment']?.toString() ?? '',
      buildIdentifier: body['build_identifier']?.toString() ?? '',
      buildNumber: body['build_number']?.toString() ?? '',
      releaseSequence: body['release_sequence'] is int
          ? body['release_sequence'] as int
          : 0,
      communityRevision: body['community_revision']?.toString(),
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
  final int unreadAttentionCount;
  final String instanceId;
  final String? codexHome;
  final String? codexBinary;
  final String buildVersion;
  final String edition;
  final String deploymentEnvironment;
  final String buildIdentifier;
  final String buildNumber;
  final int releaseSequence;
  final String? communityRevision;
  final Set<String> capabilities;

  bool get supportsPersistentSessions =>
      capabilities.contains('persistent_sessions_v1');

  bool get supportsAlwaysOnWebAccess =>
      capabilities.contains('always_on_web_access_v1');
}
