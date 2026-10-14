import 'package:flutter/material.dart';
import 'application/skills_controller.dart';
import 'data/task_models.dart';

Future<bool> confirmSkillAction(
  BuildContext context,
  String title,
  String details,
) async =>
    await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(title),
        scrollable: true,
        content: SizedBox(width: 560, child: SelectableText(details)),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Confirm'),
          ),
        ],
      ),
    ) ??
    false;

String skillDetails(Map entry) =>
    '${entry['description'] ?? ''}\n\nSource: ${entry['source'] ?? 'Unknown'}\nPath: ${entry['path']}\nRevision: ${entry['revision'] ?? 'Unversioned'}\nSHA-256: ${entry['content_hash']}\nLicense/provenance: ${entry['license'] ?? 'Unknown — not verified'}\n${entry['has_scripts'] == true ? 'Contains scripts or executable files. They are not run during installation.' : 'No scripts detected.'}\n${entry['validation_error'] ?? ''}\n${(entry['missing_dependencies'] as List? ?? []).join('\n')}';

class SkillChips extends StatelessWidget {
  const SkillChips({super.key, required this.skills, this.onEdit});
  final List<Map<String, dynamic>> skills;
  final VoidCallback? onEdit;
  @override
  Widget build(BuildContext context) => Wrap(
    spacing: 6,
    runSpacing: 4,
    children: [
      for (final skill in skills)
        Tooltip(
          message: 'Pinned SHA-256: ${skill['content_hash']}\n${skill['path']}',
          child: Chip(label: Text(skill['name'] as String)),
        ),
      if (onEdit != null)
        TextButton.icon(
          onPressed: onEdit,
          icon: const Icon(Icons.extension_outlined, size: 16),
          label: Text(skills.isEmpty ? 'Select skills…' : 'Edit skills…'),
        ),
    ],
  );
}

Future<List<Map<String, dynamic>>?> showSkillPicker(
  BuildContext context,
  SkillsController controller,
  String project,
  List<Map<String, dynamic>> selected,
) => showDialog<List<Map<String, dynamic>>>(
  context: context,
  builder: (_) => _SkillPicker(
    controller: controller,
    project: project,
    selected: selected,
  ),
);

class _SkillPicker extends StatefulWidget {
  const _SkillPicker({
    required this.controller,
    required this.project,
    required this.selected,
  });
  final SkillsController controller;
  final String project;
  final List<Map<String, dynamic>> selected;
  @override
  State<_SkillPicker> createState() => _SkillPickerState();
}

