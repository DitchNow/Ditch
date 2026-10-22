import 'dart:async';
import 'package:flutter/services.dart';
import 'github_inbox.dart';
import 'release_ui_visibility.dart';
import 'skills_directory.dart';
import 'acceptance_review.dart';
import 'package:flutter/material.dart';
import 'application/task_board_controller.dart';
import 'data/task_models.dart';

import 'data/session_title.dart';
import 'design_system/ditch_theme.dart';

const projectWriteDisclosure =
    'Agents assigned to this project and entitled Ditchmaster execution can read, create, modify, rename, and delete its files according to your chosen approval profile. Project setup also prepares .ditch/agents, .ditch/hooks, and .ditch/mcp.';

class TaskBoard extends StatefulWidget {
  const TaskBoard({
    super.key,
    required this.controller,
    required this.projects,
    required this.connected,
    required this.executionProfile,
    required this.onOpenAgent,
    this.initialProjectId,
    this.onProjectChanged,
    this.onRunSelected,
    this.onPause,
  });
  final TaskBoardController controller;
  final List<TaskProjectOption> projects;
  final bool connected;
  final String? initialProjectId;
  final ValueChanged<String?>? onProjectChanged;
  final Future<void> Function(String requestId, List<TaskDto> tasks)?
  onRunSelected;
  final Future<void> Function()? onPause;
  final Map<String, dynamic> Function() executionProfile;
  final ValueChanged<String> onOpenAgent;
  @override
  State<TaskBoard> createState() => _TaskBoardState();
}

class _TaskBoardState extends State<TaskBoard> {
  late String? projectId = widget.initialProjectId;
  Set<String> subprojects = {};
  String? repositoryFilter;
  String? issueRepository;
  bool archived = false;
  bool showGithub = false;
  final selected = <String>{};
  bool dispatching = false;
  String? dispatchError;
  String? runRequestId;
  List<TaskDto>? pendingRun;

