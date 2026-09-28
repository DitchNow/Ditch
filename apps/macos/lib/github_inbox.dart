import 'package:flutter/material.dart';
import 'application/task_board_controller.dart';
import 'data/task_models.dart';

Future<Map<String, dynamic>> githubRequest(
  TaskTransport request,
  Object operation,
) async {
  final result = await request({'GitHub': operation});
  if (result['Error'] case final Map error) {
    throw StateError('${error['message']}');
  }
  if (result['GitHub'] case final Map body) {
    return Map<String, dynamic>.from(body);
  }
  throw const FormatException(
    'GitHub integration requires the matching Ditch Runtime.',
  );
}

class GitHubSettingsDialog extends StatefulWidget {
  const GitHubSettingsDialog({super.key, required this.request});
  final TaskTransport request;
  @override
  State<GitHubSettingsDialog> createState() => _GitHubSettingsState();
}

class _GitHubSettingsState extends State<GitHubSettingsDialog> {
  Map<String, dynamic>? status;
  String? error;
  bool busy = false;
  @override
  void initState() {
    super.initState();
    load('Status');
  }

  Future<void> load(Object operation) async {
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final result = await githubRequest(widget.request, operation);
      if (mounted) setState(() => status = result);
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('GitHub integration'),
    content: SizedBox(
      width: 480,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (busy) const LinearProgressIndicator(),
          Text(
            status?['installed'] == true
                ? 'Managed GitHub CLI is installed.'
                : 'Install the GitHub CLI managed by Ditch. No terminal setup required.',
          ),
          const SizedBox(height: 12),
          if (status?['detail'] != null) Text('${status!['detail']}'),
          if (error != null)
            Text(
              error!,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          const SizedBox(height: 12),
          const Text(
            'This integration only reads GitHub. Importing an issue does not start an agent or change the issue.',
          ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Close'),
      ),
      TextButton(
        onPressed: busy ? null : () => load('Status'),
        child: const Text('Refresh'),
      ),
      FilledButton(
        onPressed: busy ? null : () => load('Install'),
        child: Text(
          status?['installed'] == true ? 'Repair CLI' : 'Install CLI',
        ),
      ),
      FilledButton(
        onPressed:
            busy ||
                status?['installed'] != true ||
                status?['connection_state'] == 'unsupported_coexistence'
            ? null
            : () => load('Connect'),
        child: const Text('Connect GitHub'),
      ),
    ],
  );
}

class GitHubInbox extends StatefulWidget {
  const GitHubInbox({
    super.key,
    required this.request,
    required this.projects,
    required this.onImported,
  });
  final TaskTransport request;
  final List<TaskProjectOption> projects;
  final Future<void> Function() onImported;
  @override
  State<GitHubInbox> createState() => _GitHubInboxState();
}

class _GitHubInboxState extends State<GitHubInbox> {
  List<Map<String, dynamic>> links = [], issues = [];
  Map<String, dynamic>? link;
  final selected = <int>{};
  String filter = 'Open';
  String? error, lastSync;
  bool busy = false, hasMore = false;
  int page = 0;
  @override
  void initState() {
    super.initState();
    loadLinks();
  }

  Future<void> loadLinks() async {
    try {
      final result = await githubRequest(widget.request, {
        'Links': {'project_id': null},
      });
      if (!mounted) return;
      final visible = widget.projects.map((p) => p.id).toSet();
      setState(() {
        links = (result['repositories'] as List)
            .map((v) => Map<String, dynamic>.from(v as Map))
            .where((v) => visible.contains(v['project_id']))
            .toList();
      });
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    }
  }