class _SkillPickerState extends State<_SkillPicker> {
  late final selected = widget.selected
      .map((s) => Map<String, dynamic>.from(s))
      .toList();
  late Future<List<Map<String, dynamic>>> entries = widget.controller.list(
    widget.project,
  );
  String query = '';
  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Select skills'),
    content: SizedBox(
      width: 620,
      height: 440,
      child: Column(
        children: [
          const Text(
            'Select a small set; three is a useful default. Skills require App Server and apply to a fresh thread. The order below sets precedence.',
          ),
          TextField(
            decoration: const InputDecoration(labelText: 'Search skills'),
            onChanged: (s) => setState(() => query = s.toLowerCase()),
          ),
          Expanded(
            child: FutureBuilder<List<Map<String, dynamic>>>(
              future: entries,
              builder: (context, snapshot) {
                if (!snapshot.hasData) {
                  return const Center(child: CircularProgressIndicator());
                }
                final values = snapshot.data!
                    .where(
                      (s) => '${s['name']} ${s['description']}'
                          .toLowerCase()
                          .contains(query),
                    )
                    .toList();
                return ListView(
                  children: [
                    if (widget.controller.error != null)
                      Text(widget.controller.error!),
                    if (values.isEmpty)
                      const Text(
                        'No recognized skills. Use Skills → Available to install one, or refresh after checking the selected Codex installation.',
                      ),
                    for (final entry in values)
                      CheckboxListTile(
                        value: selected.any(
                          (s) => s['identity'] == entry['identity'],
                        ),
                        title: Text(entry['name'] as String),
                        subtitle: Text(
                          '${selected.any((s) => s['identity'] == entry['identity'] && s['content_hash'] != entry['content_hash']) ? 'Changed since selection: deselect and reselect to accept this revision.\n' : ''}${entry['scope']} · ${entry['path']}\n${entry['description']}\n${entry['validation_error'] ?? (SkillsController.usable(entry) ? '' : 'Disabled, unrecognized, or missing dependencies')}',
                        ),
                        onChanged: !SkillsController.usable(entry)
                            ? null
                            : (checked) => setState(() {
                                selected.removeWhere(
                                  (s) => s['identity'] == entry['identity'],
                                );
                                if (checked == true) {
                                  selected.add(SkillsController.binding(entry));
                                }
                              }),
                      ),
                  ],
                );
              },
            ),
          ),
          SizedBox(
            height: 90,
            child: ListView(
              children: [
                for (var i = 0; i < selected.length; i++)
                  ListTile(
                    dense: true,
                    title: Text('${i + 1}. ${selected[i]['name']}'),
                    trailing: Wrap(
                      children: [
                        IconButton(
                          tooltip: 'Move skill earlier',
                          onPressed: i == 0
                              ? null
                              : () => setState(() {
                                  final item = selected.removeAt(i);
                                  selected.insert(i - 1, item);
                                }),
                          icon: const Icon(Icons.arrow_upward, size: 16),
                        ),
                        IconButton(
                          tooltip: 'Remove selected skill',
                          onPressed: () => setState(() => selected.removeAt(i)),
                          icon: const Icon(Icons.close, size: 16),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
          ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () =>
            setState(() => entries = widget.controller.list(widget.project)),
        child: const Text('Refresh'),
      ),
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () async {
          if (selected.length > 3 &&
              !await confirmSkillAction(
                context,
                'Attach more than three skills?',
                '${selected.length} skills may add conflicting instructions and consume more context. Keep this explicit selection?',
              )) {
            return;
          }
          if (context.mounted) Navigator.pop(context, selected);
        },
        child: const Text('Use selected skills'),
      ),
    ],
  );
}

class SkillsDirectory extends StatefulWidget {
  const SkillsDirectory({
    super.key,
    required this.controller,
    required this.projects,
    required this.connected,
  });
  final SkillsController controller;
  final List<TaskProjectOption> projects;
  final bool connected;
  @override
  State<SkillsDirectory> createState() => _SkillsDirectoryState();
}

class _SkillsDirectoryState extends State<SkillsDirectory> {
  String? project, source;
  String query = '', scope = 'all';
  bool available = false, loading = false;
  List<Map<String, dynamic>> entries = [], sources = [];
  int? nextOffset;
  int invalidation = 0;
  void onInvalidation() {
    if (widget.controller.invalidation != invalidation &&
        !widget.controller.busy &&
        !loading) {
      invalidation = widget.controller.invalidation;
      load();
    }
  }

  @override
  void dispose() {
    widget.controller.removeListener(onInvalidation);
    super.dispose();
  }

  @override
  void initState() {
    super.initState();
    widget.controller.addListener(onInvalidation);
    project = widget.projects.firstOrNull?.id;
    load();
  }

  Future<void> load() async {
    if (loading) return;
    setState(() => loading = true);
    final result = await widget.controller.call(project, {
      'Sources': {'offset': 0, 'limit': 100},
    });
    if (!mounted) return;
    sources = ((result?['Sources'] as Map?)?['sources'] as List? ?? [])
        .map((e) => Map<String, dynamic>.from(e as Map))
        .toList();
    if (!sources.any((entry) => entry['id'] == source)) {
      source = sources.firstOrNull?['id'] as String?;
    }
    if (!available && project != null) {
      entries = await widget.controller.list(project!);
    }
    if (mounted) setState(() => loading = false);
  }

  Future<void> browse({bool more = false}) async {
    if (source == null) return;
    final chosen = sources.firstWhere((s) => s['id'] == source);
    final location = chosen['location'] as String;
    final network = !location.startsWith('/');
    if (network &&
        !await confirmSkillAction(
          context,
          'Fetch catalog source?',
          'Source: $location\nRef: ${chosen['reference']}\nNetwork access is required to enumerate this collection. This does not install skills or run repository scripts.',
        )) {
      return;
    }
    final result = await widget.controller.call(project, {
      'BrowseSource': {
        'source_id': source,
        'allow_network': network,
        'offset': more ? nextOffset : 0,
        'limit': 30,
      },
    });
    if (!mounted) return;
    if (result != null) {
      final page = result['Entries'] as Map;
      setState(() {
        final values = (page['entries'] as List).map(
          (e) => Map<String, dynamic>.from(e as Map),
        );
        entries = more ? [...entries, ...values] : values.toList();
        nextOffset = page['next_offset'] as int?;
      });
    }
    setState(() {});
  }

  Future<void> install(Map entry) async {
    if (project == null) return;
    final network = !(entry['source'] as String).startsWith('/');
    if (network &&
        !await confirmSkillAction(
          context,
          'Prepare selected skill?',
          'Fetch ${entry['source']} to prepare only ${entry['relative_path']}. You will review the resolved revision, license, files and destination before installing.',
        )) {
      return;
    }
    final result = await widget.controller.call(project, {
      'PrepareInstall': {
        'source_id': entry['source_id'],
        'relative_path': entry['relative_path'],
        'allow_network': network,
      },
    });
    if (!mounted || result == null) {
      if (mounted) setState(() {});
      return;
    }
    final plan = result['Plan'] as Map;
    final selected = plan['entry'] as Map;
    if (!await confirmSkillAction(
      context,
      'Install this revision?',
      '${skillDetails(selected)}\n\nRequested ref: ${(plan['source'] as Map)['reference']}\nResolved commit: ${plan['resolved_revision'] ?? 'Local content hash'}\nDestination: ${plan['destination']}\nIncluded paths:\n${(plan['included_paths'] as List).join('\n')}',
    )) {
      await widget.controller.call(project, {
        'DiscardPlan': {'plan_id': plan['id']},
      });
      return;
    }
    await widget.controller.call(project, {
      'ConfirmInstall': {'plan_id': plan['id']},
    });
    if (mounted) {
      setState(() => available = false);
      await load();
    }
  }

  Future<void> action(Map entry, String action) async {
    final id = entry['identity'];
    if (action == 'preview') {
      final result = await widget.controller.call(project, {
        'Preview': {'identity': id},
      });
      if (!mounted || result == null) return;
      final preview = result['Preview'] as Map;
      await showDialog<void>(
        context: context,
        builder: (context) => AlertDialog(
          title: Text(entry['name'] as String),
          scrollable: true,
          content: SizedBox(
            width: 650,
            child: SelectableText(
              '${skillDetails(preview['entry'] as Map)}\n\n${preview['instructions']}',
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
      return;
    }
    if (action == 'update') {
      await install(entry);
      return;
    }
    if (action == 'rollback') {
      final result = await widget.controller.call(project, {
        'Versions': {'identity': id},
      });
      if (!mounted || result == null) return;
      final versions = result['Versions'] as List;
      final hash = await showDialog<String>(
        context: context,
        builder: (context) => SimpleDialog(
          title: const Text('Choose a retained revision'),
          children: [
            for (final value in versions)
              SimpleDialogOption(
                onPressed: () => Navigator.pop(
                  context,
                  (value['entry'] as Map)['content_hash'] as String,
                ),
                child: Text(
                  '${value['created_at']}\n${(value['entry'] as Map)['content_hash']}',
                ),
              ),
          ],
        ),
      );
      if (hash != null &&
          mounted &&
          await confirmSkillAction(
            context,
            'Roll back selected skill?',
            '${entry['name']}\nRetained hash: $hash\nExisting task pins remain unchanged.',
          )) {
        await widget.controller.call(project, {
          'Rollback': {'identity': id, 'content_hash': hash},
        });
      }
    } else if (action == 'sync') {
      final targets = widget.projects.where((p) => p.remote).toList();
      final target = await showDialog<String>(
        context: context,
        builder: (context) => SimpleDialog(
          title: const Text('Sync to SSH project'),
          children: [
            if (targets.isEmpty)
              const Padding(
                padding: EdgeInsets.all(16),
                child: Text('Register an SSH project first.'),
              ),
            for (final target in targets)
              SimpleDialogOption(
                onPressed: () => Navigator.pop(context, target.id),
                child: Text(target.name),
              ),
          ],
        ),
      );
      if (target != null &&
          mounted &&
          await confirmSkillAction(
            context,
            'Sync this exact revision?',
            '${skillDetails(entry)}\n\nTransfer only this selected skill to the SSH account’s Ditch skills directory. Network access is required. Scripts are not executed.',
          )) {
        await widget.controller.call(null, {
          'Sync': {
            'identity': id,
            'content_hash': entry['content_hash'],
            'target_project_id': target,
          },
        });
      }
    } else if (action == 'remove') {
      if (await confirmSkillAction(
        context,
        'Remove managed skill?',
        '${entry['name']}\nTask and session pins block removal. Unpinned installed revisions will be removed.',
      )) {
        await widget.controller.call(project, {
          'Remove': {'identity': id},
        });
      }
    } else if (action == 'toggle') {
      if (await confirmSkillAction(
        context,
        entry['enabled'] == true ? 'Disable skill?' : 'Enable skill?',
        skillDetails(entry),
      )) {
        await widget.controller.call(project, {
          'SetEnabled': {'identity': id, 'enabled': entry['enabled'] != true},
        });
      }
    }
    if (mounted) await load();
  }

  Future<void> addSource() async {
    final name = TextEditingController(),
        location = TextEditingController(),
        reference = TextEditingController(text: 'HEAD');
    final confirm = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Add skill source'),
        scrollable: true,
        content: SizedBox(
          width: 500,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              TextField(
                controller: name,
                decoration: const InputDecoration(labelText: 'Name'),
              ),
              TextField(
                controller: location,
                decoration: const InputDecoration(
                  labelText: 'Git URL or absolute directory',
                ),
              ),
              TextField(
                controller: reference,
                decoration: const InputDecoration(labelText: 'Ref / version'),
              ),
              const Text(
                'Adding a source records metadata. Fetching and installing require separate actions. Local roots become available to Codex discovery.',
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
            child: const Text('Add source'),
          ),
        ],
      ),
    );
    if (confirm == true) {
      await widget.controller.call(project, {
        'AddSource': {
          'name': name.text.trim(),
          'location': location.text.trim(),
          'reference': reference.text.trim(),
        },
      });
    }
    name.dispose();
    location.dispose();
    reference.dispose();
    if (mounted) await load();
  }

  @override
  Widget build(BuildContext context) => AnimatedBuilder(
    animation: widget.controller,
    builder: (context, _) => Padding(
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 12,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text('Skills', style: Theme.of(context).textTheme.headlineSmall),
              TextButton(
                onPressed: project == null || widget.controller.busy
                    ? null
                    : () async {
                        final result = await widget.controller.call(
                          project,
                          'Capabilities',
                        );
                        if (!context.mounted || result == null) return;
                        final report = result['Capabilities'] as Map;
                        await showDialog<void>(
                          context: context,
                          builder: (context) => AlertDialog(
                            title: const Text('Execution support'),
                            scrollable: true,
                            content: SelectableText(
                              'App Server: ${report['app_server']}\nExtra skill roots: ${report['extra_roots']}\nSandbox probe: ${report['sandbox_supported']}\nActive boundary: ${report['selected_boundary']}\n${(report['details'] as List).join('\n')}',
                            ),
                            actions: [
                              TextButton(
                                onPressed: () => Navigator.pop(context),
                                child: const Text('Close'),
                              ),
                            ],
                          ),
                        );
                      },
                child: const Text('Check execution support'),
              ),

              DropdownButton<String>(
                value: project,
                hint: const Text('Select project'),
                items: widget.projects
                    .map(
                      (p) => DropdownMenuItem(
                        value: p.id,
                        child: Text('${p.name}${p.remote ? ' (SSH)' : ''}'),
                      ),
                    )
                    .toList(),
                onChanged: (id) {
                  setState(() {
                    project = id;
                    entries = [];
                  });
                  load();
                },
              ),
              SegmentedButton<bool>(
                segments: const [
                  ButtonSegment(value: false, label: Text('Installed')),
                  ButtonSegment(value: true, label: Text('Available')),
                ],
                selected: {available},
                onSelectionChanged: (s) {
                  setState(() {
                    available = s.first;
                    entries = [];
                  });
                  load();
                },
              ),
              IconButton(
                tooltip: 'Refresh skills',
                onPressed: widget.controller.busy ? null : load,
                icon: const Icon(Icons.refresh),
              ),
            ],
          ),
          if (!widget.connected) const Text('Reconnecting to runtime…'),
          const Text(
            'Only explicitly selected skill paths are attached. Local installations are not automatically available on SSH targets.',
          ),
          if (available)
            Wrap(
              spacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                DropdownButton<String>(
                  value: source,
                  hint: const Text('Source'),
                  items: sources
                      .map(
                        (s) => DropdownMenuItem(
                          value: s['id'] as String,
                          child: Text(s['name'] as String),
                        ),
                      )
                      .toList(),
                  onChanged: (id) => setState(() {
                    source = id;
                    entries = [];
                  }),
                ),
                TextButton(
                  onPressed: widget.controller.busy ? null : () => browse(),
                  child: const Text('Browse source'),
                ),
                TextButton(
                  onPressed: addSource,
                  child: const Text('Add source…'),
                ),
                TextButton(
                  onPressed: source == null
                      ? null
                      : () async {
                          if (await confirmSkillAction(
                            context,
                            'Remove source?',
                            'Remove source metadata; installed revisions remain available.',
                          )) {
                            await widget.controller.call(project, {
                              'RemoveSource': {'source_id': source},
                            });
                            if (mounted) {
                              setState(() => source = null);
                              load();
                            }
                          }
                        },
                  child: const Text('Remove source'),
                ),
              ],
            ),
          Row(
            children: [
              Expanded(
                child: TextField(
                  decoration: const InputDecoration(labelText: 'Search skills'),
                  onChanged: (v) => setState(() => query = v.toLowerCase()),
                ),
              ),
              const SizedBox(width: 12),
              DropdownButton<String>(
                value: scope,
                items:
                    [
                          'all',
                          'user',
                          'repo',
                          'system',
                          'admin',
                          'managed',
                          'available',
                        ]
                        .map((s) => DropdownMenuItem(value: s, child: Text(s)))
                        .toList(),
                onChanged: (s) => setState(() => scope = s!),
              ),
            ],
          ),
          if (widget.controller.busy || loading)
            const LinearProgressIndicator(),
          if (widget.controller.error != null)
            SelectableText(
              widget.controller.error!,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          Expanded(
            child: ListView(
              children: [
                if (entries.isEmpty && !loading)
                  Padding(
                    padding: const EdgeInsets.all(24),
                    child: Text(
                      available
                          ? 'Choose a source and browse its collection. Nothing is installed automatically.'
                          : 'No skills to display. Select a project and refresh, or open Available.',
                    ),
                  ),
                for (final entry in entries.where(
                  (e) =>
                      (scope == 'all' || e['scope'] == scope) &&
                      '${e['name']} ${e['description']}'.toLowerCase().contains(
                        query,
                      ),
                ))
                  Card(
                    child: ListTile(
                      title: Text(entry['name'] as String),
                      subtitle: Text(
                        '${entry['description']}\n${entry['scope']} · ${entry['source']}\n${entry['has_scripts'] == true ? 'Scripts / executables · ' : ''}${entry['license'] == null ? 'Unknown license · ' : ''}${entry['enabled'] == false ? 'Disabled · ' : ''}${entry['recognized'] == false ? 'Not loaded · ' : ''}${entry['validation_error'] ?? ''}',
                      ),
                      onTap: available ? null : () => action(entry, 'preview'),
                      trailing: available
                          ? TextButton(
                              onPressed:
                                  widget.controller.busy || project == null
                                  ? null
                                  : () => install(entry),
                              child: const Text('Install…'),
                            )
                          : PopupMenuButton<String>(
                              tooltip: 'Skill actions',
                              onSelected: (v) => action(entry, v),
                              itemBuilder: (_) => [
                                const PopupMenuItem(
                                  value: 'preview',
                                  child: Text('Preview instructions'),
                                ),
                                PopupMenuItem(
                                  value: 'toggle',
                                  child: Text(
                                    entry['enabled'] == true
                                        ? 'Disable'
                                        : 'Enable',
                                  ),
                                ),
                                if (entry['managed'] == true) ...[
                                  const PopupMenuItem(
                                    value: 'update',
                                    child: Text('Update…'),
                                  ),
                                  const PopupMenuItem(
                                    value: 'rollback',
                                    child: Text('Rollback…'),
                                  ),
                                  const PopupMenuItem(
                                    value: 'sync',
                                    child: Text('Sync to SSH…'),
                                  ),
                                  const PopupMenuItem(
                                    value: 'remove',
                                    child: Text('Remove…'),
                                  ),
                                ],
                              ],
                            ),
                    ),
                  ),
                if (available && nextOffset != null)
                  TextButton(
                    onPressed: () => browse(more: true),
                    child: const Text('Load more'),
                  ),
              ],
            ),
          ),
        ],
      ),
    ),
  );
}
