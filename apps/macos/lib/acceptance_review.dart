import 'dart:convert';
import 'package:flutter/material.dart';
import 'application/task_board_controller.dart';
import 'data/task_models.dart';

class AcceptanceReview extends StatefulWidget {
  const AcceptanceReview({
    super.key,
    required this.task,
    required this.controller,
    required this.running,
  });
  final TaskDto task;
  final TaskBoardController controller;
  final bool running;
  @override
  State<AcceptanceReview> createState() => _AcceptanceReviewState();
}

class _AcceptanceReviewState extends State<AcceptanceReview> {
  final Set<String> checked = {};
  Future<void> operation(
    String name, [
    Map<String, dynamic> extra = const {},
  ]) async {
    await widget.controller.mutate(widget.task.projectId, {
      name: {
        'task_id': widget.task.id,
        'expected_revision': widget.task.revision,
        ...extra,
      },
    });
  }

  Future<void> diff(Map submission) async {
    final result = await widget.controller.mutate(widget.task.projectId, {
      'ReviewDiff': {
        'task_id': widget.task.id,
        'submission_id': submission['id'],
      },
    });
    if (!mounted || result == null) return;
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Workspace diff'),
        content: SizedBox(
          width: 760,
          height: 480,
          child: SingleChildScrollView(
            child: SelectableText((result['Diff'] as Map)['text'] as String),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Close'),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final data = widget.task.acceptance;
    final config = data['config'] as Map? ?? {};
    final policy = config['policy'] as Map? ?? {};
    final attempts = data['attempts'] as List? ?? [];
    final submissions = data['submissions'] as List? ?? [];
    final current = submissions
        .cast<Map>()
        .where((s) => s['id'] == data['current_submission'])
        .firstOrNull;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Divider(),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            TextButton.icon(
              onPressed: widget.running || widget.controller.busy
                  ? null
                  : () => showDialog<void>(
                      context: context,
                      builder: (_) => AcceptanceEditor(
                        task: widget.task,
                        controller: widget.controller,
                      ),
                    ),
              icon: const Icon(Icons.rule),
              label: const Text('Acceptance checks'),
            ),
            if (widget.running)
              TextButton.icon(
                onPressed: widget.controller.busy
                    ? null
                    : () => operation('CancelLoop'),
                icon: const Icon(Icons.stop_circle_outlined),
                label: const Text('Cancel loop'),
              ),
            if (!widget.running &&
                (widget.task.column == TaskColumn.inProgress ||
                    widget.task.column == TaskColumn.inReview))
              TextButton(
                onPressed: widget.controller.busy
                    ? null
                    : () => operation('Revalidate'),
                child: const Text('Revalidate evidence'),
              ),
          ],
        ),
        Text(
          policy['enabled'] == true
              ? 'Bounded retries: ${policy['max_attempts']} attempts · ${policy['deadline_seconds']}s deadline · ${policy['retry_mode']} thread'
              : 'Automatic retries disabled',
        ),
        if ((config['criteria'] as List? ?? []).isNotEmpty ||
            policy['enabled'] == true)
          const Text(
            'Acceptance runs through App Server with the task’s existing permissions.',
          ),
        if (current == null)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 8),
            child: Text(
              'No immutable review submission yet. Human criteria and agent assertions cannot pass automatic acceptance checks.',
            ),
          ),
        if (current != null) ...[
          const SizedBox(height: 12),
          Text(
            'Review evidence',
            style: Theme.of(context).textTheme.titleSmall,
          ),
          for (final criterion in current['criteria'] as List? ?? [])
            if (criterion['status'] == 'HumanReview')
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                title: Text(criterion['criterion']['label'] as String),
                subtitle: const Text('Human assessment'),
                value: checked.contains(
                  '${current['id']}/${criterion['criterion']['id']}',
                ),
                onChanged: (v) => setState(() {
                  final key =
                      '${current['id']}/${criterion['criterion']['id']}';
                  if (v == true) {
                    checked.add(key);
                  } else {
                    checked.remove(key);
                  }
                }),
              )
            else
              ListTile(
                contentPadding: EdgeInsets.zero,
                dense: true,
                title: Text(
                  '${criterion['criterion']['label']} · ${criterion['status']}',
                ),
                subtitle: SelectableText(criterion['output'] as String? ?? ''),
              ),
          Text(
            'Workspace HEAD: ${current['workspace']['head'] ?? 'Not available'}',
          ),
          SelectableText(
            'Pre-existing dirty paths:\n${(current['preexisting_dirty_paths'] as List? ?? []).join('\n')}',
          ),
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: const Text('Changed files and diff'),
            children: [
              Align(
                alignment: Alignment.centerLeft,
                child: SelectableText(
                  (current['workspace']['dirty_paths'] as List? ?? []).join(
                    '\n',
                  ),
                ),
              ),
              Align(
                alignment: Alignment.centerLeft,
                child: SelectableText(
                  current['workspace']['diff_stat'] as String? ?? '',
                ),
              ),
              TextButton(
                onPressed: () => diff(current),
                child: const Text('View current matching diff'),
              ),
            ],
          ),
          for (final warning in current['workspace']['warnings'] as List? ?? [])
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Text(warning as String),
            ),
          if ((current['approvals'] as List? ?? []).isNotEmpty)
            Text('Approvals: ${(current['approvals'] as List).join(', ')}'),
        ],
        if (attempts.isNotEmpty)
          ExpansionTile(
            tilePadding: EdgeInsets.zero,
            title: Text('Attempt timeline (${attempts.length})'),
            children: [
              for (final attempt in attempts)
                ExpansionTile(
                  title: Text(
                    'Attempt ${attempt['ordinal']} · ${attempt['result']}',
                  ),
                  subtitle: Text('${attempt['started_at']}'),
                  children: [
                    Align(
                      alignment: Alignment.centerLeft,
                      child: SelectableText(
                        'Thread: ${attempt['thread_id'] ?? 'Unavailable'}\n${attempt['failure']?['message'] ?? ''}\n${attempt['summary'] ?? 'No final response recorded'}',
                      ),
                    ),
                    Text(
                      attempt['provider_usage'] == null
                          ? 'Token usage unavailable'
                          : 'Provider usage: ${jsonEncode(attempt['provider_usage'])}',
                    ),
                    for (final check in attempt['validators'] as List? ?? [])
                      ListTile(
                        dense: true,
                        title: Text(
                          '${check['criterion']['label']} · ${check['status']} · ${check['duration_ms']}ms',
                        ),
                        subtitle: SelectableText(
                          '${check['command'] == null ? '' : jsonEncode(check['command'])}\nExit: ${check['exit_code'] ?? 'Unavailable'}\n${check['output']}',
                        ),
                      ),
                  ],
                ),
            ],
          ),
      ],
    );
  }
}

