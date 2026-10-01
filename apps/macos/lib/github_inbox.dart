import 'dart:async';
import 'package:flutter/services.dart';
import 'package:flutter/material.dart';
import 'application/task_board_controller.dart';
import 'data/task_models.dart';

class GitHubRepositoryPicker extends StatefulWidget {
  const GitHubRepositoryPicker({
    super.key,
    required this.request,
    required this.projects,
  });
  final TaskTransport request;
  final List<TaskProjectOption> projects;
  @override
  State<GitHubRepositoryPicker> createState() => _GitHubRepositoryPickerState();
}

class _GitHubRepositoryPickerState extends State<GitHubRepositoryPicker> {
  final input = TextEditingController();
  String? project, error;
  List<Map<String, dynamic>> repositories = [];
  bool busy = false, hasMore = true;
  int page = 0;
  @override
  void initState() {
    super.initState();
    project = widget.projects.firstOrNull?.id;
  }

  @override
  void dispose() {
    if (busy) {
      unawaited(
        githubRequest(
          widget.request,
          'CancelRead',
        ).then<void>((_) {}, onError: (Object _) {}),
      );
    }
    input.dispose();
    super.dispose();
  }

  Future<void> browse() async {
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final result = await githubRequest(widget.request, {
        'Repositories': {'page': page + 1},
      });
      if (!mounted) return;
      setState(() {
        repositories.addAll(
          (result['repositories'] as List).map(
            (v) => Map<String, dynamic>.from(v as Map),
          ),
        );
        page++;
        hasMore = result['has_more'] == true;
      });
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  Future<void> link() async {
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await githubRequest(widget.request, {
        'Link': {'project_id': project, 'repository': input.text.trim()},
      });
      if (mounted) Navigator.pop(context, true);
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Link GitHub repository'),
    content: SizedBox(
      width: 520,
      height: 420,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          DropdownButton<String>(
            value: project,
            isExpanded: true,
            items: [
              for (final p in widget.projects)
                DropdownMenuItem(value: p.id, child: Text(p.name)),
            ],
            onChanged: busy ? null : (v) => setState(() => project = v),
          ),
          TextField(
            controller: input,
            enabled: !busy,
            decoration: const InputDecoration(
              labelText: 'OWNER/REPO or GitHub URL',
            ),
          ),
          const SizedBox(height: 12),
          const Text(
            'Browse repositories you can access, including organization and collaborator repositories. Linking does not clone or modify them.',
          ),
          if (busy) const LinearProgressIndicator(),
          if (error != null)
            Text(
              error!,
              style: TextStyle(color: Theme.of(context).colorScheme.error),
            ),
          Expanded(
            child: ListView(
              children: [
                for (final repo in repositories)
                  ListTile(
                    title: Text('${repo['full_name']}'),
                    subtitle: Text(
                      repo['private'] == true ? 'Private' : 'Public',
                    ),
                    onTap: busy
                        ? null
                        : () => setState(
                            () => input.text = '${repo['full_name']}',
                          ),
                  ),
                if (hasMore)
                  TextButton(
                    onPressed: busy ? null : browse,
                    child: Text(
                      page == 0
                          ? 'Browse accessible repositories'
                          : 'Load more repositories',
                    ),
                  ),
                if (page > 0 && repositories.isEmpty)
                  const Text(
                    'No repositories available. You can also enter a repository directly.',
                  ),
              ],
            ),
          ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: busy || project == null ? null : link,
        child: const Text('Verify and link'),
      ),
    ],
  );
}

class GitHubIssueDetail extends StatefulWidget {
  const GitHubIssueDetail({
    super.key,
    required this.request,
    required this.issue,
    required this.link,
  });
  final TaskTransport request;
  final Map<String, dynamic> issue, link;
  @override
  State<GitHubIssueDetail> createState() => _GitHubIssueDetailState();
}

class _GitHubIssueDetailState extends State<GitHubIssueDetail> {
  List<Map<String, dynamic>> comments = [];
  bool busy = false, hasMore = true;
  String? error;
  int page = 0;
  @override
  void dispose() {
    if (busy) {
      unawaited(
        githubRequest(
          widget.request,
          'CancelRead',
        ).then<void>((_) {}, onError: (Object _) {}),
      );
    }
    super.dispose();
  }

