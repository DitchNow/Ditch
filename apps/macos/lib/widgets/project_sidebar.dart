import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../application/project_sidebar_controller.dart';
import '../data/project_models.dart';
import '../design_system/ditch_theme.dart';
import '../release_ui_visibility.dart';

class ProjectSidebar extends StatefulWidget {
  const ProjectSidebar({
    required this.width,
    required this.projects,
    required this.controller,
    required this.remoteHostStatus,
    required this.selectedProjectKey,
    required this.onAddProject,
    required this.onSelectProject,
    required this.onRevealProject,
    required this.onCopyProjectPath,
    required this.onReconnectRemoteProject,
    required this.onDeleteProject,
    required this.summaryForProject,
    this.onSetupRemoteProject,
    this.onOpenBoard,
    this.onOpenSkills,
    this.editionNavigation = const [],
    this.skillsSelected = false,
    this.boardSelected = false,
    super.key,
  });

  final double width;
  final List<DitchProject> projects;
  final ProjectSidebarController controller;
  final Map<String, String> remoteHostStatus;
  final String? selectedProjectKey;
  final VoidCallback onAddProject;
  final ValueChanged<DitchProject> onSelectProject;
  final ValueChanged<DitchProject> onRevealProject;
  final ValueChanged<DitchProject> onCopyProjectPath;
  final ValueChanged<DitchProject> onReconnectRemoteProject;
  final ValueChanged<DitchProject>? onSetupRemoteProject;
  final ValueChanged<DitchProject> onDeleteProject;
  final ProjectAgentSummary Function(DitchProject) summaryForProject;
  final VoidCallback? onOpenBoard;
  final VoidCallback? onOpenSkills;
  final List<Widget> editionNavigation;
  final bool skillsSelected;
  final bool boardSelected;

  @override
  State<ProjectSidebar> createState() => _ProjectSidebarState();
}

class _ProjectSidebarState extends State<ProjectSidebar> {
  final _search = TextEditingController();
  final _focus = FocusNode();
  bool _searchOpen = false;

  ProjectSidebarController get controller => widget.controller;

  @override
  void initState() {
    super.initState();
    _search.text = controller.value.query;
    controller.addListener(_changed);
    _focus.addListener(_focusChanged);
  }

