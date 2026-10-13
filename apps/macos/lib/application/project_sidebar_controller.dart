import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import '../data/project_models.dart';

enum ProjectScope { all, local, remote }

@immutable
class ProjectGroup {
  const ProjectGroup({required this.id, required this.name});
  final String id;
  final String name;
}

// Presentation-only group: unassigned projects keep their null membership.
const otherProjectsGroup = ProjectGroup(
  id: '__other_projects__',
  name: 'Other Projects',
);

@immutable
class ProjectSidebarState {
  const ProjectSidebarState({
    this.scope = ProjectScope.local,
    this.query = '',
    this.activeOnly = false,
    this.groups = const [],
    this.memberships = const {},
    this.collapsed = const {},
    this.persistenceError = false,
  });

  final ProjectScope scope;
  final String query;
  final bool activeOnly;
  final List<ProjectGroup> groups;
  final Map<String, String> memberships;
  final Set<String> collapsed;
  final bool persistenceError;

  ProjectSidebarState copyWith({
    ProjectScope? scope,
    String? query,
    bool? activeOnly,
    List<ProjectGroup>? groups,
    Map<String, String>? memberships,
    Set<String>? collapsed,
    bool? persistenceError,
  }) => ProjectSidebarState(
    scope: scope ?? this.scope,
    query: query ?? this.query,
    activeOnly: activeOnly ?? this.activeOnly,
    groups: List.unmodifiable(groups ?? this.groups),
    memberships: Map.unmodifiable(memberships ?? this.memberships),
    collapsed: Set.unmodifiable(collapsed ?? this.collapsed),
    persistenceError: persistenceError ?? this.persistenceError,
  );
}

/// Mac-local organization only. Runtime projects and sessions stay authoritative.
class ProjectSidebarController extends ValueNotifier<ProjectSidebarState> {
  ProjectSidebarController({
    ProjectScope initialScope = ProjectScope.local,
    Future<String?> Function()? load,
    Future<void> Function(String)? save,
  }) : _load = load ?? _loadNative,
       _save = save ?? _saveNative,
       super(ProjectSidebarState(scope: initialScope));

  static const _channel = MethodChannel('the_ditch/application');
  final Future<String?> Function() _load;
  final Future<void> Function(String) _save;
  Future<void> _writes = Future.value();
  int _revision = 0;
  int _nextId = 0;
  bool _disposed = false;

  static Future<String?> _loadNative() async {
    try {
      return await _channel.invokeMethod<String>(
        'getProjectSidebarPreferences',
      );
    } on MissingPluginException {
      return null;
    }
  }

  static Future<void> _saveNative(String json) async {
    try {
      await _channel.invokeMethod<void>('setProjectSidebarPreferences', json);
    } on MissingPluginException {
      // Widget tests and non-macOS hosts keep in-memory preferences.
    }
  }

  Future<void> load() async {
    final revision = _revision;
    try {
      final source = await _load();
      if (_disposed || source == null || revision != _revision) return;
      final data = jsonDecode(source);
      if (data is! Map || data['version'] != 1) return;
      final rawGroups = data['groups'];
      final groups = <ProjectGroup>[];
      final ids = <String>{};
      if (rawGroups is List) {
        for (final group in rawGroups) {
          if (group is! Map) continue;
          final id = group['id'];
          final name = group['name'];
          if (id is String &&
              id.isNotEmpty &&
              id != otherProjectsGroup.id &&
              name is String &&
              name.trim().isNotEmpty &&
              ids.add(id)) {
            groups.add(ProjectGroup(id: id, name: name.trim()));
          }
        }
      }
      final rawMemberships = data['memberships'];
      final memberships = <String, String>{};
      if (rawMemberships is Map) {
        for (final entry in rawMemberships.entries) {
          if (entry.key is String &&
              entry.value is String &&
              ids.contains(entry.value)) {
            memberships[entry.key as String] = entry.value as String;
          }
        }
      }
      final rawCollapsed = data['collapsed'];
      value = value.copyWith(
        scope: switch (data['scope']) {
          'all' => ProjectScope.all,
          'remote' => ProjectScope.remote,
          _ => ProjectScope.local,
        },
        groups: groups,
        activeOnly: data['active_only'] == true,
        memberships: memberships,
        collapsed: rawCollapsed is List
            ? rawCollapsed
                  .whereType<String>()
                  .where(
                    (id) =>
                        ids.contains(id) ||
                        (groups.isNotEmpty && id == otherProjectsGroup.id),
                  )
                  .toSet()
            : {},
      );
    } on FormatException {
      // An invalid preference document must not prevent opening projects.
    } on Object {
      if (!_disposed) value = value.copyWith(persistenceError: true);
    }
  }