  Future<void> loadComments() async {
    setState(() {
      busy = true;
      error = null;
    });
    try {
      final result = await githubRequest(widget.request, {
        'Comments': {
          'project_id': widget.link['project_id'],
          'repository_id': widget.link['repository']['id'],
          'number': widget.issue['number'],
          'page': page + 1,
        },
      });
      if (!mounted) return;
      setState(() {
        comments.addAll(
          (result['comments'] as List).map(
            (v) => Map<String, dynamic>.from(v as Map),
          ),
        );
        page++;
        hasMore = result['has_more'] == true;
      });
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final issue = widget.issue;
    return AlertDialog(
      title: Text('${issue['title']}'),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              // Literal Markdown keeps HTML inert and never fetches remote images.
              SelectableText(
                '${issue['html_url']}\n${issue['state']} · ${issue['state_reason'] ?? ''}\nAuthor: ${issue['user']?['login'] ?? 'Unknown'}\nCreated: ${issue['created_at'] ?? ''} · Updated: ${issue['updated_at'] ?? ''}\nLabels: ${(issue['labels'] as List? ?? []).map((v) => v['name']).join(', ')}\nAssignees: ${(issue['assignees'] as List? ?? []).map((v) => v['login']).join(', ')}\n\n${issue['body'] ?? ''}',
              ),
              const Divider(),
              for (final comment in comments)
                Padding(
                  padding: const EdgeInsets.only(bottom: 16),
                  child: SelectableText(
                    '${comment['user']?['login'] ?? 'Deleted user'} · ${comment['created_at'] ?? ''}\n${comment['body'] ?? ''}',
                  ),
                ),
              if (busy) const LinearProgressIndicator(),
              if (error != null)
                Text(
                  error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              if (hasMore)
                TextButton(
                  onPressed: busy ? null : loadComments,
                  child: Text(
                    page == 0
                        ? 'Load comments (${issue['comments'] ?? 0})'
                        : 'Load more comments',
                  ),
                ),
              if (page > 0 && comments.isEmpty) const Text('No comments.'),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Close'),
        ),
      ],
    );
  }
}

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
  String? error, attemptedGeneration;
  bool sending = false, polling = false, browserFailed = false;
  int statusEpoch = 0;
  Timer? timer;
  bool get active => status?['busy'] == true;

  @override
  void initState() {
    super.initState();
    poll();
    timer = Timer.periodic(const Duration(seconds: 1), (_) => poll());
  }

  @override
  void dispose() {
    timer?.cancel();
    if (active) {
      unawaited(
        githubRequest(
          widget.request,
          'Cancel',
        ).then<void>((_) {}, onError: (Object _) {}),
      );
    }
    super.dispose();
  }

  Future<void> openBrowser() async {
    final epoch = statusEpoch;
    try {
      await githubRequest(widget.request, 'OpenBrowser');
      if (mounted && epoch == statusEpoch) {
        setState(() {
          browserFailed = false;
          error = null;
        });
      }
    } on Object catch (_) {
      if (mounted && epoch == statusEpoch) {
        setState(() {
          browserFailed = true;
          error = 'Could not open your browser. Try again below.';
        });
      }
    }
  }

  Future<void> poll() async {
    if (polling || sending) return;
    polling = true;
    final epoch = statusEpoch;
    try {
      final result = await githubRequest(widget.request, 'Status');
      if (!mounted || epoch != statusEpoch) return;
      setState(() {
        status = result;
        if (result['connected'] == true) error = null;
      });
      if (result['browser_ready'] == true &&
          attemptedGeneration != result['generation']) {
        attemptedGeneration = result['generation'] as String?;
        await openBrowser();
      }
    } on Object catch (e) {
      if (mounted && epoch == statusEpoch) setState(() => error = '$e');
    } finally {
      polling = false;
    }
  }

