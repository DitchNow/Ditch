import 'dart:convert';
import 'package:flutter/material.dart';
import '../application/task_board_controller.dart';
import '../data/task_models.dart';

Future<void> showTaskReleaseDialog(
  BuildContext context, {
  required TaskBoardController controller,
  required List<TaskDto> tasks,
  Map<String, String> projectNames = const {},
  String? preselectedRelease,
}) async {
  if (tasks.isEmpty || controller.busy || controller.hasPendingRelease) return;
  await showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (_) => _TaskReleaseDialog(
      controller: controller,
      tasks: tasks,
      projectNames: projectNames,
      preselectedRelease: preselectedRelease,
    ),
  );
}

class _TaskReleaseDialog extends StatefulWidget {
  const _TaskReleaseDialog({
    required this.controller,
    required this.tasks,
    required this.projectNames,
    this.preselectedRelease,
  });
  final TaskBoardController controller;
  final List<TaskDto> tasks;
  final Map<String, String> projectNames;
  final String? preselectedRelease;
  @override
  State<_TaskReleaseDialog> createState() => _TaskReleaseDialogState();
}

class _TaskReleaseDialogState extends State<_TaskReleaseDialog> {
  final choices = <String, String>{};
  final names = <String, TextEditingController>{};
  final succeeded = <String>{};
  bool loading = true, submitting = false, loaded = false;
  String? error;
  String? unresolvedProject;
  late final projects = widget.tasks.map((t) => t.projectId).toSet().toList();
  bool get accepting => widget.tasks.first.column == TaskColumn.inReview;

  @override
  void initState() {
    super.initState();
    for (final project in projects) {
      choices[project] = widget.preselectedRelease ?? 'other';
      names[project] = TextEditingController();
    }
    load();
  }

  Future<void> load() async {
    setState(() {
      loading = true;
      loaded = false;
      error = null;
    });
    try {
      for (final project in projects) {
        await widget.controller.loadReleases(project);
        if (choices[project] != 'other' &&
            choices[project] != 'new' &&
            !widget.controller
                .projectReleases(project)
                .any((r) => r.id == choices[project])) {
          choices[project] = 'other';
        }
      }
      loaded = true;
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => loading = false);
    }
  }

  String? validation(String project) {
    if (choices[project] != 'new') return null;
    final name = names[project]!.text.trim();
    if (name.isEmpty) return 'Enter a release name';
    if (utf8.encode(name).length > 128) return 'Use at most 128 UTF-8 bytes';
    if (widget.controller.projectReleases(project).any((r) => r.name == name)) {
      return 'This release exists. Select it above.';
    }
    return null;
  }

  Future<void> submit() async {
    if (!loaded ||
        loading ||
        submitting ||
        projects.any((p) => !succeeded.contains(p) && validation(p) != null)) {
      return;
    }
    setState(() {
      submitting = true;
      error = null;
    });
    for (final project in projects.where((p) => !succeeded.contains(p))) {
      final selected = widget.tasks
          .where((t) => t.projectId == project)
          .map((t) => widget.controller.task(t.id))
          .whereType<TaskDto>()
          .toList();
      if (selected.length !=
          widget.tasks.where((t) => t.projectId == project).length) {
        error =
            'A selected task disappeared. Close this dialog and select tasks again.';
        break;
      }
      final choice = choices[project]!;
      final Object destination = choice == 'other'
          ? 'Other'
          : choice == 'new'
          ? {'New': names[project]!.text.trim()}
          : {'Existing': choice};
      final result = unresolvedProject == project
          ? await widget.controller.retry()
          : await widget.controller.releaseBatch(
              selected,
              destination,
              accept: accepting,
            );
      if (!mounted) return;
      if (result == null) {
        unresolvedProject = widget.controller.canRetry ? project : null;
        error =
            widget.controller.error ??
            'Release update could not be confirmed. Retry.';
        break;
      }
      unresolvedProject = null;
      succeeded.add(project);
    }
    if (!mounted) return;
    if (succeeded.length == projects.length) {
      Navigator.pop(context);
    } else {
      setState(() => submitting = false);
    }
  }

  @override
  void dispose() {
    for (final name in names.values) {
      name.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !submitting,
    child: AlertDialog(
      title: Text(accepting ? 'Accept and release work' : 'Move to release'),
      content: SizedBox(
        width: 440,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                accepting
                    ? 'Confirm you reviewed the summary and acceptance criteria for ${widget.tasks.length} task(s).'
                    : 'Choose a release for ${widget.tasks.length} completed task(s).',
              ),
              if (loading)
                const Padding(
                  padding: EdgeInsets.all(16),
                  child: LinearProgressIndicator(),
                ),
              if (!loading && loaded)
                for (final project in projects)
                  Padding(
                    padding: const EdgeInsets.only(top: 16),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          widget.projectNames[project] ?? project,
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                        if (succeeded.contains(project))
                          const Text('Updated successfully')
                        else ...[
                          DropdownButtonFormField<String>(
                            key: ValueKey('release-choice-$project'),
                            initialValue: choices[project],
                            decoration: const InputDecoration(
                              labelText: 'Release',
                            ),
                            isExpanded: true,
                            items: [
                              const DropdownMenuItem(
                                value: 'other',
                                child: Text('Other Tasks'),
                              ),
                              for (final release
                                  in widget.controller.projectReleases(project))
                                DropdownMenuItem(
                                  value: release.id,
                                  child: Text(
                                    release.name,
                                    overflow: TextOverflow.ellipsis,
                                  ),
                                ),
                              const DropdownMenuItem(
                                value: 'new',
                                child: Text('Create new release…'),
                              ),
                            ],
                            onChanged: submitting || unresolvedProject != null
                                ? null
                                : (value) =>
                                      setState(() => choices[project] = value!),
                          ),
                          if (choices[project] == 'new')
                            TextField(
                              key: ValueKey('release-name-$project'),
                              controller: names[project],
                              enabled: !submitting && unresolvedProject == null,
                              autofocus: true,
                              decoration: InputDecoration(
                                labelText: 'Version or release name',
                                errorText: validation(project),
                              ),
                              onChanged: (_) => setState(() {}),
                              onSubmitted: (_) => submit(),
                            ),
                        ],
                      ],
                    ),
                  ),
              if (error != null)
                Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Text(
                    error!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                ),
              if (succeeded.isNotEmpty)
                const Text(
                  'Successful projects are saved. Only remaining projects will be retried.',
                ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: submitting ? null : () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        if (error != null && choices.isNotEmpty && !submitting)
          TextButton(onPressed: load, child: const Text('Refresh releases')),
        FilledButton(
          autofocus: true,
          onPressed:
              !loaded ||
                  loading ||
                  submitting ||
                  projects.any(
                    (p) => !succeeded.contains(p) && validation(p) != null,
                  )
              ? null
              : submit,
          child: Text(
            submitting
                ? 'Saving…'
                : unresolvedProject != null
                ? 'Retry pending update'
                : accepting
                ? 'Accept and move'
                : 'Move',
          ),
        ),
      ],
    ),
  );
}