  Future<void> runSelected() async {
    final tasks =
        pendingRun ??
        widget.controller.tasks.where((t) => selected.contains(t.id)).toList();
    if (tasks.isEmpty || dispatching) return;
    if (pendingRun == null) {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text('Run ${tasks.length} selected task(s)?'),
          content: SizedBox(
            width: 480,
            child: SingleChildScrollView(
              child: Text(
                'Ditchmaster will use each task\'s saved execution settings, with its current defaults for newly adopted tasks, until ready for review or blocked. Only these tasks are authorized:\n\n${tasks.map((t) => '• ${widget.projects.where((p) => p.id == t.projectId).firstOrNull?.name ?? t.projectId}: ${t.title}').join('\n')}',
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Run selected'),
            ),
          ],
        ),
      );
      if (confirmed != true || !mounted) return;
      pendingRun = tasks;
      runRequestId = TaskBoardController.newRequestId();
    }
    setState(() {
      dispatching = true;
      dispatchError = null;
    });
    try {
      await widget.onRunSelected!(runRequestId!, tasks);
      if (!mounted) return;
      setState(() {
        selected.clear();
        pendingRun = null;
        runRequestId = null;
      });
      await widget.controller.refresh();
    } on Object catch (e) {
      if (mounted) setState(() => dispatchError = '$e');
    } finally {
      if (mounted) setState(() => dispatching = false);
    }
  }

  Future<void> pause() async {
    try {
      await widget.onPause!();
      if (mounted) setState(() => dispatchError = null);
    } on Object catch (e) {
      if (mounted) setState(() => dispatchError = '$e');
    }
  }

  final horizontal = ScrollController();
  @override
  void initState() {
    super.initState();
    unawaited(loadSubprojects());
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && projectId != null) {
        unawaited(widget.controller.loadBoardExecution(projectId!));
      }
    });
  }

  Future<void> loadSubprojects() async {
    final id = projectId;
    if (id == null) return;
    try {
      final members = await widget.controller.boardProjects(id);
      if (mounted && projectId == id) {
        setState(() {
          subprojects = members.toSet();
          repositoryFilter = null;
        });
      }
    } on Object catch (e) {
      if (mounted) setState(() => dispatchError = '$e');
    }
  }

  Future<void> chooseSubprojects() async {
    final parent = projectId;
    if (parent == null) return;
    final members = Set<String>.from(subprojects);
    final result = await showDialog<bool>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, update) => AlertDialog(
          title: const Text('Projects included on this board'),
          content: SizedBox(
            width: 460,
            height: 350,
            child: ListView(
              children: [
                const Text(
                  'Choose registered subprojects. Each task keeps its own execution project.',
                ),
                for (final p in widget.projects.where((p) => p.id != parent))
                  CheckboxListTile(
                    title: Text(p.name),
                    value: members.contains(p.id),
                    onChanged: (v) => update(() {
                      if (v == true) {
                        members.add(p.id);
                      } else {
                        members.remove(p.id);
                      }
                    }),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Save'),
            ),
          ],
        ),
      ),
    );
    if (result != true) return;
    try {
      final saved = await widget.controller.boardProjects(
        parent,
        members: members.toList(),
      );
      if (mounted && projectId == parent) {
        setState(() {
          subprojects = saved.toSet();
          repositoryFilter = null;
          issueRepository = null;
          selected.clear();
        });
      }
    } on Object catch (e) {
      if (mounted) setState(() => dispatchError = '$e');
    }
  }

  @override
  void dispose() {
    horizontal.dispose();
    super.dispose();
  }

  Future<void> inspect(TaskDto task) => showTaskInspector(
    context,
    widget.controller,
    task,
    widget.projects,
    widget.executionProfile,
    widget.onOpenAgent,
  );
  Future<void> move(TaskDto task, TaskColumn column, String? before) async {
    if (before == task.id || !widget.connected || widget.controller.busy) {
      return;
    }
    if (column == task.column) {
      if (projectId == null ||
          subprojects.isNotEmpty ||
          column != TaskColumn.inProgress ||
          !task.waitingInQueue) {
        return;
      }
      await widget.controller.mutate(task.projectId, {
        'ReorderQueue': {
          'task_id': task.id,
          'expected_revision': task.revision,
          'before_id': before,
        },
      });
      return;
    }
    if (column == TaskColumn.inProgress &&
        task.column == TaskColumn.todo &&
        task.coordinatorGroup == null) {
      await widget.controller.mutate(task.projectId, {
        'Enqueue': {
          'task_id': task.id,
          'expected_revision': task.revision,
          'execution_profile': {
            ...widget.executionProfile(),
            'transport': 'AppServer',
          },
        },
      });
      return;
    }
    if (column != task.column &&
        (column == TaskColumn.inProgress ||
            column == TaskColumn.done ||
            column == TaskColumn.inReview ||
            task.column == TaskColumn.inReview ||
            task.column == TaskColumn.done)) {
      await inspect(task);
      return;
    }
    await widget.controller.mutate(task.projectId, {
      'Move': {
        'task_id': task.id,
        'expected_revision': task.revision,
        'column': column.wire,
        'before_id': before,
      },
    });
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.controller,
    builder: (context, _) {
      final c = widget.controller;
      final repositories = {
        for (final t in c.tasks.where((t) => t.githubSource != null))
          t.githubSource!['repository_id'].toString(): t
              .githubSource!['repository']
              .toString(),
      };
      final effectiveRepository = repositories.containsKey(issueRepository)
          ? issueRepository
          : null;
      final tasks = c.tasks
          .where(
            (t) =>
                (projectId == null ||
                    t.projectId == projectId ||
                    subprojects.contains(t.projectId)) &&
                (repositoryFilter == null || t.projectId == repositoryFilter) &&
                (effectiveRepository == null ||
                    t.githubSource?['repository_id'].toString() ==
                        effectiveRepository) &&
                (archived || !t.archived),
          )
          .toList();
      return Row(
        children: [
          if (showGitHubBoardControls)
            Offstage(
              offstage: !showGithub,
              child: SizedBox(
                width: 350,
                child: GitHubInbox(
                  key: ValueKey('$projectId:${subprojects.toList()..sort()}'),
                  request: widget.controller.request,
                  projects: widget.projects
                      .where(
                        (p) =>
                            projectId == null ||
                            p.id == projectId ||
                            subprojects.contains(p.id),
                      )
                      .toList(),
                  onImported: widget.controller.refresh,
                  onOpenTask: (id) {
                    final task = widget.controller.task(id);
                    if (task != null) inspect(task);
                  },
                ),
              ),
            ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.all(18),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Wrap(
                    spacing: 12,
                    runSpacing: 8,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      Text(
                        'Board',
                        style: Theme.of(context).textTheme.headlineSmall,
                      ),
                      SizedBox(
                        width: 190,
                        child: DropdownButtonHideUnderline(
                          child: DropdownButton<String>(
                            isExpanded: true,
                            value: projectId ?? '',
                            items: [
                              const DropdownMenuItem(
                                value: '',
                                child: Text('All projects'),
                              ),
                              ...widget.projects.map(
                                (p) => DropdownMenuItem(
                                  value: p.id,
                                  child: Text(
                                    p.name,
                                    overflow: TextOverflow.ellipsis,
                                  ),
                                ),
                              ),
                            ],
                            onChanged: (id) {
                              setState(() {
                                projectId = id == '' ? null : id;
                                subprojects = {};
                                repositoryFilter = null;
                                issueRepository = null;
                                selected.clear();
                              });
                              unawaited(loadSubprojects());
                              if (projectId != null) {
                                unawaited(c.loadBoardExecution(projectId!));
                              }
                              widget.onProjectChanged?.call(projectId);
                            },
                          ),
                        ),
                      ),
                      if (projectId != null)
                        Tooltip(
                          message:
                              c.boardExecutionErrors[projectId] ??
                              (c.boardExecutions[projectId]?.paused == true
                                  ? 'Resume this project’s queue'
                                  : 'Pause this project’s queue after the current task finishes'),
                          child: OutlinedButton.icon(
                            key: const ValueKey('board-execution-toggle'),
                            onPressed:
                                !widget.connected ||
                                    c.busy ||
                                    c.executionLoading(projectId!)
                                ? null
                                : c.boardExecutionErrors.containsKey(projectId)
                                ? () => c.loadBoardExecution(projectId!)
                                : c.boardExecutions.containsKey(projectId)
                                ? () => c.toggleBoardExecution(projectId!)
                                : null,
                            icon: c.executionLoading(projectId!)
                                ? const SizedBox(
                                    width: 16,
                                    height: 16,
                                    child: CircularProgressIndicator(
                                      strokeWidth: 2,
                                    ),
                                  )
                                : Icon(
                                    c.boardExecutionErrors.containsKey(
                                          projectId,
                                        )
                                        ? Icons.refresh
                                        : c
                                                  .boardExecutions[projectId]
                                                  ?.paused ==
                                              true
                                        ? Icons.play_arrow
                                        : Icons.pause,
                                    size: 18,
                                  ),
                            label: Text(
                              c.boardExecutionErrors.containsKey(projectId)
                                  ? 'Retry status'
                                  : c.executionLoading(projectId!)
                                  ? 'Loading…'
                                  : c.boardExecutions[projectId]?.paused == true
                                  ? 'Play'
                                  : 'Pause',
                            ),
                          ),
                        ),
                      OutlinedButton.icon(
                        onPressed: () =>
                            setState(() => showGithub = !showGithub),
                        style: showGithub
                            ? OutlinedButton.styleFrom(
                                backgroundColor: context.ditch.surfaceSoft,
                              )
                            : null,
                        icon: Icon(
                          showGithub
                              ? Icons.view_sidebar
                              : Icons.view_sidebar_outlined,
                          size: 18,
                        ),
                        label: const Text('GitHub'),
                      ),
                      if (showUnfinishedFeatureControls &&
                          widget.onRunSelected != null &&
                          (selected.isNotEmpty || pendingRun != null)) ...[
                        FilledButton.icon(
                          onPressed: !widget.connected || dispatching || c.busy
                              ? null
                              : runSelected,
                          icon: const Icon(Icons.play_arrow, size: 18),
                          label: Text(
                            pendingRun != null
                                ? 'Retry Run request'
                                : 'Run selected (${selected.length})',
                          ),
                        ),
                        if (pendingRun != null && !dispatching)
                          TextButton(
                            onPressed: () => setState(() {
                              pendingRun = null;
                              runRequestId = null;
                              selected.clear();
                            }),
                            child: const Text('Clear selection'),
                          ),
                      ],
                      FilledButton.icon(
                        key: const Key('new-task'),
                        onPressed:
                            !widget.connected ||
                                c.busy ||
                                widget.projects.isEmpty
                            ? null
                            : () => showTaskEditor(
                                context,
                                c,
                                widget.projects,
                                projectId: projectId,
                              ),
                        icon: const Icon(Icons.add, size: 18),
                        label: const Text('New task'),
                      ),
                      PopupMenuButton<String>(
                        tooltip: 'Board options',
                        icon: const Icon(Icons.more_horiz),
                        onSelected: (value) {
                          if (value == 'refresh') {
                            unawaited(c.refresh());
                          } else if (value == 'subprojects') {
                            unawaited(chooseSubprojects());
                          } else if (value == 'pause') {
                            unawaited(pause());
                          } else if (value == 'archived') {
                            setState(() {
                              archived = !archived;
                              selected.clear();
                            });
                          } else if (value.startsWith('repo:')) {
                            setState(() {
                              issueRepository = value.substring(5).isEmpty
                                  ? null
                                  : value.substring(5);
                              selected.clear();
                            });
                          } else if (value.startsWith('project:')) {
                            setState(() {
                              repositoryFilter = value.substring(8).isEmpty
                                  ? null
                                  : value.substring(8);
                              selected.clear();
                            });
                          }
                        },
                        itemBuilder: (_) => [
                          PopupMenuItem(
                            value: 'refresh',
                            enabled: !c.busy,
                            child: const Text('Refresh board'),
                          ),
                          CheckedPopupMenuItem(
                            value: 'archived',
                            checked: archived,
                            child: const Text('Show archived'),
                          ),
                          if (projectId != null)
                            const PopupMenuItem(
                              value: 'subprojects',
                              child: Text('Include subprojects…'),
                            ),
                          if (showUnfinishedFeatureControls &&
                              widget.onRunSelected != null)
                            PopupMenuItem(
                              value: 'pause',
                              enabled: widget.connected,
                              child: const Text('Pause queued work'),
                            ),
                          if (subprojects.isNotEmpty) ...[
                            const PopupMenuDivider(),
                            CheckedPopupMenuItem(
                              value: 'project:',
                              checked: repositoryFilter == null,
                              child: const Text('All included projects'),
                            ),
                            for (final p in widget.projects.where(
                              (p) =>
                                  p.id == projectId ||
                                  subprojects.contains(p.id),
                            ))
                              CheckedPopupMenuItem(
                                value: 'project:${p.id}',
                                checked: repositoryFilter == p.id,
                                child: Text(p.name),
                              ),
                          ],
                          if (repositories.isNotEmpty) ...[
                            const PopupMenuDivider(),
                            CheckedPopupMenuItem(
                              value: 'repo:',
                              checked: effectiveRepository == null,
                              child: const Text('All repositories'),
                            ),
                            for (final entry in repositories.entries)
                              CheckedPopupMenuItem(
                                value: 'repo:${entry.key}',
                                checked: effectiveRepository == entry.key,
                                child: Text(entry.value),
                              ),
                          ],
                        ],
                      ),
                    ],
                  ),
                  if (archived ||
                      subprojects.isNotEmpty ||
                      repositoryFilter != null ||
                      effectiveRepository != null)
                    Padding(
                      padding: const EdgeInsets.only(top: 8),
                      child: Wrap(
                        spacing: 8,
                        runSpacing: 4,
                        children: [
                          if (archived)
                            InputChip(
                              label: const Text('Archived included'),
                              onDeleted: () => setState(() {
                                archived = false;
                                selected.clear();
                              }),
                            ),
                          if (subprojects.isNotEmpty)
                            ActionChip(
                              label: Text('${subprojects.length} subprojects'),
                              onPressed: chooseSubprojects,
                            ),
                          if (repositoryFilter != null)
                            InputChip(
                              label: Text(
                                widget.projects
                                        .where((p) => p.id == repositoryFilter)
                                        .firstOrNull
                                        ?.name ??
                                    'Project filter',
                              ),
                              onDeleted: () => setState(() {
                                repositoryFilter = null;
                                selected.clear();
                              }),
                            ),
                          if (effectiveRepository != null)
                            InputChip(
                              label: Text(repositories[effectiveRepository]!),
                              onDeleted: () => setState(() {
                                issueRepository = null;
                                selected.clear();
                              }),
                            ),
                        ],
                      ),
                    ),
                  if (!widget.connected)
                    const Padding(
                      padding: EdgeInsets.symmetric(vertical: 8),
                      child: Text(
                        'Reconnecting to Ditch Runtime. Task changes are unavailable until connected.',
                      ),
                    ),
                  if (c.error != null) TaskErrorBanner(controller: c),
                  if (dispatchError != null)
                    Text(
                      dispatchError!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  if (c.busy) const LinearProgressIndicator(),
                  const SizedBox(height: 12),
                  Expanded(
                    child: !c.hydrated
                        ? const Center(child: CircularProgressIndicator())
                        : Scrollbar(
                            controller: horizontal,
                            thumbVisibility: true,
                            child: SingleChildScrollView(
                              controller: horizontal,
                              scrollDirection: Axis.horizontal,
                              child: Row(
                                crossAxisAlignment: CrossAxisAlignment.stretch,
                                children: [
                                  for (final column in TaskColumn.values)
                                    Padding(
                                      padding: const EdgeInsets.only(
                                        right: 12,
                                        bottom: 14,
                                      ),
                                      child: SizedBox(
                                        width: 280,
                                        child: DragTarget<Object>(
                                          key: ValueKey(
                                            'board-column-${column.name}',
                                          ),
                                          onWillAcceptWithDetails: (d) {
                                            if (!widget.connected || c.busy) {
                                              return false;
                                            }
                                            if (d.data is GitHubIssueDrag) {
                                              return column ==
                                                      TaskColumn.backlog &&
                                                  (d.data as GitHubIssueDrag)
                                                      .canDrop();
                                            }
                                            if (d.data is! TaskDto) {
                                              return false;
                                            }
                                            final task = d.data as TaskDto;
                                            return !task.running &&
                                                !task.archived &&
                                                (task.column != column ||
                                                    (projectId != null &&
                                                        subprojects.isEmpty &&
                                                        column ==
                                                            TaskColumn
                                                                .inProgress &&
                                                        task.waitingInQueue)) &&
                                                (column != TaskColumn.done ||
                                                    task.column ==
                                                        TaskColumn.inReview ||
                                                    task.column ==
                                                        TaskColumn.done);
                                          },
                                          onAcceptWithDetails: (d) {
                                            if (d.data is GitHubIssueDrag) {
                                              unawaited(
                                                (d.data as GitHubIssueDrag)
                                                    .importToBacklog(),
                                              );
                                            } else if (d.data is TaskDto) {
                                              unawaited(
                                                move(
                                                  d.data as TaskDto,
                                                  column,
                                                  null,
                                                ),
                                              );
                                            }
                                          },
                                          builder: (context, candidates, rejected) => AnimatedContainer(
                                            duration: const Duration(
                                              milliseconds: 160,
                                            ),
                                            decoration: BoxDecoration(
                                              color: candidates.isEmpty
                                                  ? context.ditch.surfaceSoft
                                                  : context.ditch.selection,
                                              border: Border.all(
                                                color: candidates.isEmpty
                                                    ? Colors.transparent
                                                    : context.ditch.accent,
                                                width: 2,
                                              ),
                                              borderRadius:
                                                  BorderRadius.circular(12),
                                            ),
                                            child: Column(
                                              crossAxisAlignment:
                                                  CrossAxisAlignment.stretch,
                                              children: [
                                                Padding(
                                                  padding: const EdgeInsets.all(
                                                    14,
                                                  ),
                                                  child: Text(
                                                    candidates.isEmpty
                                                        ? '${column.label}  ${tasks.where((t) => t.column == column).length}'
                                                        : 'Drop into ${column.label}',
                                                    style: Theme.of(
                                                      context,
                                                    ).textTheme.titleSmall,
                                                  ),
                                                ),
                                                Expanded(
                                                  child: Builder(
                                                    builder: (context) {
                                                      final cards = tasks
                                                          .where(
                                                            (t) =>
                                                                t.column ==
                                                                column,
                                                          )
                                                          .toList();
                                                      if (column ==
                                                              TaskColumn
                                                                  .inProgress &&
                                                          projectId != null &&
                                                          subprojects.isEmpty) {
                                                        cards.sort((a, b) {
                                                          if (a.running !=
                                                              b.running) {
                                                            return a.running
                                                                ? -1
                                                                : 1;
                                                          }
                                                          if (a.queuePosition ==
                                                                  null ||
                                                              b.queuePosition ==
                                                                  null) {
                                                            return a.queuePosition ==
                                                                    null
                                                                ? (b.queuePosition ==
                                                                          null
                                                                      ? 0
                                                                      : 1)
                                                                : -1;
                                                          }
                                                          return a
                                                              .queuePosition!
                                                              .compareTo(
                                                                b.queuePosition!,
                                                              );
                                                        });
                                                      }
                                                      if (cards.isEmpty) {
                                                        return Center(
                                                          child: Text(
                                                            '—',
                                                            style: TextStyle(
                                                              color: context
                                                                  .ditch
                                                                  .mutedText,
                                                            ),
                                                          ),
                                                        );
                                                      }
                                                      return ListView.builder(
                                                        padding:
                                                            const EdgeInsets.fromLTRB(
                                                              8,
                                                              0,
                                                              8,
                                                              8,
                                                            ),
                                                        itemCount: cards.length,
                                                        itemBuilder: (context, index) {
                                                          final task =
                                                              cards[index];
                                                          final project = widget
                                                              .projects
                                                              .where(
                                                                (p) =>
                                                                    p.id ==
                                                                    task.projectId,
                                                              )
                                                              .firstOrNull;
                                                          Widget
                                                          card() => TaskCard(
                                                            task: task,
                                                            queuePaused:
                                                                c
                                                                    .boardExecutions[task
                                                                        .projectId]
                                                                    ?.paused ==
                                                                true,
                                                            hasAgent: c.agents
                                                                .containsKey(
                                                                  task.selectedAgentId,
                                                                ),
                                                            showQueueOrder:
                                                                projectId !=
                                                                    null &&
                                                                subprojects
                                                                    .isEmpty &&
                                                                column ==
                                                                    TaskColumn
                                                                        .inProgress &&
                                                                task.executionQueue !=
                                                                    null,
                                                            projectName:
                                                                project?.name ??
                                                                'Unavailable project',
                                                            onOpen: () =>
                                                                inspect(task),
                                                            onMove: (column) =>
                                                                move(
                                                                  task,
                                                                  column,
                                                                  null,
                                                                ),
                                                            onMoveUp:
                                                                index == 0 ||
                                                                    projectId ==
                                                                        null ||
                                                                    subprojects
                                                                        .isNotEmpty ||
                                                                    !task
                                                                        .waitingInQueue ||
                                                                    !cards[index -
                                                                            1]
                                                                        .waitingInQueue
                                                                ? null
                                                                : () => move(
                                                                    task,
                                                                    column,
                                                                    cards[index -
                                                                            1]
                                                                        .id,
                                                                  ),
                                                          );
                                                          return DragTarget<
                                                            TaskDto
                                                          >(
                                                            onWillAcceptWithDetails: (d) =>
                                                                widget
                                                                    .connected &&
                                                                !c.busy &&
                                                                !d
                                                                    .data
                                                                    .running &&
                                                                !d
                                                                    .data
                                                                    .archived &&
                                                                (d.data.column !=
                                                                        column ||
                                                                    (projectId !=
                                                                            null &&
                                                                        subprojects
                                                                            .isEmpty &&
                                                                        column ==
                                                                            TaskColumn.inProgress &&
                                                                        d
                                                                            .data
                                                                            .waitingInQueue &&
                                                                        task.waitingInQueue)) &&
                                                                (column !=
                                                                        TaskColumn
                                                                            .done ||
                                                                    d.data.column ==
                                                                        TaskColumn
                                                                            .inReview ||
                                                                    d.data.column ==
                                                                        TaskColumn
                                                                            .done) &&
                                                                d.data.id !=
                                                                    task.id &&
                                                                d.data.projectId ==
                                                                    task.projectId,
                                                            onAcceptWithDetails:
                                                                (d) =>
                                                                    unawaited(
                                                                      move(
                                                                        d.data,
                                                                        column,
                                                                        task.id,
                                                                      ),
                                                                    ),
                                                            builder: (context, incoming, _) => Padding(
                                                              padding:
                                                                  const EdgeInsets.only(
                                                                    bottom: 8,
                                                                  ),
                                                              child: Column(
                                                                children: [
                                                                  if (incoming
                                                                      .isNotEmpty)
                                                                    const Divider(
                                                                      thickness:
                                                                          3,
                                                                    ),
                                                                  if (showUnfinishedFeatureControls &&
                                                                      widget.onRunSelected !=
                                                                          null &&
                                                                      task.column ==
                                                                          TaskColumn
                                                                              .todo &&
                                                                      !task
                                                                          .running &&
                                                                      !task
                                                                          .archived)
                                                                    CheckboxListTile(
                                                                      dense:
                                                                          true,
                                                                      title: const Text(
                                                                        'Select for Run',
                                                                      ),
                                                                      value: selected
                                                                          .contains(
                                                                            task.id,
                                                                          ),
                                                                      onChanged:
                                                                          pendingRun !=
                                                                                  null ||
                                                                              dispatching
                                                                          ? null
                                                                          : (
                                                                              v,
                                                                            ) => setState(() {
                                                                              if (v ==
                                                                                  true) {
                                                                                selected.add(
                                                                                  task.id,
                                                                                );
                                                                              } else {
                                                                                selected.remove(
                                                                                  task.id,
                                                                                );
                                                                              }
                                                                            }),
                                                                    ),
                                                                  Draggable<
                                                                    TaskDto
                                                                  >(
                                                                    data: task,
                                                                    maxSimultaneousDrags:
                                                                        task.running ||
                                                                            task.archived ||
                                                                            !widget.connected
                                                                        ? 0
                                                                        : 1,
                                                                    feedback: Transform.rotate(
                                                                      angle:
                                                                          -.025,
                                                                      child: Material(
                                                                        elevation:
                                                                            14,
                                                                        borderRadius:
                                                                            BorderRadius.circular(
                                                                              10,
                                                                            ),
                                                                        child: SizedBox(
                                                                          width:
                                                                              260,
                                                                          child:
                                                                              card(),
                                                                        ),
                                                                      ),
                                                                    ),
                                                                    childWhenDragging:
                                                                        Opacity(
                                                                          opacity:
                                                                              .25,
                                                                          child:
                                                                              card(),
                                                                        ),
                                                                    child:
                                                                        card(),
                                                                  ),
                                                                ],
                                                              ),
                                                            ),
                                                          );
                                                        },
                                                      );
                                                    },
                                                  ),
                                                ),
                                              ],
                                            ),
                                          ),
                                        ),
                                      ),
                                    ),
                                ],
                              ),
                            ),
                          ),
                  ),
                ],
              ),
            ),
          ),
        ],
      );
    },
  );
}