  Future<void> action(String operation) async {
    if (sending) return;
    statusEpoch++;
    setState(() {
      sending = true;
      error = null;
      browserFailed = false;
    });
    try {
      final result = await githubRequest(widget.request, operation);
      if (mounted) setState(() => status = result);
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => sending = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final phase = status?['connection_state'];
    final connected = status?['connected'] == true;
    final account = status?['account'] as Map?;
    final code = status?['device_code'] as String?;
    final failed =
        phase == 'failed' || phase == 'account_changed' || error != null;
    return PopScope(
      canPop: !sending,
      child: AlertDialog(
        title: const Text('GitHub'),
        content: SizedBox(
          width: 440,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (active || sending || status == null) ...[
                  const LinearProgressIndicator(),
                  const SizedBox(height: 16),
                ],
                if (connected)
                  Text(
                    'Connected as @${account?['login'] ?? 'GitHub user'}',
                    style: Theme.of(context).textTheme.titleMedium,
                  )
                else if (code != null) ...[
                  const Text(
                    'Enter this code in your browser to connect GitHub.',
                  ),
                  const SizedBox(height: 12),
                  SelectableText(
                    code,
                    style: Theme.of(context).textTheme.headlineSmall,
                  ),
                  Wrap(
                    children: [
                      TextButton(
                        onPressed: () =>
                            Clipboard.setData(ClipboardData(text: code)),
                        child: const Text('Copy code'),
                      ),
                      TextButton(
                        onPressed: sending ? null : openBrowser,
                        child: Text(
                          browserFailed ? 'Open browser' : 'Open browser again',
                        ),
                      ),
                    ],
                  ),
                ] else if (active || failed)
                  Text(
                    status?['detail'] as String? ??
                        'Preparing browser sign-in…',
                  )
                else
                  const Text(
                    'Connect GitHub in your browser to browse issues and add them to Backlog.',
                  ),
                if (error != null) ...[
                  const SizedBox(height: 12),
                  Text(
                    error!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                ],
                const SizedBox(height: 16),
                const Text(
                  'Imports do not start agents or change GitHub issues.',
                ),
                const SizedBox(height: 12),
                Text(
                  connected
                      ? 'Disconnecting Ditch keeps the shared GitHub sign-in available to other tools.'
                      : 'Connecting may update the GitHub CLI sign-in shared with other tools on this Mac.',
                ),
              ],
            ),
          ),
        ),
        actions: [
          if (!active)
            TextButton(
              onPressed: sending ? null : () => Navigator.pop(context),
              child: Text(connected ? 'Done' : 'Close'),
            ),
          if (active)
            TextButton(
              onPressed: sending ? null : () => action('Cancel'),
              child: const Text('Cancel'),
            )
          else if (connected)
            TextButton(
              onPressed: sending ? null : () => action('Disconnect'),
              child: const Text('Disconnect'),
            )
          else
            FilledButton(
              onPressed: sending
                  ? null
                  : () => status == null ? poll() : action('Connect'),
              child: Text(failed ? 'Try again' : 'Connect GitHub'),
            ),
        ],
      ),
    );
  }
}

class GitHubInbox extends StatefulWidget {
  const GitHubInbox({
    super.key,
    required this.request,
    required this.projects,
    required this.onImported,
    this.onOpenTask,
  });
  final TaskTransport request;
  final List<TaskProjectOption> projects;
  final Future<void> Function() onImported;
  final ValueChanged<String>? onOpenTask;
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
  bool connected = false, checkingConnection = false;
  String? connectionGeneration, importRequestId, readProgress;
  bool linksLoaded = false, loadingLinks = false;
  int requestGeneration = 0;
  Map<String, dynamic> imported = {};
  Timer? connectionTimer;
  BuildContext? privateDialogContext;
  int page = 0;
  @override
  void initState() {
    super.initState();
    checkConnection();
    connectionTimer = Timer.periodic(
      const Duration(seconds: 2),
      (_) => checkConnection(),
    );
  }

  @override
  void dispose() {
    connectionTimer?.cancel();
    if (busy) {
      unawaited(
        githubRequest(
          widget.request,
          'CancelRead',
        ).then<void>((_) {}, onError: (Object _) {}),
      );
    }
    super.dispose();
  }

  Future<void> checkConnection() async {
    if (checkingConnection) return;
    checkingConnection = true;
    try {
      final result = await githubRequest(widget.request, 'Status');
      if (!mounted) return;
      final next = result['connected'] == true;
      final changed =
          result['generation'] != connectionGeneration || next != connected;
      if (busy && readProgress != result['read_progress']) {
        setState(() => readProgress = result['read_progress'] as String?);
      }
      if (changed) {
        final dialog = privateDialogContext;
        if (dialog != null && dialog.mounted) Navigator.pop(dialog);
        privateDialogContext = null;
        setState(() {
          connected = next;
          connectionGeneration = result['generation'] as String?;
          requestGeneration++;
          links = [];
          linksLoaded = false;
          readProgress = null;
          issues = [];
          link = null;
          selected.clear();
          imported = {};
          lastSync = null;
          importRequestId = null;
          page = 0;
          hasMore = false;
          busy = false;
          error = next
              ? null
              : 'Connect GitHub to browse issues. Imported tasks remain on your board.';
        });
      }
      if (next && !linksLoaded) await loadLinks();
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      checkingConnection = false;
    }
  }

  Future<void> loadLinks() async {
    if (loadingLinks) return;
    loadingLinks = true;
    final generation = requestGeneration;
    try {
      final result = await githubRequest(widget.request, {
        'Links': {'project_id': null},
      });
      if (!mounted || generation != requestGeneration) return;
      final visible = widget.projects.map((p) => p.id).toSet();
      setState(() {
        linksLoaded = true;
        error = null;
        links = (result['repositories'] as List)
            .map((v) => Map<String, dynamic>.from(v as Map))
            .where((v) => visible.contains(v['project_id']))
            .toList();
      });
    } on Object catch (e) {
      if (mounted && generation == requestGeneration) {
        setState(() => error = '$e');
      }
    } finally {
      loadingLinks = false;
    }
  }