  Future<void> linkRepository() async {
    final input = TextEditingController();
    String? project = widget.projects.firstOrNull?.id;
    final confirm = await showDialog<bool>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, update) => AlertDialog(
          title: const Text('Link GitHub repository'),
          content: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              DropdownButton<String>(
                value: project,
                items: [
                  for (final p in widget.projects)
                    DropdownMenuItem(value: p.id, child: Text(p.name)),
                ],
                onChanged: (v) => update(() => project = v),
              ),
              TextField(
                controller: input,
                decoration: const InputDecoration(
                  labelText: 'OWNER/REPO or GitHub URL',
                ),
              ),
              const Text(
                'Choose the project where imported work should execute. Linking does not clone or modify the repository.',
              ),
            ],
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: project == null
                  ? null
                  : () => Navigator.pop(context, true),
              child: const Text('Verify and link'),
            ),
          ],
        ),
      ),
    );
    final repository = input.text;
    input.dispose();
    if (confirm != true || project == null) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await githubRequest(widget.request, {
        'Link': {'project_id': project, 'repository': repository},
      });
      await loadLinks();
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> fetch({bool more = false}) async {
    final current = link;
    if (current == null || busy) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final next = more ? page + 1 : 1;
      final result = await githubRequest(widget.request, {
        'Issues': {
          'project_id': current['project_id'],
          'repository_id': current['repository']['id'],
          'state': filter,
          'page': next,
        },
      });
      if (!mounted) return;
      setState(() {
        if (!more) {
          issues = [];
          selected.clear();
        }
        final rows = (result['issues'] as List).map(
          (v) => Map<String, dynamic>.from(v as Map),
        );
        final byId = {
          for (final i in [...issues, ...rows]) i['id']: i,
        };
        issues = byId.values.toList();
        page = next;
        hasMore = result['has_more'] == true;
        lastSync = result['last_synced_at']?.toString();
      });
    } on Object catch (e) {
      if (mounted) {
        setState(
          () => error = 'Could not refresh. Displayed issues may be stale. $e',
        );
      }
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> importSelected() async {
    if (link == null || selected.isEmpty || busy) return;
    final confirm = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Add ${selected.length} issues to Backlog?'),
        content: const Text(
          'This confirms the selected work for your backlog. Closed GitHub issues retain their Closed source badge; they are not marked accepted in Ditch. No agents will start.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Add to Backlog'),
          ),
        ],
      ),
    );
    if (confirm != true || !mounted) return;
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await githubRequest(widget.request, {
        'Import': {
          'project_id': link!['project_id'],
          'repository_id': link!['repository']['id'],
          'numbers': selected.toList(),
        },
      });
      await widget.onImported();
      if (mounted) setState(() => selected.clear());
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.all(12),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text('GitHub inbox', style: Theme.of(context).textTheme.titleLarge),
        Wrap(
          children: [
            TextButton(
              onPressed: busy ? null : linkRepository,
              child: const Text('Link repository'),
            ),
            TextButton(
              onPressed: () => showDialog<void>(
                context: context,
                builder: (_) => GitHubSettingsDialog(request: widget.request),
              ),
              child: const Text('Connection'),
            ),
          ],
        ),
        DropdownButton<String>(
          isExpanded: true,
          value: link == null
              ? null
              : '${link!['project_id']}:${link!['repository']['id']}',
          hint: const Text('Select a linked repository'),
          items: [
            for (final v in links)
              DropdownMenuItem(
                value: '${v['project_id']}:${v['repository']['id']}',
                child: Text(
                  '${v['repository']['full_name']}',
                  overflow: TextOverflow.ellipsis,
                ),
              ),
          ],
          onChanged: busy
              ? null
              : (id) {
                  setState(() {
                    link = links.firstWhere(
                      (v) =>
                          '${v['project_id']}:${v['repository']['id']}' == id,
                    );
                    issues = [];
                    page = 0;
                    selected.clear();
                  });
                  fetch();
                },
        ),
        Wrap(
          spacing: 8,
          children: [
            DropdownButton<String>(
              value: filter,
              items: [
                for (final v in ['Open', 'Closed', 'All'])
                  DropdownMenuItem(value: v, child: Text(v)),
              ],
              onChanged: busy
                  ? null
                  : (v) {
                      setState(() => filter = v!);
                      fetch();
                    },
            ),
            IconButton(
              tooltip: 'Refresh GitHub issues',
              onPressed: busy || link == null ? null : fetch,
              icon: const Icon(Icons.refresh),
            ),
          ],
        ),
        if (busy) const LinearProgressIndicator(),
        if (error != null)
          Text(
            error!,
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          ),
        if (lastSync != null) Text('Last refreshed: $lastSync'),
        if (links.isEmpty)
          const Text(
            'Connect GitHub and explicitly link repositories for this board.',
          ),
        Expanded(
          child: issues.isEmpty
              ? const Center(child: Text('No issues loaded'))
              : ListView(
                  children: [
                    for (final issue in issues)
                      CheckboxListTile(
                        value: selected.contains(issue['number']),
                        onChanged: busy
                            ? null
                            : (v) => setState(() {
                                if (v == true) {
                                  selected.add(issue['number'] as int);
                                } else {
                                  selected.remove(issue['number']);
                                }
                              }),
                        title: Text('#${issue['number']} ${issue['title']}'),
                        subtitle: TextButton(
                          onPressed: () => showDialog<void>(
                            context: context,
                            builder: (context) => AlertDialog(
                              title: Text('${issue['title']}'),
                              content: SizedBox(
                                width: 560,
                                child: SingleChildScrollView(
                                  child: SelectableText(
                                    '${issue['html_url']}\n${issue['state']} · ${issue['state_reason'] ?? ''}\nAuthor: ${issue['user']?['login'] ?? 'Unknown'}\nCreated: ${issue['created_at'] ?? ''} · Updated: ${issue['updated_at'] ?? ''}\nLabels: ${(issue['labels'] as List? ?? []).map((v) => v['name']).join(', ')}\nAssignees: ${(issue['assignees'] as List? ?? []).map((v) => v['login']).join(', ')}\nComments: ${issue['comments'] ?? 0}\n\n${issue['body'] ?? ''}',
                                  ),
                                ),
                              ),
                              actions: [
                                TextButton(
                                  onPressed: () => Navigator.pop(context),
                                  child: const Text('Close'),
                                ),
                              ],
                            ),
                          ),
                          child: Text('${issue['state']} · View details'),
                        ),
                      ),
                    if (hasMore)
                      TextButton(
                        onPressed: busy ? null : () => fetch(more: true),
                        child: const Text('Load more'),
                      ),
                  ],
                ),
        ),
        FilledButton(
          onPressed: busy || selected.isEmpty ? null : importSelected,
          child: Text('Add ${selected.length} to Backlog'),
        ),
      ],
    ),
  );
}