  void _persist() {
    _revision++;
    final document = jsonEncode({
      'version': 1,
      'scope': value.scope.name,
      'active_only': value.activeOnly,
      'groups': [
        for (final group in value.groups) {'id': group.id, 'name': group.name},
      ],
      'memberships': value.memberships,
      'collapsed': value.collapsed.toList(),
    });
    _writes = _writes.then((_) async {
      try {
        await _save(document);
        if (!_disposed && value.persistenceError) {
          value = value.copyWith(persistenceError: false);
        }
      } on Object {
        if (!_disposed) value = value.copyWith(persistenceError: true);
      }
    });
  }

  Future<void> flush() => _writes;
  void retrySave() => _persist();

  void setQuery(String query) => value = value.copyWith(query: query);

  void setActiveOnly(bool activeOnly) {
    if (activeOnly == value.activeOnly) return;
    value = value.copyWith(activeOnly: activeOnly);
    _persist();
  }

  void setScope(ProjectScope scope) {
    if (scope == value.scope) return;
    value = value.copyWith(scope: scope);
    _persist();
  }

  String createGroup(String name) {
    final trimmed = name.trim();
    if (trimmed.isEmpty) throw ArgumentError.value(name, 'name');
    final id = 'group-${DateTime.now().microsecondsSinceEpoch}-${_nextId++}';
    value = value.copyWith(
      groups: [
        ...value.groups,
        ProjectGroup(id: id, name: trimmed),
      ],
    );
    _persist();
    return id;
  }

  void renameGroup(String id, String name) {
    final trimmed = name.trim();
    if (trimmed.isEmpty || !value.groups.any((g) => g.id == id)) return;
    value = value.copyWith(
      groups: [
        for (final group in value.groups)
          group.id == id ? ProjectGroup(id: id, name: trimmed) : group,
      ],
    );
    _persist();
  }

  void deleteGroup(String id) {
    value = value.copyWith(
      groups: value.groups.where((g) => g.id != id).toList(),
      memberships: Map.of(value.memberships)
        ..removeWhere((_, group) => group == id),
      collapsed: Set.of(value.collapsed)
        ..remove(id)
        ..removeWhere(
          (key) =>
              key == otherProjectsGroup.id &&
              !value.groups.any((group) => group.id != id),
        ),
    );
    _persist();
  }

  String? groupFor(DitchProject project) =>
      value.memberships[projectKey(project)] ??
      value.memberships[projectFallbackKey(project)];

  void moveProject(DitchProject project, String? groupId) {
    if (groupId != null && !value.groups.any((g) => g.id == groupId)) return;
    final memberships = Map.of(value.memberships)
      ..remove(projectFallbackKey(project))
      ..remove(projectKey(project));
    if (groupId != null) memberships[projectKey(project)] = groupId;
    value = value.copyWith(memberships: memberships);
    _persist();
  }

  /// Upgrade provisional path keys when the runtime supplies project IDs.
  /// Do not prune absent projects: an offline SSH host can still own them.
  void reconcileProjects(List<DitchProject> projects) {
    final memberships = Map.of(value.memberships);
    var changed = false;
    for (final project in projects) {
      final fallback = projectFallbackKey(project);
      if (project.id != null && memberships.containsKey(fallback)) {
        memberships.putIfAbsent(project.id!, () => memberships[fallback]!);
        memberships.remove(fallback);
        changed = true;
      }
    }
    if (changed) {
      value = value.copyWith(memberships: memberships);
      _persist();
    }
  }

  void toggleGroup(String id) {
    final collapsed = Set.of(value.collapsed);
    if (!collapsed.remove(id)) collapsed.add(id);
    value = value.copyWith(collapsed: collapsed);
    _persist();
  }

  void revealProject(DitchProject project) {
    final group =
        groupFor(project) ??
        (value.groups.isNotEmpty ? otherProjectsGroup.id : null);
    value = value.copyWith(
      scope: value.scope == ProjectScope.all
          ? ProjectScope.all
          : project.isRemote
          ? ProjectScope.remote
          : ProjectScope.local,
      query: '',
      activeOnly: false,
      collapsed: Set.of(value.collapsed)..remove(group),
    );
    _persist();
  }

  List<DitchProject> matchingProjects(
    List<DitchProject> projects, {
    Set<String> activeProjectKeys = const {},
  }) {
    final query = value.query.trim().toLowerCase();
    final matchingGroups = value.groups
        .where((g) => g.name.toLowerCase().contains(query))
        .map((g) => g.id)
        .toSet();
    final matchesOther =
        value.groups.isNotEmpty &&
        otherProjectsGroup.name.toLowerCase().contains(query);
    return projects.where((project) {
        if (value.activeOnly &&
            !activeProjectKeys.contains(projectKey(project))) {
          return false;
        }
        if (value.scope != ProjectScope.all &&
            project.isRemote != (value.scope == ProjectScope.remote)) {
          return false;
        }
        return query.isEmpty ||
            matchingGroups.contains(groupFor(project)) ||
            (matchesOther && groupFor(project) == null) ||
            [
              project.name,
              project.path,
              project.sshHostAlias ?? '',
            ].any((text) => text.toLowerCase().contains(query));
      }).toList()
      ..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}