class TaskCard extends StatelessWidget {
  const TaskCard({
    super.key,
    required this.task,
    required this.projectName,
    required this.onOpen,
    required this.onMove,
    this.onMoveUp,
    this.showQueueOrder = false,
    this.hasAgent = false,
    this.queuePaused = false,
  });
  final TaskDto task;
  final String projectName;
  final VoidCallback onOpen;
  final ValueChanged<TaskColumn> onMove;
  final VoidCallback? onMoveUp;
  final bool showQueueOrder;
  final bool hasAgent;
  final bool queuePaused;
  @override
  Widget build(BuildContext context) => Semantics(
    label:
        '${task.title}, ${task.column.label}, ${task.condition}, $projectName',
    child: Card(
      margin: EdgeInsets.zero,
      color: context.ditch.surface,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(10),
        side: BorderSide(
          color: task.column == TaskColumn.inReview
              ? context.ditch.waiting
              : context.ditch.line,
        ),
      ),
      child: InkWell(
        onTap: onOpen,
        borderRadius: BorderRadius.circular(10),
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      task.title,
                      maxLines: 3,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                  ),
                  if (task.githubSource case final Map source)
                    Tooltip(
                      message:
                          'GitHub: ${source['state']} · ${source['repository']}',
                      child: Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 6),
                        child: Text(
                          '#${source['number']}',
                          style: Theme.of(context).textTheme.labelSmall,
                        ),
                      ),
                    ),
                  if (showQueueOrder && (task.waitingInQueue || task.running))
                    Tooltip(
                      message: task.waitingInQueue
                          ? 'Drag to change execution order'
                          : 'Execution order is fixed while running',
                      child: Icon(
                        task.waitingInQueue
                            ? Icons.drag_indicator
                            : Icons.lock_outline,
                        size: 16,
                        color: context.ditch.muted,
                      ),
                    ),
                  PopupMenuButton<String>(
                    tooltip: 'Task actions for ${task.title}',
                    onSelected: (value) {
                      if (value == 'open') {
                        onOpen();
                      } else if (value == 'up') {
                        onMoveUp?.call();
                      } else {
                        onMove(
                          TaskColumn.values.firstWhere((c) => c.wire == value),
                        );
                      }
                    },
                    itemBuilder: (_) => [
                      const PopupMenuItem(
                        value: 'open',
                        child: Text('Open task / review'),
                      ),
                      if (onMoveUp != null && !task.running)
                        const PopupMenuItem(
                          value: 'up',
                          child: Text('Move up'),
                        ),
                      if (!task.running && !task.archived)
                        ...TaskColumn.values
                            .where(
                              (c) =>
                                  c != task.column &&
                                  (c != TaskColumn.done ||
                                      task.column == TaskColumn.inReview),
                            )
                            .map(
                              (c) => PopupMenuItem(
                                value: c.wire,
                                child: Text(
                                  c == TaskColumn.done
                                      ? 'Review and Accept…'
                                      : 'Move to ${c.label}…',
                                ),
                              ),
                            ),
                    ],
                  ),
                ],
              ),
              Text(projectName, style: Theme.of(context).textTheme.bodySmall),
              if ((task.waitingInQueue && queuePaused) ||
                  ((task.waitingInQueue || task.queueNeedsRetry) &&
                      task.reason != null))
                Padding(
                  padding: const EdgeInsets.only(top: 4),
                  child: Text(
                    task.waitingInQueue && queuePaused
                        ? 'Board paused'
                        : task.reason!,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ),
              const SizedBox(height: 6),
              Wrap(
                spacing: 8,
                runSpacing: 4,
                children: [
                  if (task.planning['job'] != null ||
                      (task.planning['text'] as String? ?? '').isNotEmpty)
                    _TaskPlanStatus(task: task),
                  if (task.priority != 'Normal')
                    Text(
                      task.priority,
                      style: Theme.of(context).textTheme.labelSmall,
                    ),
                  if (task.archived || task.condition != 'Idle')
                    Text(
                      task.archived
                          ? (task.condition == 'Idle'
                                ? 'Archived'
                                : 'Archived · ${task.condition}')
                          : task.condition,
                      style: Theme.of(context).textTheme.labelSmall,
                    ),
                  if (hasAgent)
                    const Icon(
                      Icons.smart_toy_outlined,
                      size: 16,
                      semanticLabel: 'Assigned agent',
                    ),
                ],
              ),
              if (task.queueNeedsRetry)
                TextButton(
                  onPressed: onOpen,
                  child: const Text('Review & retry'),
                ),
            ],
          ),
        ),
      ),
    ),
  );
}