  Future<void> linkRepository() async {
    final linked = await showDialog<bool>(
      context: context,
      builder: (context) {
        privateDialogContext = context;
        return GitHubRepositoryPicker(
          request: widget.request,
          projects: widget.projects,
        );
      },
    );
    if (linked == true && mounted) await loadLinks();
  }

  Future<void> fetch({bool more = false}) async {
    final current = link;
    if (current == null || busy) return;
    final generation = requestGeneration;
    setState(() {
      busy = true;
      readProgress = null;
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
      if (!mounted || generation != requestGeneration) return;
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
        imported = Map<String, dynamic>.from(result['imported'] as Map? ?? {});
        page = next;
        hasMore = result['has_more'] == true;
        lastSync = result['last_synced_at']?.toString();
      });
    } on Object catch (e) {
      if (mounted && generation == requestGeneration) {
        setState(
          () => error = 'Could not refresh. Displayed issues may be stale. $e',
        );
      }
    } finally {
      if (mounted && generation == requestGeneration) {
        setState(() => busy = false);
      }
    }
  }

  Future<void> importSelected() async {
    if (link == null || selected.isEmpty || busy) return;
    final current = link!;
    final numbers = selected.toList();
    final generation = requestGeneration;
    final confirm = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Add ${selected.length} issues to Backlog?'),
        content: const Text(
          'This confirms the selected work for your backlog. Closed GitHub issues retain their Closed source badge; they are not marked accepted in Ditch. No agents will start. Previously imported issues keep their local work and board position.',
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
    if (confirm != true || !mounted || generation != requestGeneration) return;
    importRequestId ??= TaskBoardController.newRequestId();
    setState(() {
      busy = true;
      readProgress = 'Preparing import…';
      error = null;
    });
    try {
      await githubRequest(widget.request, {
        'Import': {
          'project_id': current['project_id'],
          'repository_id': current['repository']['id'],
          'numbers': numbers,
          'request_id': importRequestId,
        },
      });
      await widget.onImported();
      if (mounted && generation == requestGeneration) {
        setState(() {
          selected.clear();
          importRequestId = null;
          busy = false;
        });
        await fetch();
      }
    } on Object catch (e) {
      if (mounted && generation == requestGeneration) {
        setState(() => error = '$e');
      }
    } finally {
      if (mounted && generation == requestGeneration) {
        setState(() => busy = false);
      }
    }
  }

  Future<void> cancelLoading() async {
    setState(() {
      requestGeneration++;
      busy = false;
    });
    try {
      await githubRequest(widget.request, 'CancelRead');
      await widget.onImported();
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
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
              onPressed: busy || !connected ? null : linkRepository,
              child: const Text('Link repository'),
            ),
            TextButton(
              onPressed: () async {
                await showDialog<void>(
                  context: context,
                  builder: (_) => GitHubSettingsDialog(request: widget.request),
                );
                await checkConnection();
              },
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
                    imported = {};
                    lastSync = null;
                    hasMore = false;
                    importRequestId = null;
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
        if (busy) ...[
          const LinearProgressIndicator(),
          if (readProgress != null) Text(readProgress!),
          TextButton(
            onPressed: cancelLoading,
            child: const Text('Cancel loading'),
          ),
        ],
        if (error != null)
          Text(
            error!,
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          ),
        if (connected && !linksLoaded)
          TextButton(
            onPressed: loadingLinks ? null : loadLinks,
            child: const Text('Retry repositories'),
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
                        secondary:
                            imported['${issue['id']}'] is String &&
                                widget.onOpenTask != null
                            ? IconButton(
                                tooltip: 'Open imported task',
                                icon: const Icon(Icons.open_in_new),
                                onPressed: () => widget.onOpenTask!(
                                  imported['${issue['id']}'] as String,
                                ),
                              )
                            : null,
                        value: selected.contains(issue['number']),
                        onChanged:
                            busy || imported.containsKey('${issue['id']}')
                            ? null
                            : (v) => setState(() {
                                importRequestId = null;
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
                            builder: (context) {
                              privateDialogContext = context;
                              return GitHubIssueDetail(
                                request: widget.request,
                                issue: issue,
                                link: link!,
                              );
                            },
                          ),
                          child: Text(
                            '${issue['state']} · ${imported.containsKey('${issue['id']}') ? 'Already imported · ' : ''}View details',
                          ),
                        ),
                      ),
                  ],
                ),
        ),
        if (hasMore)
          TextButton(
            onPressed: busy ? null : () => fetch(more: true),
            child: const Text('Load more'),
          ),
        FilledButton(
          onPressed:
              busy || !connected || selected.isEmpty || selected.length > 50
              ? null
              : importSelected,
          child: Text('Add ${selected.length} to Backlog'),
        ),
        if (selected.length > 50)
          const Text('Select up to 50 issues per import.'),
      ],
    ),
  );
}
