import 'package:flutter/foundation.dart';

enum TaskColumn {
  backlog('Backlog', 'Backlog'),
  todo('Todo', 'Todo'),
  inProgress('InProgress', 'In Progress'),
  inReview('InReview', 'In Review'),
  done('Done', 'Done');

  const TaskColumn(this.wire, this.label);
  final String wire;
  final String label;
}

@immutable
class TaskDto {
  const TaskDto({
    required this.id,
    required this.projectId,
    required this.title,
    required this.description,
    required this.state,
    required this.condition,
    required this.priority,
    required this.criteria,
    required this.orderKey,
    required this.revision,
    required this.archived,
    this.agentId,
    this.reason,
    this.summary,
    this.skills = const [],
    this.acceptance = const {},
    this.githubSource,
  });
  factory TaskDto.fromJson(Map<String, dynamic> json) {
    String requiredString(String key) {
      final value = json[key];
      if (value is! String || value.isEmpty) {
        throw FormatException('Invalid task $key');
      }
      return value;
    }

    final state = requiredString('state');
    if (!const {
      'Draft',
      'Backlog',
      'Ready',
      'Running',
      'Blocked',
      'InReview',
      'Accepted',
      'Rejected',
      'Cancelled',
    }.contains(state)) {
      throw FormatException('Unsupported task state: $state');
    }
    return TaskDto(
      acceptance: Map<String, dynamic>.from(json["acceptance"] as Map? ?? {}),
      githubSource: json['github_source'] is Map
          ? Map<String, dynamic>.from(json['github_source'] as Map)
          : null,
      skills: (json['skills'] as List? ?? [])
          .map((s) => Map<String, dynamic>.from(s as Map))
          .toList(),
      id: requiredString('id'),
      projectId: requiredString('project_id'),
      title: requiredString('title'),
      description: json['description'] as String? ?? '',
      state: state,
      condition:
          json['condition'] as String? ??
          (state == 'Cancelled' ? 'Cancelled' : 'Idle'),
      priority: json['priority'] as String? ?? 'Normal',
      criteria: List<String>.from(
        json['acceptance_criteria'] as List? ?? const [],
      ),
      orderKey: json['order_key'] as int? ?? 0,
      revision: json['revision'] as int? ?? 1,
      archived: json['archived'] == true || state == 'Cancelled',
      agentId: json['assigned_agent_id'] as String?,
      reason: json['last_reason'] as String?,
      summary: json['review_summary'] as String?,
    );
  }
  final String id, projectId, title, description, state, condition, priority;
  final List<String> criteria;
  final List<Map<String, dynamic>> skills;
  final Map<String, dynamic> acceptance;
  final Map<String, dynamic>? githubSource;
  final int orderKey, revision;
  final bool archived;
  final String? agentId, reason, summary;
  TaskColumn get column => switch (state) {
    'Backlog' => TaskColumn.backlog,
    'InReview' => TaskColumn.inReview,
    'Accepted' => TaskColumn.done,
    'Running' || 'Blocked' || 'Rejected' => TaskColumn.inProgress,
    _ => TaskColumn.todo,
  };
  bool get running =>
      const {'Running', 'Queued', 'AwaitingApproval'}.contains(condition);
}

@immutable
class TaskAuditDto {
  const TaskAuditDto({
    required this.action,
    required this.actor,
    required this.createdAt,
    this.reason,
    this.summary,
  });
  factory TaskAuditDto.fromJson(Map<String, dynamic> value) => TaskAuditDto(
    action: value['action'] as String,
    actor: value['actor'] as String,
    createdAt: DateTime.parse(value['created_at'] as String),
    reason: value['reason'] as String?,
    summary: value['summary'] as String?,
  );
  final String action, actor;
  final DateTime createdAt;
  final String? reason, summary;
}

@immutable
class TaskProjectOption {
  const TaskProjectOption(this.id, this.name, {this.remote = false});
  final String id, name;
  final bool remote;
}

class TaskRequestException implements Exception {
  const TaskRequestException(this.code, this.message);
  final String code, message;
  @override
  String toString() => message;
}