class _TaskPlanStatus extends StatelessWidget {
  const _TaskPlanStatus({required this.task});
  final TaskDto task;

  @override
  Widget build(BuildContext context) {
    final job = task.planning['job'] as Map?;
    final status = job?['status'];
    final hasPlan = (task.planning['text'] as String? ?? '').trim().isNotEmpty;
    final running = status == 'running';
    final failed = !hasPlan && (status == 'failed' || status == 'interrupted');
    final needsReview =
        !hasPlan && (job?['result'] as String? ?? '').trim().isNotEmpty;
    final label = running
        ? 'Planning…'
        : hasPlan
        ? 'Plan ready'
        : needsReview
        ? 'Review plan'
        : failed
        ? 'Plan failed'
        : status == 'canceled'
        ? 'Plan canceled'
        : 'No plan';
    final color = failed
        ? Theme.of(context).colorScheme.error
        : running || needsReview || (!hasPlan && status == 'canceled')
        ? context.ditch.muted
        : context.ditch.success;
    return Tooltip(
      message: running
          ? 'Generating an implementation plan in the background'
          : failed
          ? 'Open task for details and retry'
          : 'Open task to review its implementation plan',
      child: Semantics(
        liveRegion: true,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
          decoration: BoxDecoration(
            color: color.withValues(alpha: 0.08),
            borderRadius: BorderRadius.circular(6),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (running)
                SizedBox.square(
                  dimension: 12,
                  child: CircularProgressIndicator(
                    strokeWidth: 1.5,
                    color: color,
                  ),
                )
              else
                Icon(
                  failed ? Icons.error_outline : Icons.description_outlined,
                  size: 13,
                  color: color,
                ),
              const SizedBox(width: 5),
              Text(
                label,
                style: Theme.of(
                  context,
                ).textTheme.labelSmall?.copyWith(color: color),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class TaskErrorBanner extends StatelessWidget {
  const TaskErrorBanner({super.key, required this.controller});
  final TaskBoardController controller;
  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 8),
    child: Row(
      children: [
        Expanded(
          child: Text(
            controller.error ?? '',
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          ),
        ),
        TextButton(
          onPressed: controller.busy
              ? null
              : controller.canRetry
              ? controller.retry
              : controller.refresh,
          child: Text(controller.canRetry ? 'Retry action' : 'Refresh'),
        ),
      ],
    ),
  );
}

Future<TaskDto?> showTaskEditor(
  BuildContext context,
  TaskBoardController controller,
  List<TaskProjectOption> projects, {
  String? projectId,
  TaskDto? task,
}) => showDialog<TaskDto>(
  context: context,
  builder: (_) => _TaskEditor(
    controller: controller,
    projects: projects,
    projectId: projectId,
    task: task,
  ),
);

class _TaskEditor extends StatefulWidget {
  const _TaskEditor({
    required this.controller,
    required this.projects,
    this.projectId,
    this.task,
  });
  final TaskBoardController controller;
  final List<TaskProjectOption> projects;
  final String? projectId;
  final TaskDto? task;
  @override
  State<_TaskEditor> createState() => _TaskEditorState();
}

class _TaskEditorState extends State<_TaskEditor> {
  final form = GlobalKey<FormState>();
  late final title = TextEditingController(text: widget.task?.title);
  late final description = TextEditingController(
    text: widget.task?.description,
  );
  late final criteria = TextEditingController(
    text: widget.task?.criteria.join('\n'),
  );
  late String? projectId =
      widget.task?.projectId ??
      widget.projectId ??
      widget.projects.firstOrNull?.id;
  late String priority = widget.task?.priority ?? 'Normal';
  late List<Map<String, dynamic>> skills = widget.task?.skills ?? [];
  Future<void> selectSkills() async {
    if (projectId == null) return;
    final selected = await showSkillPicker(
      context,
      widget.controller.skills,
      projectId!,
      skills,
    );
    if (selected != null && mounted) setState(() => skills = selected);
  }

  @override
  void dispose() {
    title.dispose();
    description.dispose();
    criteria.dispose();
    super.dispose();
  }

  Future<void> save() async {
    if (!form.currentState!.validate() || projectId == null) return;
    final draft = {
      'skills': skills,
      'title': title.text.trim(),
      'description': description.text,
      'acceptance_criteria': criteria.text
          .split('\n')
          .map((s) => s.trim())
          .where((s) => s.isNotEmpty)
          .toList(),
      'priority': priority,
    };
    final task = widget.task;
    final response = await widget.controller.mutate(
      projectId!,
      task == null
          ? {
              'CreateBacklog': {'draft': draft},
            }
          : {
              'Update': {
                'task_id': task.id,
                'expected_revision': task.revision,
                'draft': draft,
              },
            },
    );
    if (response != null && mounted) {
      Navigator.pop(
        context,
        TaskDto.fromJson(Map<String, dynamic>.from(response['Changed'] as Map)),
      );
    }
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.controller,
    builder: (context, _) => AlertDialog(
      title: Text(widget.task == null ? 'New Task' : 'Edit Task'),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(
          child: Form(
            key: form,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                DropdownButtonFormField<String>(
                  initialValue: projectId,
                  decoration: const InputDecoration(labelText: 'Project'),
                  items: widget.projects
                      .map(
                        (p) =>
                            DropdownMenuItem(value: p.id, child: Text(p.name)),
                      )
                      .toList(),
                  onChanged: widget.task != null
                      ? null
                      : (id) => setState(() {
                          projectId = id;
                          skills = [];
                        }),
                ),
                TextFormField(
                  controller: title,
                  autofocus: true,
                  maxLength: 512,
                  decoration: const InputDecoration(labelText: 'Title'),
                  validator: (v) => v == null || v.trim().isEmpty
                      ? 'Enter a task title'
                      : null,
                ),
                TextFormField(
                  controller: description,
                  minLines: 3,
                  maxLines: 8,
                  maxLength: 65536,
                  decoration: const InputDecoration(
                    labelText: 'Description (Markdown supported)',
                  ),
                ),
                TextFormField(
                  controller: criteria,
                  minLines: 2,
                  maxLines: 6,
                  decoration: const InputDecoration(
                    labelText: 'Acceptance criteria',
                    helperText: 'One criterion per line',
                  ),
                ),
                DropdownButtonFormField<String>(
                  initialValue: priority,
                  decoration: const InputDecoration(labelText: 'Priority'),
                  items: ['Low', 'Normal', 'High', 'Urgent']
                      .map((p) => DropdownMenuItem(value: p, child: Text(p)))
                      .toList(),
                  onChanged: (v) => setState(() => priority = v!),
                ),
                if (showUnfinishedFeatureControls)
                  SkillChips(skills: skills, onEdit: selectSkills),
                if (widget.controller.error != null)
                  TaskErrorBanner(controller: widget.controller),
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: widget.controller.busy
              ? null
              : () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: widget.controller.busy ? null : save,
          child: const Text('Save Task'),
        ),
      ],
    ),
  );
}

Future<void> showTaskInspector(
  BuildContext context,
  TaskBoardController controller,
  TaskDto task,
  List<TaskProjectOption> projects,
  Map<String, dynamic> Function() profile,
  ValueChanged<String> onOpenAgent,
) => showDialog<void>(
  context: context,
  barrierDismissible: false,
  builder: (_) => _TaskInspector(
    controller: controller,
    taskId: task.id,
    projects: projects,
    profile: profile,
    onOpenAgent: onOpenAgent,
  ),
);

class _TaskInspector extends StatefulWidget {
  const _TaskInspector({
    required this.controller,
    required this.taskId,
    required this.projects,
    required this.profile,
    required this.onOpenAgent,
  });
  final TaskBoardController controller;
  final String taskId;
  final List<TaskProjectOption> projects;
  final Map<String, dynamic> Function() profile;
  final ValueChanged<String> onOpenAgent;
  @override
  State<_TaskInspector> createState() => _TaskInspectorState();
}

class _TaskInspectorState extends State<_TaskInspector> {
  final feedback = TextEditingController();
  late final TextEditingController description;
  late final TextEditingController plan;
  late String savedDescription, savedPlan;
  String? submittedDescription, submittedPlan;
  bool saving = false;
  bool get dirty =>
      description.text != savedDescription || plan.text != savedPlan;
  late Future<List<TaskAuditDto>> history;
  int? historyRevision;
  bool useAppServer = false;
  @override
  void initState() {
    super.initState();
    final task = widget.controller.task(widget.taskId);
    savedDescription = task?.description ?? '';
    savedPlan = task?.planning['text'] as String? ?? '';
    description = TextEditingController(text: savedDescription);
    plan = TextEditingController(text: savedPlan);
    useAppServer =
        widget.profile()['transport'] == 'AppServer' ||
        (widget.controller.task(widget.taskId)?.skills.isNotEmpty ?? false);
    reload();
    widget.controller.addListener(onTaskChanged);
  }

  void onTaskChanged() {
    if (mounted &&
        widget.controller.task(widget.taskId)?.revision != historyRevision) {
      final task = widget.controller.task(widget.taskId);
      if (task == null) {
        setState(() {});
        return;
      }
      final nextPlan = task.planning['text'] as String? ?? '';
      if (description.text == savedDescription) {
        description.text = task.description;
        savedDescription = task.description;
      } else if (task.description == submittedDescription) {
        savedDescription = task.description;
      }
      if (plan.text == savedPlan) {
        plan.text = nextPlan;
        savedPlan = nextPlan;
      } else if (nextPlan == submittedPlan) {
        savedPlan = nextPlan;
      } else {
        final job = task.planning['job'] as Map?;
        if (job?['status'] == 'completed' &&
            job?['result'] == nextPlan &&
            job?['previous_plan'] == savedPlan) {
          // Rebase an unsaved plan edit onto the AI result without changing it.
          savedPlan = nextPlan;
        }
      }
      setState(reload);
    }
  }

  void reload() {
    final task = widget.controller.task(widget.taskId);
    if (task != null) {
      historyRevision = task.revision;
      history = widget.controller.history(task);
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(onTaskChanged);
    feedback.dispose();
    description.dispose();
    plan.dispose();
    super.dispose();
  }

  Future<bool> saveDescription() async {
    if (!dirty) return true;
    if (saving || widget.controller.busy) return false;
    final task = widget.controller.task(widget.taskId);
    if (task == null) return false;
    setState(() => saving = true);
    submittedDescription = description.text;
    submittedPlan = plan.text;
    final result = await widget.controller.mutate(task.projectId, {
      'UpdateDescription': {
        'task_id': task.id,
        'expected_description': savedDescription,
        'expected_plan': savedPlan,
        'description': submittedDescription,
        'plan': submittedPlan,
      },
    });
    if (!mounted) return false;
    if (result != null) {
      savedDescription = submittedDescription!;
      savedPlan = submittedPlan!;
    }
    submittedDescription = submittedPlan = null;
    setState(() => saving = false);
    return result != null && !dirty;
  }

  Future<void> close() async {
    if (await saveDescription() && mounted) Navigator.pop(context);
  }

  Future<void> openGitHubTaskSource(Map source) async {
    final uri = Uri.tryParse(source['url'] as String? ?? '');
    if (uri == null || uri.scheme != 'https' || uri.host != 'github.com') {
      return;
    }
    try {
      await const MethodChannel(
        'the_ditch/application',
      ).invokeMethod<bool>('openURL', uri.toString());
    } on Object {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(const SnackBar(content: Text('Could not open GitHub.')));
      }
    }
  }

  Future<void> generatePlan() async {
    if (!await saveDescription() || !mounted) return;
    final task = widget.controller.task(widget.taskId)!;
    await widget.controller.mutate(task.projectId, {
      'GeneratePlan': {
        'task_id': task.id,
        'expected_revision': task.revision,
        'model': widget.profile()['model'],
      },
    });
  }

  Future<void> selectSkills(TaskDto task) async {
    final selected = await showSkillPicker(
      context,
      widget.controller.skills,
      task.projectId,
      task.skills,
    );
    if (selected == null) return;
    final result = await widget.controller.skills.call(task.projectId, {
      'Bind': {
        'task_id': task.id,
        'expected_revision': task.revision,
        'skills': selected,
      },
    });
    if (result?['Bound'] case final Map value) {
      widget.controller.applyEvent({'TaskChanged': value});
      if (mounted) {
        setState(() => useAppServer = selected.isNotEmpty || useAppServer);
      }
    }
    if (mounted && widget.controller.skills.error != null) {
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(SnackBar(content: Text(widget.controller.skills.error!)));
    }
  }

  Future<void> transition(TaskDto task, Object action) async {
    if (!await saveDescription() || !mounted) return;
    task = widget.controller.task(task.id)!;
    await widget.controller.mutate(task.projectId, {
      'Transition': {
        'task_id': task.id,
        'expected_revision': task.revision,
        'action': action,
      },
    });
    if (mounted) setState(reload);
  }

  String agentName(Map<String, dynamic>? agent) => agent == null
      ? 'Agent unavailable'
      : sessionDisplayName(
          agent['user_title'] as String?,
          agent['codex_title'] as String?,
          namingFailed: agent['naming_failed'] == true,
        );

  Future<void> selectAgent(TaskDto task, String? id) async {
    if (!await saveDescription() || !mounted) return;
    final c = widget.controller;
    task = c.task(task.id)!;
    final agent = c.agents[id];
    final otherId = agent?['task_id'] as String?;
    final other = otherId == null || otherId == task.id
        ? null
        : c.task(otherId);
    if (other != null) {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Reassign agent?'),
          content: Text(
            'Move this agent from “${other.title}” to “${task.title}”? Its conversation history is kept.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Reassign'),
            ),
          ],
        ),
      );
      if (confirmed != true || !mounted) return;
    }
    await c.mutate(task.projectId, {
      'SelectAgent': {
        'task_id': task.id,
        'expected_revision': task.revision,
        'agent_id': id,
        if (other != null) 'replace_task_id': other.id,
        if (other != null) 'replace_task_revision': other.revision,
      },
    });
    if (mounted) setState(reload);
  }