  @override
  void didUpdateWidget(ProjectSidebar oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.controller != controller) {
      oldWidget.controller.removeListener(_changed);
      controller.addListener(_changed);
      _search.text = controller.value.query;
    }
  }

  void _changed() {
    if (_search.text != controller.value.query) {
      _search.value = TextEditingValue(
        text: controller.value.query,
        selection: TextSelection.collapsed(
          offset: controller.value.query.length,
        ),
      );
    }
    setState(() {});
  }

  void _focusChanged() {
    if (!_focus.hasFocus && controller.value.query.isEmpty) {
      setState(() => _searchOpen = false);
    }
  }

  @override
  void dispose() {
    controller.removeListener(_changed);
    _focus.removeListener(_focusChanged);
    _focus.dispose();
    _search.dispose();
    super.dispose();
  }

  Future<void> _editGroup([ProjectGroup? group]) async {
    final name = await showDialog<String>(
      context: context,
      builder: (_) => _GroupNameDialog(initialName: group?.name),
    );
    if (!mounted || name == null) return;
    if (group == null) {
      controller.createGroup(name);
    } else {
      controller.renameGroup(group.id, name);
    }
  }

  Future<void> _moveProject(DitchProject project) async {
    final choice = await showDialog<String>(
      context: context,
      builder: (context) => SimpleDialog(
        title: Text('Move ${project.name}'),
        children: [
          for (final group in controller.value.groups)
            SimpleDialogOption(
              onPressed: () => Navigator.pop(context, group.id),
              child: Row(
                children: [
                  const Icon(Icons.folder_outlined, size: 16),
                  const SizedBox(width: 8),
                  Expanded(child: Text(group.name)),
                  if (controller.groupFor(project) == group.id)
                    const Icon(Icons.check, size: 16),
                ],
              ),
            ),
          SimpleDialogOption(
            onPressed: () => Navigator.pop(context, ''),
            child: const Text('New Group…'),
          ),
        ],
      ),
    );
    if (!mounted || choice == null) return;
    if (choice.isNotEmpty) {
      controller.moveProject(project, choice);
    } else {
      final name = await showDialog<String>(
        context: context,
        builder: (_) => const _GroupNameDialog(),
      );
      if (!mounted || name == null) return;
      controller.moveProject(project, controller.createGroup(name));
    }
  }

  Widget _searchControls() {
    final tokens = context.ditch;
    final searchHint = switch (controller.value.scope) {
      ProjectScope.all => 'Search all projects',
      ProjectScope.local => 'Search local projects',
      ProjectScope.remote => 'Search remote projects',
    };
    final expanded = _searchOpen || controller.value.query.isNotEmpty;
    if (expanded) {
      return CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.escape): () {
            controller.setQuery('');
            _focus.unfocus();
            setState(() => _searchOpen = false);
          },
        },
        child: TextField(
          key: const Key('project-search-field'),
          controller: _search,
          focusNode: _focus,
          autofocus: _searchOpen,
          onChanged: controller.setQuery,
          onTapOutside: (_) => _focus.unfocus(),
          style: const TextStyle(fontSize: 12),
          decoration: InputDecoration(
            hintText: searchHint,
            contentPadding: const EdgeInsets.symmetric(
              horizontal: 8,
              vertical: 9,
            ),
            prefixIcon: const Icon(Icons.search, size: 16),
            prefixIconConstraints: const BoxConstraints(minWidth: 28),
            suffixIconConstraints: const BoxConstraints(
              minWidth: 28,
              minHeight: 30,
            ),
            suffixIcon: IconButton(
              key: const Key('clear-project-search'),
              tooltip: 'Clear search',
              constraints: const BoxConstraints.tightFor(width: 28, height: 30),
              padding: EdgeInsets.zero,
              icon: const Icon(Icons.close, size: 14),
              onPressed: () {
                controller.setQuery('');
                _focus.unfocus();
                setState(() => _searchOpen = false);
              },
            ),
          ),
        ),
      );
    }
    return SizedBox(
      height: 32,
      child: Row(
        children: [
          Expanded(
            child: Tooltip(
              message: searchHint,
              child: InkWell(
                key: const Key('open-project-search'),
                borderRadius: BorderRadius.circular(tokens.radiusSmall),
                onTap: () => setState(() => _searchOpen = true),
                child: Container(
                  height: 32,
                  decoration: BoxDecoration(
                    color: tokens.controlBackground,
                    border: Border.all(color: tokens.line),
                    borderRadius: BorderRadius.circular(tokens.radiusSmall),
                  ),
                  child: LayoutBuilder(
                    builder: (context, constraints) => Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        Icon(Icons.search, size: 16, color: tokens.muted),
                        if (constraints.maxWidth >= 64) ...[
                          const SizedBox(width: 4),
                          Flexible(
                            child: Text(
                              'Search',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 11,
                                color: tokens.muted,
                              ),
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(width: 6),
          SizedBox(
            width: widget.width < 200 ? 102 : 124,
            child: SegmentedButton<ProjectScope>(
              expandedInsets: EdgeInsets.zero,
              key: const Key('project-scope'),
              showSelectedIcon: false,
              segments: const [
                ButtonSegment(
                  value: ProjectScope.local,
                  label: FittedBox(fit: BoxFit.scaleDown, child: Text('Local')),
                ),
                ButtonSegment(
                  value: ProjectScope.remote,
                  label: FittedBox(
                    fit: BoxFit.scaleDown,
                    child: Text('Remote'),
                  ),
                ),
              ],
              emptySelectionAllowed: true,
              selected: controller.value.scope == ProjectScope.all
                  ? <ProjectScope>{}
                  : {controller.value.scope},
              onSelectionChanged: (selection) => controller.setScope(
                selection.firstOrNull ?? ProjectScope.all,
              ),
              style: ButtonStyle(
                visualDensity: VisualDensity.compact,
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                minimumSize: const WidgetStatePropertyAll(Size(0, 32)),
                padding: WidgetStatePropertyAll(
                  EdgeInsets.symmetric(horizontal: widget.width < 200 ? 5 : 9),
                ),
                textStyle: WidgetStatePropertyAll(
                  Theme.of(context).textTheme.labelSmall?.copyWith(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                side: WidgetStatePropertyAll(BorderSide(color: tokens.line)),
                backgroundColor: WidgetStateProperty.resolveWith(
                  (states) => states.contains(WidgetState.selected)
                      ? tokens.surfaceRaised
                      : tokens.sidebar,
                ),
                foregroundColor: WidgetStateProperty.resolveWith(
                  (states) => states.contains(WidgetState.selected)
                      ? tokens.ink
                      : tokens.muted,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _project(
    DitchProject project, {
    required ProjectAgentSummary summary,
    bool indented = false,
  }) {
    return Padding(
      key: ValueKey('sidebar-project-${projectKey(project)}'),
      padding: EdgeInsets.only(left: indented ? 10 : 0, bottom: 4),
      child: ProjectTile(
        identity: projectKey(project),
        name: project.name,
        path: project.path,
        isRemote: project.isRemote,
        sshHostAlias: project.sshHostAlias,
        remoteStatus:
            widget.remoteHostStatus[project.sshHostAlias] ?? 'connecting',
        selected: widget.selectedProjectKey == projectKey(project),
        onTap: () => widget.onSelectProject(project),
        onReveal: () => widget.onRevealProject(project),
        onCopyPath: () => widget.onCopyProjectPath(project),
        onReconnect: project.isRemote
            ? () => widget.onReconnectRemoteProject(project)
            : null,
        onSetup: project.isRemote && widget.onSetupRemoteProject != null
            ? () => widget.onSetupRemoteProject!(project)
            : null,
        onDelete: () => widget.onDeleteProject(project),
        onMoveToGroup: () => _moveProject(project),
        onRemoveFromGroup: controller.groupFor(project) == null
            ? null
            : () => controller.moveProject(project, null),
        runningCount: summary.runningCount,
        stoppedCount: summary.stoppedCount,
        hasUnreadResult: summary.hasUnreadResult,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final state = controller.value;
    final summaries = {
      for (final project in widget.projects)
        projectKey(project): widget.summaryForProject(project),
    };
    final activeProjectKeys = summaries.entries
        .where((entry) => entry.value.runningCount > 0)
        .map((entry) => entry.key)
        .toSet();
    final matches = controller.matchingProjects(
      widget.projects,
      activeProjectKeys: activeProjectKeys,
    );
    Widget projectRow(DitchProject project, {bool indented = false}) =>
        _project(
          project,
          summary: summaries[projectKey(project)]!,
          indented: indented,
        );
    int activeCount(List<DitchProject> projects) => projects
        .where((project) => activeProjectKeys.contains(projectKey(project)))
        .length;
    final searching = state.query.trim().isNotEmpty;
    final rows = <Widget>[];
    for (final group in state.groups) {
      final members = matches
          .where((p) => controller.groupFor(p) == group.id)
          .toList();
      final empty = !widget.projects.any(
        (p) => controller.groupFor(p) == group.id,
      );
      if (members.isEmpty &&
          !(empty &&
              !state.activeOnly &&
              (!searching ||
                  group.name.toLowerCase().contains(
                    state.query.trim().toLowerCase(),
                  )))) {
        continue;
      }
      final expanded = searching || !state.collapsed.contains(group.id);
      rows.add(
        _ProjectGroupRow(
          key: ValueKey('project-group-${group.id}'),
          group: group,
          count: members.length,
          activeProjectCount: activeCount(members),
          expanded: expanded,
          onToggle: searching ? null : () => controller.toggleGroup(group.id),
          onRename: () => _editGroup(group),
          onDelete: () => controller.deleteGroup(group.id),
        ),
      );
      if (expanded) {
        if (empty) {
          rows.add(
            Padding(
              padding: const EdgeInsets.fromLTRB(22, 3, 4, 8),
              child: Text(
                'No projects yet',
                style: TextStyle(fontSize: 11, color: context.ditch.muted),
              ),
            ),
          );
        }
        rows.addAll(members.map((p) => projectRow(p, indented: true)));
      }
    }
    final ungrouped = matches
        .where((p) => controller.groupFor(p) == null)
        .toList();
    if (state.groups.isEmpty) {
      rows.addAll(ungrouped.map(projectRow));
    } else if (ungrouped.isNotEmpty) {
      final expanded =
          searching || !state.collapsed.contains(otherProjectsGroup.id);
      rows.add(
        _ProjectGroupRow(
          key: ValueKey('project-group-${otherProjectsGroup.id}'),
          group: otherProjectsGroup,
          count: ungrouped.length,
          activeProjectCount: activeCount(ungrouped),
          expanded: expanded,
          onToggle: searching
              ? null
              : () => controller.toggleGroup(otherProjectsGroup.id),
        ),
      );
      if (expanded) {
        rows.addAll(ungrouped.map((p) => projectRow(p, indented: true)));
      }
    }

    return ColoredBox(
      color: context.ditch.sidebar,
      child: SizedBox(
        width: widget.width,
        child: SafeArea(
          top: false,
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ...widget.editionNavigation,
                if (showUnfinishedFeatureControls &&
                    widget.onOpenBoard != null) ...[
                  ListTile(
                    dense: true,
                    selected: widget.boardSelected,
                    leading: const Icon(Icons.view_kanban_outlined),
                    title: const Text('Board'),
                    onTap: widget.onOpenBoard,
                  ),
                  if (widget.onOpenSkills != null)
                    ListTile(
                      dense: true,
                      selected: widget.skillsSelected,
                      leading: const Icon(Icons.extension_outlined),
                      title: const Text('Skills'),
                      onTap: widget.onOpenSkills,
                    ),
                  const SizedBox(height: 12),
                ],
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Projects (${matches.length})',
                        key: const Key('project-result-count'),
                        style: Theme.of(context).textTheme.labelSmall?.copyWith(
                          color: context.ditch.mutedText,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    IconButton(
                      key: const Key('project-active-filter'),
                      tooltip: state.activeOnly
                          ? 'Show all projects (active filter on)'
                          : 'Show only projects with active agents',
                      isSelected: state.activeOnly,
                      onPressed: () =>
                          controller.setActiveOnly(!state.activeOnly),
                      style: IconButton.styleFrom(
                        minimumSize: const Size(24, 24),
                        maximumSize: const Size(24, 24),
                        padding: EdgeInsets.zero,
                        foregroundColor: state.activeOnly
                            ? context.ditch.accentDark
                            : context.ditch.muted,
                        backgroundColor: state.activeOnly
                            ? context.ditch.accentSoft
                            : Colors.transparent,
                      ),
                      icon: const Icon(Icons.monitor_heart_outlined, size: 15),
                    ),
                    PopupMenuButton<String>(
                      key: const Key('project-organization-menu'),
                      tooltip: 'Organize projects',
                      padding: EdgeInsets.zero,
                      constraints: const BoxConstraints(minWidth: 140),
                      iconSize: 16,
                      style: IconButton.styleFrom(
                        minimumSize: const Size(28, 28),
                        maximumSize: const Size(28, 28),
                      ),
                      icon: const Icon(Icons.more_horiz),
                      onSelected: (_) => _editGroup(),
                      itemBuilder: (_) => const [
                        PopupMenuItem(value: 'new', child: Text('New Group…')),
                      ],
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                AnimatedSize(
                  duration: const Duration(milliseconds: 140),
                  alignment: Alignment.topLeft,
                  child: _searchControls(),
                ),
                const SizedBox(height: 10),
                if (state.persistenceError)
                  TextButton(
                    onPressed: controller.retrySave,
                    child: const Text(
                      'Couldn’t save groups. Retry',
                      style: TextStyle(fontSize: 11),
                    ),
                  ),
                Expanded(
                  child: rows.isEmpty
                      ? Align(
                          alignment: Alignment.topLeft,
                          child: Padding(
                            padding: const EdgeInsets.symmetric(vertical: 12),
                            child: Text(
                              searching
                                  ? 'No matching projects'
                                  : state.activeOnly
                                  ? 'No projects with active agents'
                                  : state.scope == ProjectScope.all
                                  ? 'No projects'
                                  : 'No ${state.scope.name} projects',
                              key: const Key('project-empty-state'),
                              style: TextStyle(
                                fontSize: 12,
                                color: context.ditch.muted,
                              ),
                            ),
                          ),
                        )
                      : ListView.builder(
                          itemCount: rows.length,
                          itemBuilder: (_, index) => rows[index],
                        ),
                ),
                const SizedBox(height: 8),
                TextButton.icon(
                  onPressed: widget.onAddProject,
                  style: TextButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 4),
                    textStyle: Theme.of(context).textTheme.labelLarge,
                  ),
                  icon: const Icon(Icons.add, size: 16),
                  label: const Text('Add Project'),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _GroupNameDialog extends StatefulWidget {
  const _GroupNameDialog({this.initialName});
  final String? initialName;
  @override
  State<_GroupNameDialog> createState() => _GroupNameDialogState();
}

class _GroupNameDialogState extends State<_GroupNameDialog> {
  late final _name = TextEditingController(text: widget.initialName);
  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _submit() {
    if (_name.text.trim().isNotEmpty) Navigator.pop(context, _name.text.trim());
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.initialName == null ? 'New Group' : 'Rename Group'),
    content: SizedBox(
      width: 280,
      child: TextField(
        key: const Key('project-group-name'),
        controller: _name,
        autofocus: true,
        maxLength: 80,
        decoration: const InputDecoration(labelText: 'Group name'),
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) => _submit(),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: _name.text.trim().isEmpty ? null : _submit,
        child: Text(widget.initialName == null ? 'Create' : 'Save'),
      ),
    ],
  );
}

class _ProjectGroupRow extends StatelessWidget {
  const _ProjectGroupRow({
    required this.group,
    required this.count,
    required this.activeProjectCount,
    required this.expanded,
    required this.onToggle,
    this.onRename,
    this.onDelete,
    super.key,
  });
  final ProjectGroup group;
  final int count;
  final int activeProjectCount;
  final bool expanded;
  final VoidCallback? onToggle;
  final VoidCallback? onRename;
  final VoidCallback? onDelete;

  Future<void> _menu(BuildContext context, Offset position) async {
    final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
    final action = await showMenu<String>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(position.dx, position.dy, 0, 0),
        Offset.zero & overlay.size,
      ),
      items: const [
        PopupMenuItem(value: 'rename', child: Text('Rename Group…')),
        PopupMenuItem(value: 'delete', child: Text('Delete Group')),
      ],
    );
    if (action == 'rename') onRename?.call();
    if (action == 'delete') onDelete?.call();
  }

  @override
  Widget build(BuildContext context) => Semantics(
    button: true,
    expanded: expanded,
    label: '${group.name}, $count projects',
    child: InkWell(
      onTap: onToggle,
      onSecondaryTapDown: onRename == null && onDelete == null
          ? null
          : (details) => _menu(context, details.globalPosition),
      borderRadius: BorderRadius.circular(context.ditch.radiusSmall),
      child: SizedBox(
        height: 34,
        child: Row(
          children: [
            Icon(
              expanded ? Icons.keyboard_arrow_down : Icons.keyboard_arrow_right,
              size: 16,
              color: context.ditch.muted,
            ),
            const SizedBox(width: 2),
            Icon(
              expanded ? Icons.folder_open_outlined : Icons.folder_outlined,
              size: 15,
              color: context.ditch.muted,
            ),
            const SizedBox(width: 6),
            Expanded(
              child: Text(
                group.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
            const SizedBox(width: 5),
            Text(
              '$count',
              style: TextStyle(fontSize: 11, color: context.ditch.muted),
            ),
            if (activeProjectCount > 0) ...[
              const SizedBox(width: 8),
              ProjectActivityIndicator(
                key: ValueKey('project-group-activity-${group.id}'),
                count: activeProjectCount,
              ),
            ],
            if (onRename != null && onDelete != null)
              PopupMenuButton<String>(
                tooltip: 'Group actions',
                padding: EdgeInsets.zero,
                iconSize: 14,
                style: IconButton.styleFrom(
                  minimumSize: const Size(24, 28),
                  maximumSize: const Size(24, 28),
                ),
                onSelected: (action) =>
                    action == 'rename' ? onRename?.call() : onDelete?.call(),
                itemBuilder: (_) => const [
                  PopupMenuItem(value: 'rename', child: Text('Rename Group…')),
                  PopupMenuItem(value: 'delete', child: Text('Delete Group')),
                ],
              ),
          ],
        ),
      ),
    ),
  );
}

/// Counts projects, not agents. Only the small dot animates.
class ProjectActivityIndicator extends StatefulWidget {
  const ProjectActivityIndicator({required this.count, super.key});
  final int count;

  @override
  State<ProjectActivityIndicator> createState() =>
      _ProjectActivityIndicatorState();
}

class _ProjectActivityIndicatorState extends State<ProjectActivityIndicator>
    with SingleTickerProviderStateMixin {
  late final _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 900),
    value: 1,
  );
  late final _opacity = Tween<double>(
    begin: 0.35,
    end: 1,
  ).chain(CurveTween(curve: Curves.easeInOut)).animate(_pulse);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context)) {
      _pulse.stop();
      _pulse.value = 1;
    } else if (!_pulse.isAnimating) {
      _pulse.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final label = widget.count == 1
        ? '1 project has running agents'
        : '${widget.count} projects have running agents';
    return Tooltip(
      message: label,
      child: Semantics(
        label: label,
        child: ExcludeSemantics(
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                '${widget.count}',
                style: TextStyle(fontSize: 11, color: context.ditch.muted),
              ),
              const SizedBox(width: 4),
              FadeTransition(
                opacity: _opacity,
                child: Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(
                    color: context.ditch.success,
                    shape: BoxShape.circle,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class ProjectTile extends StatelessWidget {
  const ProjectTile({
    this.identity,
    this.onMoveToGroup,
    this.onRemoveFromGroup,
    required this.name,
    required this.path,
    required this.selected,
    required this.onTap,
    required this.onReveal,
    required this.onCopyPath,
    this.onReconnect,
    this.onSetup,
    required this.onDelete,
    this.runningCount = 0,
    this.stoppedCount = 0,
    this.hasUnreadResult = false,
    this.isRemote = false,
    this.sshHostAlias,
    this.remoteStatus,
    super.key,
  });

  final String? identity;
  final VoidCallback? onMoveToGroup;
  final VoidCallback? onRemoveFromGroup;
  final String name;
  final String path;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onReveal;
  final VoidCallback onCopyPath;
  final VoidCallback? onReconnect;
  final VoidCallback? onSetup;
  final VoidCallback onDelete;
  final int runningCount;
  final int stoppedCount;
  final bool hasUnreadResult;
  final bool isRemote;
  final String? sshHostAlias;
  final String? remoteStatus;

  Future<void> _showContextMenu(
    BuildContext context,
    TapDownDetails details,
  ) async {
    final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
    final action = await showMenu<_ProjectMenuAction>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(
          details.globalPosition.dx,
          details.globalPosition.dy,
          0,
          0,
        ),
        Offset.zero & overlay.size,
      ),
      items: [
        const PopupMenuItem(
          value: _ProjectMenuAction.copyPath,
          child: Text('Copy Project Path'),
        ),
        if (onSetup != null)
          const PopupMenuItem(
            value: _ProjectMenuAction.setup,
            child: Text('Remote Runtime Setup…'),
          ),
        if (onReconnect != null)
          const PopupMenuItem(
            value: _ProjectMenuAction.reconnect,
            child: Text('Reconnect'),
          ),
        if (onMoveToGroup != null)
          const PopupMenuItem(
            value: _ProjectMenuAction.moveToGroup,
            child: Text('Move to Group…'),
          ),
        if (onRemoveFromGroup != null)
          const PopupMenuItem(
            value: _ProjectMenuAction.removeFromGroup,
            child: Text('Remove from Group'),
          ),
        const PopupMenuDivider(),
        const PopupMenuItem(
          value: _ProjectMenuAction.delete,
          child: Text('Delete'),
        ),
      ],
    );
    switch (action) {
      case _ProjectMenuAction.moveToGroup:
        onMoveToGroup?.call();
      case _ProjectMenuAction.removeFromGroup:
        onRemoveFromGroup?.call();
      case _ProjectMenuAction.copyPath:
        onCopyPath();
      case _ProjectMenuAction.setup:
        onSetup?.call();
      case _ProjectMenuAction.reconnect:
        onReconnect?.call();
      case _ProjectMenuAction.delete:
        onDelete();
      case null:
        break;
    }
  }

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;

    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 190;
        return InkWell(
          key: ValueKey('project-tile-${identity ?? path}'),
          borderRadius: BorderRadius.circular(tokens.radiusSmall),
          onTap: onTap,
          onSecondaryTapDown: (details) => _showContextMenu(context, details),
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: selected ? tokens.selection : Colors.transparent,
              borderRadius: BorderRadius.circular(tokens.radiusSmall),
            ),
            child: Padding(
              padding: EdgeInsets.all(compact ? 6 : 10),
              child: Row(
                children: [
                  if (!compact) ...[
                    const Icon(Icons.folder_outlined, size: 18),
                    const SizedBox(width: 8),
                  ],
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Expanded(
                              child: Text(
                                name,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                              ),
                            ),
                            if (hasUnreadResult)
                              Tooltip(
                                message: 'New agent result',
                                child: Container(
                                  key: ValueKey('project-unread-$path'),
                                  width: 8,
                                  height: 8,
                                  decoration: BoxDecoration(
                                    color: context.ditch.accent,
                                    shape: BoxShape.circle,
                                  ),
                                ),
                              ),
                          ],
                        ),
                        if (isRemote) ...[
                          const SizedBox(height: 3),
                          const RemoteProjectTag(),
                          const SizedBox(height: 3),
                          Tooltip(
                            message:
                                '$sshHostAlias · ${_titleCase(remoteStatus ?? "connecting")}',
                            child: Text(
                              '$sshHostAlias · ${_titleCase(remoteStatus ?? "connecting")}',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: Theme.of(context).textTheme.bodySmall,
                            ),
                          ),
                        ] else
                          Text(
                            path,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: Theme.of(context).textTheme.bodySmall,
                          ),
                        const SizedBox(height: 3),
                        Text(
                          '$runningCount running · $stoppedCount stopped',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          key: ValueKey('project-counts-$path'),
                          style: Theme.of(context).textTheme.labelSmall
                              ?.copyWith(color: context.ditch.mutedText),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 4),
                  if (isRemote)
                    IconButton(
                      key: ValueKey('reconnect-project-$path'),
                      onPressed: onReconnect,
                      tooltip: 'Reconnect',
                      visualDensity: VisualDensity.compact,
                      constraints: const BoxConstraints.tightFor(
                        width: 28,
                        height: 28,
                      ),
                      padding: EdgeInsets.zero,
                      icon: const Icon(Icons.sync, size: 16),
                    )
                  else
                    IconButton(
                      key: ValueKey('reveal-project-$path'),
                      onPressed: onReveal,
                      tooltip: 'Show in Finder',
                      visualDensity: VisualDensity.compact,
                      constraints: const BoxConstraints.tightFor(
                        width: 28,
                        height: 28,
                      ),
                      padding: EdgeInsets.zero,
                      icon: const Icon(Icons.folder_open_outlined, size: 16),
                    ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

enum _ProjectMenuAction {
  copyPath,
  setup,
  reconnect,
  delete,
  moveToGroup,
  removeFromGroup,
}

String _titleCase(String value) => value.isEmpty
    ? value
    : '${value.substring(0, 1).toUpperCase()}${value.substring(1)}';

class RemoteProjectTag extends StatelessWidget {
  const RemoteProjectTag({super.key});

  @override
  Widget build(BuildContext context) => Container(
    padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
    decoration: BoxDecoration(
      color: context.ditch.accentSoft,
      border: Border.all(color: context.ditch.accent.withValues(alpha: 0.45)),
      borderRadius: BorderRadius.circular(4),
    ),
    child: Text(
      'Remote',
      style: TextStyle(
        color: context.ditch.accentDark,
        fontSize: 10,
        height: 1.3,
        fontWeight: FontWeight.w600,
      ),
    ),
  );
}