class AcceptanceEditor extends StatefulWidget {
  const AcceptanceEditor({
    super.key,
    required this.task,
    required this.controller,
  });
  final TaskDto task;
  final TaskBoardController controller;
  @override
  State<AcceptanceEditor> createState() => _AcceptanceEditorState();
}

class _AcceptanceEditorState extends State<AcceptanceEditor> {
  late bool enabled, identical, requireChange;
  late String retryMode;
  late final TextEditingController attempts, deadline, timeout;
  late List<Map<String, dynamic>> criteria;
  final label = TextEditingController(),
      path = TextEditingController(),
      content = TextEditingController(),
      argv = TextEditingController(text: '["cargo", "test", "--offline"]'),
      cwd = TextEditingController(text: '.'),
      exit = TextEditingController(text: '0'),
      preset = TextEditingController();
  String kind = 'File exists';
  String? error;
  @override
  void initState() {
    super.initState();
    final config = widget.task.acceptance['config'] as Map? ?? {};
    final policy = config['policy'] as Map? ?? {};
    enabled = policy['enabled'] == true;
    identical = policy['stop_on_identical_failure'] != false;
    requireChange = policy['require_change'] == true;
    retryMode = policy['retry_mode'] as String? ?? 'Fresh';
    attempts = TextEditingController(text: '${policy['max_attempts'] ?? 3}');
    deadline = TextEditingController(
      text: '${policy['deadline_seconds'] ?? 1800}',
    );
    timeout = TextEditingController(
      text: '${policy['validator_timeout_seconds'] ?? 60}',
    );
    criteria = (config['criteria'] as List? ?? [])
        .map((v) => Map<String, dynamic>.from(v as Map))
        .toList();
  }