  Widget agentAssignment(TaskDto task, bool disabled) {
    final c = widget.controller;
    final selected = task.selectedAgentId;
    final agent = c.agents[selected];
    if (task.column != TaskColumn.todo || task.archived) {
      return ListTile(
        contentPadding: EdgeInsets.zero,
        leading: const Icon(Icons.lock_outline, size: 18),
        title: Text(
          selected == null
              ? 'New agent when this task starts'
              : agentName(agent),
        ),
        subtitle: const Text('Move to Todo to change the agent.'),
      );
    }
    final candidates = c.agents.values.where((a) {
      if (a['project_id'] != task.projectId ||
          a['coordinator_group'] != null ||
          a['can_stop'] == true ||
          a['native_session_id'] == null ||
          a['resume_block_reason'] != null ||
          const {
            'Starting',
            'Working',
            'Stopping',
            'AwaitingApproval',
          }.contains(a['state'])) {
        return false;
      }
      final otherId = a['task_id'] as String?;
      if (otherId == null || otherId == task.id) return true;
      final other = c.task(otherId);
      return other != null &&
          other.column == TaskColumn.todo &&
          !other.archived &&
          !other.running &&
          other.coordinatorGroup == null;
    }).toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 8),
        InputDecorator(
          decoration: const InputDecoration(labelText: 'Agent'),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<String>(
              key: const ValueKey('task-agent-selector'),
              value: selected ?? '',
              isExpanded: true,
              items: [
                const DropdownMenuItem(value: '', child: Text('New agent')),
                if (selected != null &&
                    !candidates.any((a) => a['id'] == selected))
                  DropdownMenuItem(
                    value: selected,
                    enabled: false,
                    child: Text(agentName(agent)),
                  ),
                for (final a in candidates)
                  DropdownMenuItem(
                    value: a['id'] as String,
                    child: Text(agentName(a), overflow: TextOverflow.ellipsis),
                  ),
              ],
              onChanged: disabled
                  ? null
                  : (id) {
                      if (id != null && id != (selected ?? '')) {
                        unawaited(selectAgent(task, id.isEmpty ? null : id));
                      }
                    },
            ),
          ),
        ),
        if (candidates.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Text(
              'No stopped conversations available in this project.',
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ),
      ],
    );
  }

  Future<void> start(TaskDto task) async {
    if (!await saveDescription() || !mounted) return;
    task = widget.controller.task(task.id)!;
    final profile = {
      ...widget.profile(),
      'transport':
          (task.coordinatorGroup == null ||
              useAppServer ||
              ((task.acceptance['config'] as Map?)?['criteria'] as List? ?? [])
                  .isNotEmpty ||
              ((task.acceptance['config'] as Map?)?['policy']
                      as Map?)?['enabled'] ==
                  true)
          ? 'AppServer'
          : 'Legacy',
    };
    if (profile['approval'] == 'FullAccess') {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Run task with Full Access?'),
          content: const Text(
            'This disables project containment and requires exclusive writer access. The agent may access files outside this project.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Use Full Access'),
            ),
          ],
        ),
      );
      if (confirmed != true) return;
    }
    await widget.controller.mutate(task.projectId, {
      (task.coordinatorGroup == null ? 'Enqueue' : 'Start'): {
        'task_id': task.id,
        'expected_revision': task.revision,
        'execution_profile': profile,
      },
    });
    if (mounted) setState(reload);
  }

  Future<void> accept(TaskDto task) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Accept this work?'),
        content: const Text(
          'Confirm you reviewed the summary and acceptance criteria. Accept moves the task to Done.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Keep in Review'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Accept'),
          ),
        ],
      ),
    );
    if (confirmed == true) await transition(task, 'Accept');
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.controller,
    builder: (context, _) {
      final c = widget.controller, task = c.task(widget.taskId);
      if (task == null) {
        return AlertDialog(
          title: const Text('Task unavailable'),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Close'),
            ),
          ],
        );
      }
      final agent = c.agents[task.selectedAgentId];
      final running = task.running || agent?['can_stop'] == true;
      final disabled = c.busy || running;
      final editable =
          !disabled &&
          !saving &&
          !task.archived &&
          task.column != TaskColumn.done &&
          task.column != TaskColumn.inReview;
      final job = task.planning['job'] as Map?;
      final generating = job?['status'] == 'running';
      final canSubmitWork =
          !task.waitingInQueue &&
          (task.executionQueue == null || agent != null);
      return PopScope(
        canPop: !dirty && !saving,
        onPopInvokedWithResult: (didPop, _) {
          if (!didPop) unawaited(close());
        },
        child: AlertDialog(
          constraints: const BoxConstraints(maxWidth: 720, maxHeight: 800),
          title: Row(
            children: [
              Expanded(child: Text(task.title)),
              if (!disabled && !task.archived && task.column != TaskColumn.done)
                PopupMenuButton<String>(
                  tooltip: 'Task options',
                  onSelected: (value) async {
                    if (!await saveDescription() || !context.mounted) return;
                    final current = c.task(task.id)!;
                    if (value == 'edit') {
                      await showTaskEditor(
                        context,
                        c,
                        widget.projects,
                        task: current,
                      );
                    } else if (value == 'archive') {
                      await transition(current, {
                        'Cancel': {'reason': 'Cancelled by user'},
                      });
                    } else if (value == 'delete') {
                      final result = await c.mutate(current.projectId, {
                        'Delete': {
                          'task_id': current.id,
                          'expected_revision': current.revision,
                        },
                      });
                      if (result != null && context.mounted) {
                        Navigator.pop(context);
                      }
                    }
                  },
                  itemBuilder: (_) => [
                    if (editable)
                      const PopupMenuItem(
                        value: 'edit',
                        child: Text('Edit task…'),
                      ),
                    if (!disabled &&
                        !task.archived &&
                        task.column != TaskColumn.done)
                      const PopupMenuItem(
                        value: 'archive',
                        child: Text('Archive task'),
                      ),
                    if (!disabled &&
                        task.revision == 1 &&
                        task.agentId == null &&
                        task.githubSource == null)
                      const PopupMenuItem(
                        value: 'delete',
                        child: Text('Delete draft'),
                      ),
                  ],
                ),
            ],
          ),
          content: SizedBox(
            width: 680,
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    children: [
                      Text(
                        task.column.label,
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                      if (task.githubSource case final Map source) ...[
                        const SizedBox(width: 12),
                        Flexible(
                          child: TextButton.icon(
                            onPressed: () => openGitHubTaskSource(source),
                            icon: const Icon(Icons.open_in_new, size: 14),
                            label: Text(
                              '${source['repository']} #${source['number']}',
                              overflow: TextOverflow.ellipsis,
                            ),
                          ),
                        ),
                      ],
                    ],
                  ),
                  if (task.coordinatorGroup == null &&
                      task.column != TaskColumn.backlog)
                    agentAssignment(task, disabled),
                  const SizedBox(height: 16),
                  TextField(
                    key: const ValueKey('task-description'),
                    controller: description,
                    readOnly: !editable,
                    minLines: 7,
                    maxLines: 14,
                    onChanged: (_) => setState(() {}),
                    decoration: const InputDecoration(
                      labelText: 'Description',
                      alignLabelWithHint: true,
                      hintText: 'What needs to be done?',
                    ),
                  ),
                  const SizedBox(height: 12),
                  Row(
                    children: [
                      if (generating) ...[
                        const SizedBox(
                          width: 14,
                          height: 14,
                          child: CircularProgressIndicator(strokeWidth: 2),
                        ),
                        const SizedBox(width: 8),
                        const Expanded(child: Text('Generating plan…')),
                        TextButton(
                          onPressed: c.busy
                              ? null
                              : () => c.mutate(task.projectId, {
                                  'CancelPlan': {
                                    'task_id': task.id,
                                    'job_id': job!['id'],
                                  },
                                }),
                          child: const Text('Cancel'),
                        ),
                      ] else
                        OutlinedButton.icon(
                          onPressed: editable ? generatePlan : null,
                          icon: const Icon(
                            Icons.auto_awesome_outlined,
                            size: 16,
                          ),
                          label: const Text('Generate with AI'),
                        ),
                    ],
                  ),
                  if (job?['error'] case final String error)
                    Padding(
                      padding: const EdgeInsets.only(top: 8),
                      child: Text(
                        error,
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                    ),
                  if (job?['status'] == 'completed' &&
                      (job?['description'] != task.description ||
                          job?['title'] != task.title))
                    const Padding(
                      padding: EdgeInsets.only(top: 8),
                      child: Text(
                        'Task changed. Review or regenerate this plan.',
                      ),
                    ),
                  if (plan.text.isNotEmpty || savedPlan.isNotEmpty) ...[
                    const SizedBox(height: 16),
                    TextField(
                      key: const ValueKey('task-plan'),
                      controller: plan,
                      readOnly: !editable,
                      minLines: 8,
                      maxLines: 18,
                      onChanged: (_) => setState(() {}),
                      decoration: const InputDecoration(
                        labelText: 'Implementation plan',
                        alignLabelWithHint: true,
                      ),
                    ),
                  ],
                  if (job?['result'] is String &&
                      job?['result'] != (task.planning['text'] ?? '') &&
                      !generating)
                    ExpansionTile(
                      title: const Text('Generated plan'),
                      children: [
                        SelectableText(job!['result'] as String),
                        TextButton(
                          onPressed: editable
                              ? () => setState(
                                  () => plan.text = job['result'] as String,
                                )
                              : null,
                          child: const Text('Use this plan'),
                        ),
                      ],
                    ),
                  const SizedBox(height: 12),
                  ExpansionTile(
                    tilePadding: EdgeInsets.zero,
                    title: const Text('Details & options'),
                    initiallyExpanded:
                        task.column == TaskColumn.inReview ||
                        task.column == TaskColumn.inProgress,
                    children: [
                      if (showUnfinishedFeatureControls)
                        SkillChips(
                          skills: task.skills,
                          onEdit: disabled ? null : () => selectSkills(task),
                        ),
                      if (showUnfinishedFeatureControls &&
                          task.coordinatorGroup != null)
                        SwitchListTile(
                          contentPadding: EdgeInsets.zero,
                          title: const Text('Use local App Server'),
                          subtitle: const Text(
                            'Opt-in: interactive approvals and explicit skills. Legacy execution cannot attach skills.',
                          ),
                          value: useAppServer,
                          onChanged: disabled
                              ? null
                              : (v) => setState(() => useAppServer = v),
                        ),

                      if (task.githubSource != null) ...[
                        const SizedBox(height: 12),
                        TextButton(
                          onPressed: () async {
                            try {
                              await githubRequest(c.request, {
                                'Refresh': {'task_id': task.id},
                              });
                              await c.refresh();
                            } on Object catch (e) {
                              c.error = '$e';
                              await c.refresh(preserveError: true);
                            }
                          },
                          child: const Text('Refresh GitHub metadata'),
                        ),
                      ],
                      if (task.criteria.isNotEmpty)
                        Text(
                          'Acceptance criteria',
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                      ...task.criteria.map(
                        (text) => Padding(
                          padding: const EdgeInsets.symmetric(vertical: 4),
                          child: Text('• $text'),
                        ),
                      ),
                      if (task.summary != null)
                        Text(
                          'Review summary',
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                      if (task.summary != null) SelectableText(task.summary!),
                      if (task.reason != null)
                        Padding(
                          padding: const EdgeInsets.only(top: 8),
                          child: SelectableText(task.reason!),
                        ),
                      const SizedBox(height: 12),
                      if (task.column == TaskColumn.inReview ||
                          task.acceptance['latest_submission'] != null)
                        AcceptanceReview(
                          task: task,
                          controller: c,
                          running: running,
                        ),
                      if (widget.projects.any(
                        (p) => p.id == task.projectId && p.remote,
                      ))
                        const Padding(
                          padding: EdgeInsets.only(top: 8),
                          child: Text(
                            'SSH execution uses interactive approvals. Project-root confinement is not currently enforced; approved commands run with the SSH account’s access.',
                          ),
                        ),
                      if (agent != null)
                        TextButton.icon(
                          onPressed: () async {
                            if (!await saveDescription() || !context.mounted) {
                              return;
                            }
                            Navigator.pop(context);
                            widget.onOpenAgent(task.selectedAgentId!);
                          },
                          icon: const Icon(Icons.chat_bubble_outline),
                          label: Text(
                            sessionDisplayName(
                              agent['user_title'] as String?,
                              agent['codex_title'] as String?,
                              namingFailed: agent['naming_failed'] == true,
                            ),
                          ),
                        ),
                      if (!running &&
                          canSubmitWork &&
                          !task.archived &&
                          (task.column == TaskColumn.inProgress ||
                              task.column == TaskColumn.inReview)) ...[
                        const SizedBox(height: 12),
                        TextField(
                          controller: feedback,
                          minLines: 2,
                          maxLines: 5,
                          onChanged: (_) => setState(() {}),
                          decoration: InputDecoration(
                            labelText: task.column == TaskColumn.inReview
                                ? 'Required review feedback'
                                : 'Submission summary',
                            hintText: task.column == TaskColumn.inReview
                                ? 'Describe the changes needed'
                                : 'Describe the work ready for review',
                          ),
                        ),
                      ],
                      const SizedBox(height: 12),
                      ExpansionTile(
                        title: const Text('Transition history'),
                        children: [
                          FutureBuilder<List<TaskAuditDto>>(
                            future: history,
                            builder: (context, snapshot) {
                              if (snapshot.hasError) {
                                return Text(
                                  'History unavailable: ${snapshot.error}',
                                );
                              }
                              if (!snapshot.hasData) {
                                return const LinearProgressIndicator();
                              }
                              return Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  for (final a in snapshot.data!)
                                    ListTile(
                                      dense: true,
                                      title: Text(
                                        '${a.action.replaceAll('_', ' ')} · ${a.actor}',
                                      ),
                                      subtitle: SelectableText(
                                        '${a.createdAt.toLocal()}${a.reason == null ? '' : '\n${a.reason}'}${a.summary == null ? '' : '\n${a.summary}'}',
                                      ),
                                    ),
                                ],
                              );
                            },
                          ),
                        ],
                      ),
                    ],
                  ),
                  if (c.error != null) ...[
                    TaskErrorBanner(controller: c),
                    if (dirty)
                      TextButton(
                        onPressed: saving
                            ? null
                            : () {
                                setState(() {
                                  savedDescription = task.description;
                                  savedPlan =
                                      task.planning['text'] as String? ?? '';
                                  description.text = savedDescription;
                                  plan.text = savedPlan;
                                });
                              },
                        child: const Text('Discard my edits'),
                      ),
                  ],
                ],
              ),
            ),
          ),
          actions: [
            TextButton(
              onPressed: saving || c.busy ? null : close,
              child: Text(dirty ? 'Save & close' : 'Close'),
            ),
            if (task.archived || task.column == TaskColumn.done)
              FilledButton(
                onPressed: disabled ? null : () => transition(task, 'Reopen'),
                child: const Text('Reopen'),
              ),
            if (!task.archived &&
                (task.column == TaskColumn.todo ||
                    task.column == TaskColumn.inProgress))
              FilledButton(
                onPressed: disabled || task.waitingInQueue
                    ? null
                    : () => start(task),
                child: Text(
                  task.waitingInQueue
                      ? 'Queued'
                      : task.coordinatorGroup == null
                      ? (task.queueNeedsRetry ? 'Retry task' : 'Add to queue')
                      : 'Start Task Agent',
                ),
              ),
            if (!task.archived &&
                canSubmitWork &&
                task.column == TaskColumn.inProgress)
              FilledButton(
                onPressed: disabled || feedback.text.trim().isEmpty
                    ? null
                    : () => transition(task, {
                        'Submit': {'summary': feedback.text.trim()},
                      }),
                child: const Text('Submit for Review'),
              ),
            if (!task.archived && task.column == TaskColumn.inReview) ...[
              TextButton(
                onPressed: disabled || feedback.text.trim().isEmpty
                    ? null
                    : () => transition(task, {
                        'RequestChanges': {'feedback': feedback.text.trim()},
                      }),
                child: const Text('Request Changes'),
              ),
              FilledButton(
                onPressed: disabled ? null : () => accept(task),
                child: const Text('Accept…'),
              ),
            ],
          ],
        ),
      );
    },
  );
}
