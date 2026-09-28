import 'dart:async';
import 'github_inbox.dart';
import 'skills_directory.dart';
import 'acceptance_review.dart';
import 'package:flutter/material.dart';
import 'application/task_board_controller.dart';
import 'data/task_models.dart';
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
          if (showGithub)
            SizedBox(
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
                      FilterChip(
                        label: const Text('GitHub inbox'),
                        selected: showGithub,
                        onSelected: (v) => setState(() => showGithub = v),
                      ),
                      if (widget.onRunSelected != null) ...[
                        FilledButton.icon(
                          onPressed:
                              !widget.connected ||
                                  dispatching ||
                                  c.busy ||
                                  (selected.isEmpty && pendingRun == null)
                              ? null
                              : runSelected,
                          icon: const Icon(Icons.play_arrow),
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
                        TextButton(
                          onPressed: widget.connected ? pause : null,
                          child: const Text('Pause queued work'),
                        ),
                      ],
                      DropdownButton<String>(
                        value: projectId ?? '',
                        hint: const Text('All projects'),
                        items: [
                          const DropdownMenuItem(
                            value: '',
                            child: Text('All projects'),
                          ),
                          ...widget.projects.map(
                            (p) => DropdownMenuItem(
                              value: p.id,
                              child: Text(p.name),
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
                          widget.onProjectChanged?.call(projectId);
                        },
                      ),
                      if (projectId != null)
                        TextButton(
                          onPressed: chooseSubprojects,
                          child: const Text('Include subprojects'),
                        ),
                      if (subprojects.isNotEmpty)
                        DropdownButton<String>(
                          value: repositoryFilter ?? '',
                          items: [
                            const DropdownMenuItem(
                              value: '',
                              child: Text('All included projects'),
                            ),
                            for (final p in widget.projects.where(
                              (p) =>
                                  p.id == projectId ||
                                  subprojects.contains(p.id),
                            ))
                              DropdownMenuItem(
                                value: p.id,
                                child: Text(p.name),
                              ),
                          ],
                          onChanged: (id) => setState(() {
                            repositoryFilter = id == '' ? null : id;
                            selected.clear();
                          }),
                        ),
                      if (c.tasks.any((t) => t.githubSource != null))
                        DropdownButton<String>(
                          value: effectiveRepository ?? '',
                          items: [
                            const DropdownMenuItem(
                              value: '',
                              child: Text('All repositories'),
                            ),
                            for (final entry in repositories.entries)
                              DropdownMenuItem(
                                value: entry.key,
                                child: Text(entry.value),
                              ),
                          ],
                          onChanged: (v) => setState(() {
                            issueRepository = v == '' ? null : v;
                            selected.clear();
                          }),
                        ),
                      FilterChip(
                        label: const Text('Include archived'),
                        selected: archived,
                        onSelected: (v) => setState(() {
                          archived = v;
                          selected.clear();
                        }),
                      ),
                      IconButton(
                        tooltip: 'Refresh board',
                        onPressed: c.busy ? null : c.refresh,
                        icon: const Icon(Icons.refresh),
                      ),
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
                        icon: const Icon(Icons.add),
                        label: const Text('New Task'),
                      ),
                    ],
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
                  if (widget.onRunSelected != null)
                    const Text(
                      'Backlog is approved scope. To Do is ready work. Only Run selected starts Ditchmaster.',
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
                                        child: DragTarget<TaskDto>(
                                          onWillAcceptWithDetails: (d) =>
                                              widget.connected &&
                                              !c.busy &&
                                              (column != TaskColumn.done ||
                                                  d.data.column ==
                                                      TaskColumn.inReview ||
                                                  d.data.column ==
                                                      TaskColumn.done),
                                          onAcceptWithDetails: (d) => unawaited(
                                            move(d.data, column, null),
                                          ),
                                          builder: (context, candidates, rejected) => DecoratedBox(
                                            decoration: BoxDecoration(
                                              color: candidates.isEmpty
                                                  ? context.ditch.surfaceSoft
                                                  : context.ditch.selection,
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
                                                    '${column.label}  ${tasks.where((t) => t.column == column).length}',
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
                                                      if (cards.isEmpty) {
                                                        return Center(
                                                          child: Text(
                                                            column ==
                                                                    TaskColumn
                                                                        .todo
                                                                ? 'Create a task to begin'
                                                                : 'No tasks',
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
                                                            onMoveUp: index == 0
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
                                                                  if (widget.onRunSelected !=
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
                                                                    feedback: Material(
                                                                      elevation:
                                                                          8,
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
                                                                    childWhenDragging:
                                                                        Opacity(
                                                                          opacity:
                                                                              .4,
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
  });
  final TaskDto task;
  final String projectName;
  final VoidCallback onOpen;
  final ValueChanged<TaskColumn> onMove;
  final VoidCallback? onMoveUp;
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
              const SizedBox(height: 6),
              Wrap(
                spacing: 8,
                runSpacing: 4,
                children: [
                  Text(
                    task.priority,
                    style: Theme.of(context).textTheme.labelSmall,
                  ),
                  Text(
                    task.archived
                        ? 'Archived · ${task.condition}'
                        : task.condition,
                    style: Theme.of(context).textTheme.labelSmall,
                  ),
                  if (task.agentId != null)
                    const Icon(
                      Icons.smart_toy_outlined,
                      size: 16,
                      semanticLabel: 'Assigned agent',
                    ),
                ],
              ),
            ],
          ),
        ),
      ),
    ),
  );
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
  late Future<List<TaskAuditDto>> history;
  int? historyRevision;
  bool useAppServer = false;
  @override
  void initState() {
    super.initState();
    useAppServer =
        widget.profile()['transport'] == 'AppServer' ||
        (widget.controller.task(widget.taskId)?.skills.isNotEmpty ?? false);
    reload();
    widget.controller.addListener(onTaskChanged);
  }

  void onTaskChanged() {
    if (mounted &&
        widget.controller.task(widget.taskId)?.revision != historyRevision) {
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
    super.dispose();
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
    await widget.controller.mutate(task.projectId, {
      'Transition': {
        'task_id': task.id,
        'expected_revision': task.revision,
        'action': action,
      },
    });
    if (mounted) setState(reload);
  }

  Future<void> start(TaskDto task) async {
    final profile = {
      ...widget.profile(),
      'transport':
          (useAppServer ||
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
      'Start': {
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
      final agent = task.agentId == null ? null : c.agents[task.agentId];
      final running = task.running || agent?['can_stop'] == true;
      final disabled = c.busy || running;
      return AlertDialog(
        title: Text(task.title),
        content: SizedBox(
          width: 680,
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                SkillChips(
                  skills: task.skills,
                  onEdit: disabled ? null : () => selectSkills(task),
                ),
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

                Text(
                  '${task.column.label} · ${task.condition} · ${task.priority}',
                ),
                const SizedBox(height: 12),
                SelectableText(
                  task.description.isEmpty
                      ? 'No description'
                      : task.description,
                ),
                if (task.githubSource case final Map source) ...[
                  const SizedBox(height: 12),
                  Text(
                    '${source['repository']} #${source['number']} · ${source['state']} on GitHub',
                  ),
                  SelectableText('${source['url']}'),
                  Text('Last synced: ${source['last_synced_at']}'),
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
                const SizedBox(height: 16),
                Text(
                  'Acceptance criteria',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
                if (task.criteria.isEmpty) const Text('No criteria recorded.'),
                ...task.criteria.map(
                  (text) => Padding(
                    padding: const EdgeInsets.symmetric(vertical: 4),
                    child: Text('• $text'),
                  ),
                ),
                const SizedBox(height: 16),
                Text(
                  'Review summary',
                  style: Theme.of(context).textTheme.titleSmall,
                ),
                SelectableText(task.summary ?? 'Not submitted for review.'),
                if (task.reason != null)
                  Padding(
                    padding: const EdgeInsets.only(top: 8),
                    child: SelectableText(task.reason!),
                  ),
                const SizedBox(height: 12),
                AcceptanceReview(task: task, controller: c, running: running),
                if (widget.projects.any(
                  (p) => p.id == task.projectId && p.remote,
                ))
                  const Padding(
                    padding: EdgeInsets.only(top: 8),
                    child: Text(
                      'SSH execution uses interactive approvals. Project-root confinement is not currently enforced; approved commands run with the SSH account’s access.',
                    ),
                  ),
                if (task.agentId != null)
                  TextButton.icon(
                    onPressed: () {
                      Navigator.pop(context);
                      widget.onOpenAgent(task.agentId!);
                    },
                    icon: const Icon(Icons.chat_bubble_outline),
                    label: Text(
                      agent?['user_title'] as String? ??
                          agent?['codex_title'] as String? ??
                          'Open linked agent chat',
                    ),
                  ),
                if (!running &&
                    !task.archived &&
                    task.column != TaskColumn.done) ...[
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
                if (c.error != null) TaskErrorBanner(controller: c),
                const SizedBox(height: 12),
                ExpansionTile(
                  title: const Text('Transition history'),
                  children: [
                    FutureBuilder<List<TaskAuditDto>>(
                      future: history,
                      builder: (context, snapshot) {
                        if (snapshot.hasError) {
                          return Text('History unavailable: ${snapshot.error}');
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
                if (!disabled &&
                    !task.archived &&
                    task.column != TaskColumn.done &&
                    task.column != TaskColumn.inReview)
                  DropdownButton<String>(
                    hint: const Text('Link an existing idle agent'),
                    isExpanded: true,
                    items: [
                      for (final a in c.agents.values.where(
                        (a) =>
                            a['project_id'] == task.projectId &&
                            a['task_id'] == null &&
                            a['can_stop'] != true &&
                            !const {
                              'Starting',
                              'Working',
                              'Stopping',
                              'AwaitingApproval',
                            }.contains(a['state']),
                      ))
                        DropdownMenuItem(
                          value: a['id'] as String,
                          child: Text(
                            a['user_title'] as String? ??
                                a['codex_title'] as String? ??
                                a['id'] as String,
                          ),
                        ),
                    ],
                    onChanged: (id) async {
                      if (id == null) return;
                      await c.mutate(task.projectId, {
                        'Link': {
                          'task_id': task.id,
                          'expected_revision': task.revision,
                          'agent_id': id,
                        },
                      });
                      if (mounted) setState(reload);
                    },
                  ),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Close'),
          ),
          if (!task.archived &&
              task.column != TaskColumn.done &&
              task.column != TaskColumn.inReview)
            TextButton(
              onPressed: disabled
                  ? null
                  : () async {
                      await showTaskEditor(
                        context,
                        c,
                        widget.projects,
                        task: task,
                      );
                      if (mounted) setState(reload);
                    },
              child: const Text('Edit'),
            ),
          if (!task.archived && task.column != TaskColumn.done)
            TextButton(
              onPressed: disabled
                  ? null
                  : () => transition(task, {
                      'Cancel': {'reason': 'Cancelled by user'},
                    }),
              child: const Text('Cancel / Archive'),
            ),
          if (task.revision == 1 && task.agentId == null)
            TextButton(
              onPressed: disabled
                  ? null
                  : () async {
                      final result = await c.mutate(task.projectId, {
                        'Delete': {
                          'task_id': task.id,
                          'expected_revision': task.revision,
                        },
                      });
                      if (result != null && context.mounted) {
                        Navigator.pop(context);
                      }
                    },
              child: const Text('Delete Draft'),
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
              onPressed: disabled ? null : () => start(task),
              child: const Text('Start Task Agent'),
            ),
          if (!task.archived && task.column == TaskColumn.inProgress)
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
      );
    },
  );
}