  @override
  void dispose() {
    for (final c in [
      attempts,
      deadline,
      timeout,
      label,
      path,
      content,
      argv,
      cwd,
      exit,
      preset,
    ]) {
      c.dispose();
    }
    super.dispose();
  }

  Future<bool> confirm(String title, String text) async =>
      await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(title),
          content: SingleChildScrollView(child: SelectableText(text)),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Approve exact command'),
            ),
          ],
        ),
      ) ==
      true;
  Future<void> add() async {
    try {
      if (label.text.trim().isEmpty) {
        throw const FormatException('Provide a criterion label.');
      }
      Object check;
      switch (kind) {
        case 'Human':
          check = 'Human';
        case 'Agent assertion':
          check = 'AgentAssertion';
        case 'Command':
          final values = (jsonDecode(argv.text) as List).cast<String>();
          final command = {
            'argv': values,
            'cwd': cwd.text,
            'timeout_seconds': int.parse(timeout.text),
            'expected_exit': int.parse(exit.text),
          };
          if (!await confirm(
            'Approve validator command?',
            '${jsonEncode(command)}\n\nThis fixed command runs in the project under the attempt’s sandbox and network policy. It may modify project files. No shell is added. Failed checks may rerun it within the bounded policy.',
          )) {
            return;
          }
          if (preset.text.trim().isNotEmpty) {
            final result = await widget.controller.mutate(
              widget.task.projectId,
              {
                'ApproveTestPreset': {
                  'name': preset.text.trim(),
                  'command': command,
                },
              },
            );
            if (result == null) {
              throw FormatException(
                widget.controller.error ?? 'Preset approval failed',
              );
            }
            check = {
              'TestPreset': {'name': preset.text.trim()},
            };
          } else {
            check = {'Command': command};
          }
        case 'Test preset':
          check = {
            'TestPreset': {'name': preset.text.trim()},
          };
        case 'Git clean':
          check = {'Git': 'Clean'};
        case 'Git dirty':
          check = {'Git': 'Dirty'};
        case 'Git changed path':
          check = {
            'Git': {'ChangedPath': path.text},
          };
        case 'Git forbidden path':
          check = {
            'Git': {'ForbiddenPath': path.text},
          };
        case 'Git diff size':
          check = {
            'Git': {'MaxDiffBytes': int.parse(content.text)},
          };
        default:
          final predicate = switch (kind) {
            'File absent' => 'Absent',
            'File contains' => {'Contains': content.text},
            'File excludes' => {'NotContains': content.text},
            _ => 'Exists',
          };
          check = {
            'File': {'path': path.text, 'predicate': predicate},
          };
      }
      if (!mounted) return;
      setState(() {
        criteria.add({
          'id': TaskBoardController.newRequestId(),
          'label': label.text.trim(),
          'required': true,
          'kind': check,
        });
        label.clear();
        error = null;
      });
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  Future<void> save() async {
    try {
      final config = {
        'policy': {
          'enabled': enabled,
          'max_attempts': int.parse(attempts.text),
          'deadline_seconds': int.parse(deadline.text),
          'validator_timeout_seconds': int.parse(timeout.text),
          'stop_on_denial': true,
          'stop_on_identical_failure': identical,
          'require_change': requireChange,
          'retry_mode': retryMode,
        },
        'criteria': criteria,
      };
      final result = await widget.controller.mutate(widget.task.projectId, {
        'ConfigureAcceptance': {
          'task_id': widget.task.id,
          'expected_revision': widget.task.revision,
          'config': config,
          'confirmed_commands': [
            for (final c in criteria)
              if (c['kind'] is Map && (c['kind'] as Map).containsKey('Command'))
                c['kind']['Command'],
          ],
        },
      });
      if (!mounted) return;
      if (result != null) {
        Navigator.pop(context);
      } else {
        setState(() => error = widget.controller.error);
      }
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  Widget field(TextEditingController controller, String text) => Padding(
    padding: const EdgeInsets.only(bottom: 10),
    child: TextField(
      controller: controller,
      decoration: InputDecoration(labelText: text),
    ),
  );
  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Acceptance checks and retry budget'),
    content: SizedBox(
      width: 620,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Enable bounded automatic retries'),
              value: enabled,
              onChanged: (v) => setState(() => enabled = v),
            ),
            field(attempts, 'Maximum attempts (1–10)'),
            field(deadline, 'Wall-clock deadline in seconds (1–86400)'),
            field(timeout, 'Validator timeout in seconds (1–300)'),
            DropdownButtonFormField<String>(
              initialValue: retryMode,
              decoration: const InputDecoration(labelText: 'Retry thread'),
              items: [
                for (final mode in ['Fresh', 'Resume'])
                  DropdownMenuItem(value: mode, child: Text(mode)),
              ],
              onChanged: (v) => setState(() => retryMode = v!),
            ),
            CheckboxListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Stop on repeated identical failure'),
              value: identical,
              onChanged: (v) => setState(() => identical = v!),
            ),
            CheckboxListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Require workspace changes'),
              value: requireChange,
              onChanged: (v) => setState(() => requireChange = v!),
            ),
            const Text(
              'Permission denial, cancellation, infrastructure failure, and the deadline always stop execution. Retries inherit the original permissions.',
            ),
            const Divider(),
            for (var i = 0; i < criteria.length; i++)
              ListTile(
                contentPadding: EdgeInsets.zero,
                title: Text(criteria[i]['label'] as String),
                subtitle: Text(jsonEncode(criteria[i]['kind'])),
                trailing: IconButton(
                  tooltip: 'Remove criterion',
                  icon: const Icon(Icons.close),
                  onPressed: () => setState(() => criteria.removeAt(i)),
                ),
              ),
            field(label, 'New criterion label'),
            DropdownButtonFormField<String>(
              initialValue: kind,
              decoration: const InputDecoration(labelText: 'Criterion type'),
              items: [
                for (final value in [
                  'Human',
                  'Agent assertion',
                  'File exists',
                  'File absent',
                  'File contains',
                  'File excludes',
                  'Command',
                  'Test preset',
                  'Git clean',
                  'Git dirty',
                  'Git changed path',
                  'Git forbidden path',
                  'Git diff size',
                ])
                  DropdownMenuItem(value: value, child: Text(value)),
              ],
              onChanged: (v) => setState(() => kind = v!),
            ),
            if (kind.startsWith('File') || kind.endsWith('path'))
              field(path, 'Project-relative path'),
            if ([
              'File contains',
              'File excludes',
              'Git diff size',
            ].contains(kind))
              field(
                content,
                kind == 'Git diff size'
                    ? 'Maximum diff bytes'
                    : 'Literal content',
              ),
            if (kind == 'Command') ...[
              field(argv, 'Fixed argv as a JSON array'),
              field(cwd, 'Project-relative working directory'),
              field(exit, 'Expected exit code'),
              field(preset, 'Save as project test preset (optional)'),
            ],
            if (kind == 'Test preset')
              field(preset, 'Previously approved project preset name'),
            TextButton.icon(
              onPressed: criteria.length >= 16 ? null : add,
              icon: const Icon(Icons.add),
              label: const Text('Add criterion'),
            ),
            if (error != null)
              Text(
                error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error),
              ),
          ],
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: save,
        child: const Text('Save acceptance policy'),
      ),
    ],
  );
}
