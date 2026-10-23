import 'dart:async';
import 'dart:convert';
import 'package:flutter/services.dart';
import 'package:flutter/material.dart';
import 'application/task_board_controller.dart';
import 'data/task_models.dart';
import 'data/task_templates.dart';
import 'widgets/task_type_selector.dart';
import 'release_ui_visibility.dart';

/// Memory-only cache scoped to the app's runtime connection, not a pane mount.
/// Bound request methods compare by receiver, so Settings shares invalidation.
class GitHubBoardSession extends ChangeNotifier {
  static final _sessions = <TaskTransport, GitHubBoardSession>{};
  static GitHubBoardSession forRequest(TaskTransport request) =>
      _sessions.putIfAbsent(request, GitHubBoardSession.new);
  Map<String, dynamic>? status;
  Future<Map<String, dynamic>>? statusRead;
  final reads = <String, Future<Map<String, dynamic>>>{};
  final snapshots = <String, Map<String, dynamic>>{};
  final selections = <String, Map<String, dynamic>>{};
  bool initialIssuesClaimed = false;
  Future<void>? initialRead;
  int epoch = 0;

  void observeStatus(Map<String, dynamic> next) {
    final changed =
        status != null &&
        (status!['generation'] != next['generation'] ||
            status!['connected'] != next['connected']);
    status = next;
    if (changed) {
      epoch++;
      reads.clear();
      snapshots.clear();
      selections.clear();
      notifyListeners();
    }
  }

  Future<Map<String, dynamic>> read(
    Object operation,
    TaskTransport request, {
    bool force = false,
  }) {
    final key = jsonEncode(operation);
    if (force) reads.remove(key);
    return reads.putIfAbsent(key, () => githubRequest(request, operation));
  }

  void recordImport(Map operation, Map result) {
    final numbers = operation['numbers'] as List? ?? [];
    final ids = result['task_ids'] as List? ?? [];
    for (final entry in snapshots.entries) {
      if (!entry.key.startsWith(
        '${operation['project_id']}:${operation['repository_id']}:',
      )) {
        continue;
      }
      final imported = Map<String, dynamic>.from(
        entry.value['imported'] as Map? ?? {},
      );
      for (final issue in entry.value['issues'] as List) {
        final index = numbers.indexOf(issue['number']);
        if (index >= 0 && index < ids.length) {
          imported['${issue['id']}'] = ids[index];
        }
      }
      entry.value['imported'] = imported;
    }
    notifyListeners();
  }
}

/// A snapshot of one drag. The source validates its connection again on drop.
class GitHubIssueDrag {
  const GitHubIssueDrag({
    required this.count,
    required this.canDrop,
    required this.importToBacklog,
  });
  final int count;
  final bool Function() canDrop;
  final Future<void> Function() importToBacklog;
}

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
  bool busy = false, hasMore = true, loadingSuggestions = false;
  bool linking = false;
  int inputRevision = 0;
  int page = 0;
  List<String> suggestions = [];
  int suggestionEpoch = 0;
  List<Map<String, dynamic>> submodules = [];
  List<String> discoveryWarnings = [];
  final selectedSubmodules = <String>{};
  final linkedRepositories = <String>{};
  @override
  void initState() {
    super.initState();
    input.addListener(() {
      inputRevision++;
      if (mounted) setState(() {});
    });
    project = widget.projects.firstOrNull?.id;
    if (showGitHubBoardControls) loadSuggestions();
  }

  Future<void> loadSuggestions() async {
    final id = project;
    final epoch = ++suggestionEpoch;
    if (id == null) return;
    final revision = inputRevision;
    setState(() => loadingSuggestions = true);
    try {
      final result = await githubRequest(widget.request, {
        'RemoteSuggestions': {'project_id': id},
      });
      if (mounted && epoch == suggestionEpoch) {
        setState(() {
          suggestions = (result['repositories'] as List).cast<String>();
          submodules = (result['candidates'] as List? ?? [])
              .map((v) => Map<String, dynamic>.from(v as Map))
              .where((v) => v['is_root'] != true)
              .toList();
          discoveryWarnings = (result['warnings'] as List? ?? [])
              .cast<String>();
          selectedSubmodules.clear();
          if (suggestions.isNotEmpty &&
              input.text.isEmpty &&
              inputRevision == revision) {
            input.text = 'https://github.com/${suggestions.first}';
          }
        });
      }
    } on Object catch (_) {
      if (mounted && epoch == suggestionEpoch) setState(() => suggestions = []);
    } finally {
      if (mounted && epoch == suggestionEpoch) {
        setState(() => loadingSuggestions = false);
      }
    }
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
    if (busy || project == null || input.text.trim().isEmpty) return;
    setState(() {
      busy = true;
      linking = true;
      error = null;
    });
    try {
      final failures = <String>[];
      for (final repository in {input.text.trim(), ...selectedSubmodules}) {
        if (linkedRepositories.contains('$project:$repository')) continue;
        try {
          await githubRequest(widget.request, {
            'Link': {'project_id': project, 'repository': repository},
          });
          linkedRepositories.add('$project:$repository');
        } on Object catch (e) {
          failures.add('$repository: $e');
        }
        if (!mounted) return;
      }
      if (failures.isEmpty) {
        if (mounted) Navigator.pop(context, true);
      } else if (mounted) {
        setState(() => error = failures.join('\n'));
      }
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) {
        setState(() {
          busy = false;
          linking = false;
        });
      }
    }
  }

  Future<void> manageAccess() async {
    setState(() {
      busy = true;
      error = null;
    });
    try {
      await githubRequest(widget.request, 'ManageRepositories');
      if (mounted) {
        setState(() {
          page = 0;
          hasMore = true;
          repositories = [];
        });
      }
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) setState(() => busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Link GitHub repository'),
    scrollable: true,
    content: SizedBox(
      width: 560,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'Choose the repository whose issues you want to use in this project.',
          ),
          const SizedBox(height: 20),
          DropdownButtonFormField<String>(
            initialValue: project,
            isExpanded: true,
            decoration: const InputDecoration(
              labelText: 'Project',
              border: OutlineInputBorder(),
            ),
            items: [
              for (final p in widget.projects)
                DropdownMenuItem(
                  value: p.id,
                  child: Text(p.name, overflow: TextOverflow.ellipsis),
                ),
            ],
            onChanged: busy
                ? null
                : (v) {
                    setState(() {
                      project = v;
                      suggestions = [];
                      submodules = [];
                      selectedSubmodules.clear();
                      error = null;
                      input.clear();
                    });
                    if (showGitHubBoardControls) loadSuggestions();
                  },
          ),
          const SizedBox(height: 20),
          TextField(
            controller: input,
            enabled: !busy && project != null,
            onChanged: (_) {
              if (error != null) setState(() => error = null);
            },
            onSubmitted: (_) => link(),
            decoration: const InputDecoration(
              labelText: 'GitHub repository',
              hintText: 'https://github.com/owner/repository',
              helperText: 'GitHub URL or owner/repository',
              border: OutlineInputBorder(),
            ),
          ),
          if (loadingSuggestions) ...[
            const SizedBox(height: 8),
            const Text('Checking project Git remotes…'),
          ] else if (suggestions.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(
              suggestions.length == 1
                  ? 'Detected from this project’s Git remote.'
                  : 'Multiple Git remotes found. Choose the repository to link.',
            ),
            if (suggestions.length > 1)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final name in suggestions)
                      ActionChip(
                        label: Text(name),
                        onPressed: busy
                            ? null
                            : () {
                                input.text = 'https://github.com/$name';
                                setState(() => error = null);
                              },
                      ),
                  ],
                ),
              ),
          ],
          if (submodules.isNotEmpty) ...[
            const SizedBox(height: 12),
            const Text('Also link submodules'),
            for (final module in submodules)
              CheckboxListTile(
                dense: true,
                contentPadding: EdgeInsets.zero,
                controlAffinity: ListTileControlAffinity.leading,
                title: Text(module['repository'] as String),
                subtitle: Text((module['paths'] as List).join(', ')),
                value: selectedSubmodules.contains(module['repository']),
                onChanged: busy
                    ? null
                    : (selected) => setState(() {
                        if (selected == true) {
                          selectedSubmodules.add(
                            module['repository'] as String,
                          );
                        } else {
                          selectedSubmodules.remove(module['repository']);
                        }
                      }),
              ),
          ],
          if (discoveryWarnings.isNotEmpty)
            Text(
              discoveryWarnings.join('\n'),
              style: Theme.of(context).textTheme.bodySmall,
            ),
          const SizedBox(height: 16),
          Wrap(
            spacing: 12,
            runSpacing: 8,
            children: [
              OutlinedButton.icon(
                onPressed: busy
                    ? null
                    : () async {
                        setState(() {
                          page = 0;
                          hasMore = true;
                          repositories = [];
                        });
                        await browse();
                      },
                icon: const Icon(Icons.search, size: 18),
                label: Text(
                  page == 0 ? 'Browse repositories' : 'Refresh repositories',
                ),
              ),
              if (showGitHubBoardControls)
                TextButton.icon(
                  onPressed: busy ? null : manageAccess,
                  icon: const Icon(Icons.open_in_new, size: 16),
                  label: const Text('Manage GitHub access'),
                ),
            ],
          ),
          if (busy) ...[
            const SizedBox(height: 12),
            const LinearProgressIndicator(),
          ],
          if (error != null) ...[
            const SizedBox(height: 12),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.errorContainer,
                borderRadius: BorderRadius.circular(8),
              ),
              child: SelectableText(
                error!.replaceFirst(
                  RegExp(r'^(Bad state: )?github_unavailable: '),
                  '',
                ),
                style: TextStyle(
                  color: Theme.of(context).colorScheme.onErrorContainer,
                ),
              ),
            ),
          ],
          if (repositories.isNotEmpty) ...[
            const SizedBox(height: 12),
            SizedBox(
              height: repositories.length < 4
                  ? repositories.length * 60.0
                  : 240,
              child: ListView(
                children: [
                  for (final repo in repositories)
                    ListTile(
                      dense: true,
                      title: Text('${repo['full_name']}'),
                      selected:
                          input.text ==
                              'https://github.com/${repo['full_name']}' ||
                          input.text == repo['full_name'],
                      trailing: const Icon(Icons.arrow_forward, size: 18),
                      onTap: busy
                          ? null
                          : () {
                              input.text =
                                  'https://github.com/${repo['full_name']}';
                              setState(() => error = null);
                            },
                    ),
                ],
              ),
            ),
          ],
          if (page > 0 && hasMore)
            TextButton(
              onPressed: busy ? null : browse,
              child: const Text('Load more repositories'),
            ),
          if (page > 0 && repositories.isEmpty) ...[
            const SizedBox(height: 12),
            const Text(
              'No accessible repositories found. Check the GitHub App installation’s access, then refresh.',
            ),
          ],
          const SizedBox(height: 16),
          Text(
            'Linking connects GitHub issues to this project. Your Git remotes stay unchanged.',
            style: Theme.of(context).textTheme.bodySmall,
          ),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: busy
            ? null
            : () => Navigator.pop(context, linkedRepositories.isNotEmpty),
        child: Text(linkedRepositories.isEmpty ? 'Cancel' : 'Done'),
      ),
      FilledButton(
        onPressed: busy || project == null || input.text.trim().isEmpty
            ? null
            : link,
        child: Text(linking ? 'Linking…' : 'Verify and link'),
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
                '${issue['html_url']}\n${issue['state']} · ${issue['state_reason'] ?? ''}\nAuthor: ${issue['user']?['login'] ?? 'Unknown'}\nCreated: ${issue['created_at'] ?? ''} · Updated: ${issue['updated_at'] ?? ''}\nType: ${issue['type']?['name'] ?? 'None'}\nLabels: ${(issue['labels'] as List? ?? []).map((v) => v['name']).join(', ')}\nAssignees: ${(issue['assignees'] as List? ?? []).map((v) => v['login']).join(', ')}\n\n${issue['body'] ?? ''}',
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
  final session = GitHubBoardSession.forRequest(request);
  final epoch = session.epoch;
  final result = await request({'GitHub': operation});
  if (result['Error'] case final Map error) {
    throw StateError('${error['message']}');
  }
  if (result['GitHub'] case final Map body) {
    final value = Map<String, dynamic>.from(body);
    if (operation == 'Status') {
      if (epoch != session.epoch && session.status != null) {
        return session.status!;
      }
      session.observeStatus(value);
    }
    if (operation == 'Disconnect' || operation == 'Cancel') {
      session.observeStatus({'connected': false, 'generation': null});
    }
    if (epoch == session.epoch &&
        operation is Map &&
        operation['Import'] is Map) {
      session.recordImport(operation['Import'] as Map, value);
    }
    return value;
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
  bool statusFailed = false;
  int statusEpoch = 0;
  Timer? timer;
  bool get active => status?['busy'] == true;

  @override
  void initState() {
    super.initState();
    poll();
    timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (active) poll();
    });
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
        if (statusFailed || result['connected'] == true) error = null;
        statusFailed = false;
      });
      if (result['browser_ready'] == true &&
          attemptedGeneration != result['generation']) {
        attemptedGeneration = result['generation'] as String?;
        await openBrowser();
      }
    } on Object catch (e) {
      if (mounted && epoch == statusEpoch) {
        setState(() {
          error = '$e';
          statusFailed = true;
        });
      }
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
      statusFailed = false;
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
    final app = status?['provider'] == 'github_app' || showGitHubBoardControls;
    final code = status?['device_code'] as String?;
    final detail = status?['detail'] as String?;
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
                if (sending || (!failed && (active || status == null))) ...[
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
                    failed
                        ? error == null
                              ? detail ?? 'GitHub connection needs attention.'
                              : 'GitHub connection needs attention.'
                        : detail ?? 'Preparing browser sign-in…',
                  )
                else
                  Text(
                    app
                        ? 'Connect the Ditch GitHub App in your browser, choose its repository access, then link repositories to a project.'
                        : 'Connect GitHub in your browser to browse issues and add them to Backlog.',
                  ),
                if (error != null) ...[
                  const SizedBox(height: 12),
                  SelectableText(
                    error!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                ],
                if (app && code == null && active && browserFailed)
                  TextButton(
                    onPressed: sending ? null : openBrowser,
                    child: const Text('Open browser'),
                  ),
                const SizedBox(height: 16),
                const Text(
                  'Imports do not start agents or change GitHub issues.',
                ),
                const SizedBox(height: 12),
                Text(
                  app
                      ? 'Ditch Relay processes GitHub authorization and issue data. Issue creation requires your confirmation. GitHub CLI credentials are not used.'
                      : connected
                      ? 'Disconnecting Ditch keeps the shared GitHub sign-in available to other tools.'
                      : 'Connecting may update the GitHub CLI sign-in shared with other tools on this Mac.',
                ),
              ],
            ),
          ),
        ),
        actions: [
          if (app && connected)
            TextButton(
              onPressed: sending
                  ? null
                  : () async {
                      try {
                        await githubRequest(
                          widget.request,
                          'ManageRepositories',
                        );
                      } on Object catch (e) {
                        if (mounted) setState(() => error = '$e');
                      }
                    },
              child: const Text('Repositories'),
            ),
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

class GitHubCreateIssueDialog extends StatefulWidget {
  const GitHubCreateIssueDialog({
    required this.request,
    this.link,
    this.task,
    this.repositoryLinks,
    super.key,
  });
  final TaskTransport request;
  final Map<String, dynamic>? link;
  final TaskDto? task;
  final List<Map<String, dynamic>>? repositoryLinks;
  @override
  State<GitHubCreateIssueDialog> createState() => _GitHubCreateIssueState();
}

class _GitHubCreateIssueState extends State<GitHubCreateIssueDialog> {
  late final title = TextEditingController(text: widget.task?.title ?? '');
  late final body = TextEditingController(
    text: widget.task?.description ?? taskTemplates['Custom'],
  );
  late String issueKind = widget.task?.issueKind ?? 'Custom';
  String requestId = TaskBoardController.newRequestId();
  String? issueType, error;
  List<Map<String, dynamic>> links = [], types = [];
  Map<String, dynamic>? link;
  bool sending = false, attempted = false, loading = true, canSetType = false;

  @override
  void initState() {
    super.initState();
    load();
  }

  Future<void> load() async {
    try {
      final pending = widget.task == null
          ? null
          : (await githubRequest(widget.request, {
                  'Publication': {'task_id': widget.task!.id},
                }))['publication']
                as Map?;
      if (widget.link != null) {
        links = [widget.link!];
      } else if (widget.repositoryLinks != null) {
        links = widget.repositoryLinks!;
      } else {
        final result = await githubRequest(widget.request, {
          'Links': {'project_id': widget.task!.projectId},
        });
        links = (result['repositories'] as List)
            .map((v) => Map<String, dynamic>.from(v as Map))
            .toList();
      }
      if (!mounted) return;
      link = pending == null
          ? links.firstOrNull
          : links
                .where((v) => v['repository']['id'] == pending['repository_id'])
                .firstOrNull;
      if (pending != null) {
        requestId = pending['request_id'] as String;
        title.text = pending['title'] as String;
        body.text = pending['body'] as String;
        issueKind = pending['issue_kind'] as String? ?? issueKind;
        issueType = pending['issue_type'] as String?;
        attempted = true;
      }
      if (link != null) await loadTypes();
      if (links.isEmpty || link == null) {
        error = 'Link the destination repository from the GitHub pane first.';
      }
    } on Object catch (e) {
      error = '$e';
    } finally {
      if (mounted) setState(() => loading = false);
    }
  }

  Future<void> loadTypes() async {
    final current = link!;
    setState(() {
      loading = true;
      error = null;
    });
    try {
      final result = await githubRequest(widget.request, {
        'IssueTypes': {
          'project_id': current['project_id'],
          'repository_id': current['repository']['id'],
        },
      });
      if (!mounted) return;
      types = (result['types'] as List? ?? [])
          .map((v) => Map<String, dynamic>.from(v as Map))
          .toList();
      canSetType = result['can_set_type'] == true;
      if (!attempted) {
        issueType = canSetType
            ? (types
                      .where(
                        (t) =>
                            (t['name'] as String).toLowerCase() ==
                            issueKind.toLowerCase(),
                      )
                      .firstOrNull?['name']
                  as String?)
            : null;
      }
    } on Object catch (e) {
      if (mounted) error = 'Could not load GitHub types: $e';
    } finally {
      if (mounted) setState(() => loading = false);
    }
  }

  @override
  void dispose() {
    title.dispose();
    body.dispose();
    super.dispose();
  }

  Future<void> create() async {
    if (sending || loading || link == null || title.text.trim().isEmpty) return;
    setState(() {
      sending = true;
      attempted = true;
      error = null;
    });
    try {
      await githubRequest(widget.request, {
        'CreateIssue': {
          'project_id': link!['project_id'],
          'repository_id': link!['repository']['id'],
          'request_id': requestId,
          'task_id': widget.task?.id,
          'title': title.text.trim(),
          'body': body.text,
          'issue_kind': issueKind,
          'issue_type': issueType,
        },
      });
      if (mounted) Navigator.pop(context, true);
    } on Object catch (e) {
      if (mounted) {
        setState(() {
          error = '$e';
          if (error!.contains(
            'Ditch Relay rejected the request (github_issue_type_unavailable):',
          )) {
            attempted = false;
            requestId = TaskBoardController.newRequestId();
          }
        });
      }
    } finally {
      if (mounted) setState(() => sending = false);
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
    canPop: !sending,
    child: AlertDialog(
      title: Text(
        widget.task == null ? 'Create GitHub issue' : 'Create on GitHub',
      ),
      content: SizedBox(
        width: 560,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (links.isNotEmpty)
                DropdownButtonFormField<String>(
                  key: ValueKey(
                    'publish-repository-${link?['repository']['id']}',
                  ),
                  initialValue: link == null
                      ? null
                      : "${link!['project_id']}:${link!['repository']['id']}",
                  decoration: const InputDecoration(labelText: 'Repository'),
                  items: [
                    for (final entry in links)
                      DropdownMenuItem(
                        value:
                            "${entry['project_id']}:${entry['repository']['id']}",
                        child: Text(entry['repository']['full_name'] as String),
                      ),
                  ],
                  onChanged: attempted || loading
                      ? null
                      : (id) {
                          setState(
                            () => link = links.firstWhere(
                              (v) =>
                                  "${v['project_id']}:${v['repository']['id']}" ==
                                  id,
                            ),
                          );
                          loadTypes();
                        },
                ),
              TaskTypeSelector(
                value: issueKind,
                onChanged: attempted || loading
                    ? null
                    : (kind) {
                        setState(() {
                          if (body.text.isEmpty ||
                              body.text == taskTemplates[issueKind]) {
                            body.text = taskTemplates[kind]!;
                          }
                          issueKind = kind;
                          issueType = canSetType
                              ? (types
                                        .where(
                                          (v) =>
                                              (v['name'] as String)
                                                  .toLowerCase() ==
                                              kind.toLowerCase(),
                                        )
                                        .firstOrNull?['name']
                                    as String?)
                              : null;
                        });
                      },
              ),
              if (canSetType || issueType != null)
                DropdownButtonFormField<String>(
                  key: ValueKey('publish-type-$issueType'),
                  initialValue: issueType ?? '',
                  decoration: const InputDecoration(labelText: 'GitHub type'),
                  items: [
                    const DropdownMenuItem(
                      value: '',
                      child: Text('No GitHub type'),
                    ),
                    for (final type in types)
                      DropdownMenuItem(
                        value: type['name'] as String,
                        child: Text(type['name'] as String),
                      ),
                    if (issueType != null &&
                        !types.any((v) => v['name'] == issueType))
                      DropdownMenuItem(
                        value: issueType,
                        child: Text(issueType!),
                      ),
                  ],
                  onChanged: attempted || loading
                      ? null
                      : (value) => setState(
                          () => issueType = value == '' ? null : value,
                        ),
                ),
              TextField(
                controller: title,
                enabled: !attempted,
                maxLength: 256,
                decoration: const InputDecoration(labelText: 'Issue title'),
              ),
              TextField(
                controller: body,
                enabled: !attempted,
                minLines: 6,
                maxLines: 14,
                maxLength: 65536,
                decoration: const InputDecoration(
                  labelText: 'Description (Markdown)',
                ),
              ),
              Text(
                widget.task == null
                    ? 'Publishes to GitHub and adds to Backlog. No agent will start.'
                    : 'Publishes to GitHub and links this task. Board state and agent stay the same.',
              ),
              if (loading || sending) const LinearProgressIndicator(),
              if (error != null)
                Text(
                  error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              if (attempted && error != null)
                const Text(
                  'Retry uses the same publication. An uncertain result will not be sent twice.',
                ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: sending ? null : () => Navigator.pop(context, false),
          child: const Text('Close'),
        ),
        FilledButton(
          onPressed: sending || loading || link == null ? null : create,
          child: Text(attempted ? 'Retry same request' : 'Create issue'),
        ),
      ],
    ),
  );
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
  late final session = GitHubBoardSession.forRequest(widget.request);
  String get scope =>
      (widget.projects.map((p) => p.id).toList()..sort()).join(':');
  String get query =>
      '${link?['project_id']}:${link?['repository']['id']}:$filter';
  bool listLoaded = false;
  List<Map<String, dynamic>> links = [], issues = [];
  Map<String, dynamic>? link;
  final selected = <String>{};
  String? selectionAnchor;
  static final allRepositories = <String, dynamic>{
    'all': true,
    'repository': {'id': 0},
  };
  List<Map<String, dynamic>> get visibleLinks =>
      link?['all'] == true ? links : [?link];
  String sourceKey(Map v) => "${v['project_id']}:${v['repository']['id']}";
  String issueKey(Map v) => "${sourceKey(v['_link'] as Map)}:${v['id']}";
  String snapshotKey(Map v) => "${sourceKey(v)}:$filter";
  final importRequests = <String, String>{};
  void restoreVisible() {
    final rows = <Map<String, dynamic>>[];
    final admitted = <String, dynamic>{};
    var loaded = false, more = false;
    String? synced;
    for (final source in visibleLinks) {
      final saved = session.snapshots[snapshotKey(source)];
      if (saved == null) continue;
      loaded = true;
      more |= saved['has_more'] == true;
      synced = saved['last_synced_at']?.toString() ?? synced;
      for (final raw in saved['issues'] as List) {
        final row = <String, dynamic>{
          ...Map<String, dynamic>.from(raw as Map),
          '_link': source,
        };
        rows.add(row);
        final task = (saved['imported'] as Map?)?['${row['id']}'];
        if (task != null) admitted[issueKey(row)] = task;
      }
    }
    setState(() {
      issues = rows;
      imported = admitted;
      hasMore = more;
      lastSync = synced;
      listLoaded = loaded;
    });
  }

  bool confirmingImport = false;
  bool draggingIssues = false;
  bool skipImportConfirmation = false;
  static const preferencesChannel = MethodChannel('the_ditch/application');
  String filter = 'Open';
  String? error, lastSync;
  bool busy = false, hasMore = false;
  bool refreshing = false;
  bool connected = false, checkingConnection = false;
  bool checkedConnection = false;
  String? connectionGeneration, importRequestId, readProgress;
  bool linksLoaded = false, loadingLinks = false;
  int requestGeneration = 0;
  Map<String, dynamic> imported = {};
  BuildContext? privateDialogContext;
  int page = 0;
  @override
  void initState() {
    super.initState();
    unawaited(loadImportPreference());
    session.addListener(connectionChanged);
    checkConnection();
  }

  Future<void> loadImportPreference() async {
    try {
      final skip = await preferencesChannel.invokeMethod<bool>(
        'getSkipGitHubImportConfirmation',
      );
      if (mounted) setState(() => skipImportConfirmation = skip == true);
    } on MissingPluginException {
      /* Tests and older hosts use the safe default. */
    } on PlatformException {
      /* Keep confirmation enabled if preferences cannot load. */
    }
  }

  Future<void> restoreImportConfirmation() async {
    setState(() => skipImportConfirmation = false);
    try {
      await preferencesChannel.invokeMethod<void>(
        'setSkipGitHubImportConfirmation',
        false,
      );
    } on MissingPluginException {
      /* Older host; keep the session preference. */
    } on PlatformException {
      /* Confirmation stays enabled for this session. */
    }
  }

  void selectIssue(String number) {
    final keys = HardwareKeyboard.instance;
    setState(() {
      importRequestId = null;
      if (keys.isShiftPressed && selectionAnchor != null) {
        final start = issues.indexWhere((v) => issueKey(v) == selectionAnchor);
        final end = issues.indexWhere((v) => issueKey(v) == number);
        if (start >= 0 && end >= 0) {
          for (
            var i = start < end ? start : end;
            i <= (start > end ? start : end);
            i++
          ) {
            if (!imported.containsKey(issueKey(issues[i]))) {
              selected.add(issueKey(issues[i]));
            }
          }
        }
      } else if (keys.isMetaPressed || keys.isControlPressed) {
        if (!selected.remove(number)) selected.add(number);
        selectionAnchor = number;
      } else {
        selected
          ..clear()
          ..add(number);
        selectionAnchor = number;
      }
    });
  }

  @override
  void dispose() {
    session.removeListener(connectionChanged);
    super.dispose();
  }

  void connectionChanged() {
    if (mounted && checkedConnection && session.status != null) {
      applyConnection(session.status!);
      restoreVisible();
    }
  }

  void applyConnection(Map<String, dynamic> result) {
    final next = result['connected'] == true;
    if (result['generation'] == connectionGeneration && next == connected) {
      return;
    }
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
      listLoaded = false;
      busy = false;
      error = null;
    });
  }

  Future<bool> checkConnection({bool force = false}) async {
    if (checkingConnection) return false;
    checkingConnection = true;
    try {
      if (force) session.statusRead = null;
      final result = !force && session.status != null
          ? session.status!
          : await (session.statusRead ??= githubRequest(
              widget.request,
              'Status',
            ));
      if (!mounted) return false;
      applyConnection(result);
      if (connected && (!linksLoaded || force)) await loadLinks(force: force);
      return true;
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
      return false;
    } finally {
      checkingConnection = false;
      if (mounted && !checkedConnection) {
        setState(() => checkedConnection = true);
      }
    }
  }

  Future<void> loadLinks({bool force = false}) async {
    if (loadingLinks) return;
    loadingLinks = true;
    final generation = requestGeneration;
    try {
      final result = await session.read(
        {
          'Links': {'project_id': null},
        },
        widget.request,
        force: force,
      );
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
      final previous =
          link ?? session.selections[scope]?['link'] as Map<String, dynamic>?;
      final available = links
          .where(
            (v) =>
                v['project_id'] == previous?['project_id'] &&
                v['repository']['id'] == previous?['repository']['id'],
          )
          .toList();
      if (available.isNotEmpty) {
        filter = session.selections[scope]?['filter'] as String? ?? filter;
        await chooseLink(available.first);
      } else if (links.isNotEmpty) {
        await chooseLink(links.length == 1 ? links.single : allRepositories);
      } else {
        setState(() {
          link = null;
          issues = [];
          imported = {};
          listLoaded = false;
        });
      }
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
      barrierDismissible: false,
      builder: (context) {
        privateDialogContext = context;
        return GitHubRepositoryPicker(
          request: widget.request,
          projects: widget.projects,
        );
      },
    );
    if (linked == true && mounted) await loadLinks(force: true);
  }

  Future<void> chooseLink(Map<String, dynamic> next) async {
    setState(() {
      link = next;
      issues = [];
      page = 0;
      selected.clear();
      selectionAnchor = null;
      imported = {};
      lastSync = null;
      hasMore = false;
      listLoaded = false;
      importRequestId = null;
      error = null;
    });
    session.selections[scope] = {'link': next, 'filter': filter};
    restoreVisible();
    if (!listLoaded) await fetch();
  }

  Future<void> openConnection() async {
    await showDialog<void>(
      context: context,
      builder: (_) => GitHubSettingsDialog(request: widget.request),
    );
    if (mounted) await checkConnection();
  }

  Future<void> fetch({bool more = false, bool force = false}) async {
    if (link == null || busy) return;
    Completer<void>? initial;
    if (!force && !more) {
      if (session.initialIssuesClaimed) {
        await session.initialRead;
        if (mounted) restoreVisible();
        return;
      }
      session.initialIssuesClaimed = true;
      initial = Completer<void>();
      session.initialRead = initial.future;
    }
    final epoch = session.epoch;
    final generation = requestGeneration;
    final sources = List<Map<String, dynamic>>.from(visibleLinks);
    final requestedFilter = filter;
    setState(() {
      busy = true;
      error = null;
      readProgress = null;
    });
    final failures = <String>[];
    for (final source in sources) {
      if (generation != requestGeneration || epoch != session.epoch) {
        break;
      }
      final key = "${sourceKey(source)}:$requestedFilter";
      final prior = session.snapshots[key];
      if (more && prior?['has_more'] != true) continue;
      final nextPage = more ? (prior?['page'] as int? ?? 0) + 1 : 1;
      try {
        final result = await session.read(
          {
            'Issues': {
              'project_id': source['project_id'],
              'repository_id': source['repository']['id'],
              'state': requestedFilter,
              'page': nextPage,
            },
          },
          widget.request,
          force: force,
        );
        if (epoch != session.epoch || generation != requestGeneration) break;
        final byId = <dynamic, dynamic>{
          for (final row in [
            ...(more ? (prior?['issues'] as List? ?? []) : []),
            ...result['issues'] as List,
          ])
            row['id']: row,
        };
        session.snapshots[key] = {
          ...result,
          'issues': byId.values
              .map((v) => Map<String, dynamic>.from(v as Map))
              .toList(),
          'page': nextPage,
          'imported': {
            ...(more ? (prior?['imported'] as Map? ?? {}) : {}),
            ...result['imported'] as Map? ?? {},
          },
        };
      } on Object catch (e) {
        failures.add("${source['repository']['full_name']}: $e");
      }
    }
    initial?.complete();
    if (mounted && generation == requestGeneration) {
      restoreVisible();
      setState(() {
        busy = false;
        if (!more) {
          selected.clear();
          selectionAnchor = null;
        }
        if (failures.isNotEmpty) {
          error =
              'Some repositories could not refresh. Their displayed issues may be stale.\n${failures.join('\n')}';
        }
      });
    }
  }

  Future<void> importIssues(
    Map<String, dynamic> current,
    List<Map<String, dynamic>> rows,
    int generation,
  ) async {
    if (!mounted ||
        generation != requestGeneration ||
        busy ||
        confirmingImport ||
        !connected ||
        link != current ||
        rows.isEmpty ||
        rows.length > 50) {
      return;
    }
    confirmingImport = true;
    var skipNextTime = false;
    var confirm = skipImportConfirmation;
    try {
      if (!confirm) {
        confirm =
            await showDialog<bool>(
              context: context,
              builder: (dialogContext) {
                privateDialogContext = dialogContext;
                return StatefulBuilder(
                  builder: (context, update) => AlertDialog(
                    constraints: const BoxConstraints(maxWidth: 400),
                    title: Text(
                      'Add ${rows.length == 1 ? 'issue' : '${rows.length} issues'} to Backlog?',
                    ),
                    content: SizedBox(
                      width: 320,
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          const Text('Adds tasks without starting agents.'),
                          const SizedBox(height: 12),
                          CheckboxListTile(
                            contentPadding: EdgeInsets.zero,
                            dense: true,
                            controlAffinity: ListTileControlAffinity.leading,
                            title: const Text('Don’t show this again'),
                            value: skipNextTime,
                            onChanged: (v) =>
                                update(() => skipNextTime = v == true),
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
                        child: const Text('Add to Backlog'),
                      ),
                    ],
                  ),
                );
              },
            ) ==
            true;
      }
    } finally {
      confirmingImport = false;
      privateDialogContext = null;
    }
    if (!confirm ||
        !mounted ||
        generation != requestGeneration ||
        !connected ||
        link != current) {
      return;
    }
    setState(() => busy = true);
    if (skipNextTime) {
      skipImportConfirmation = true;
      try {
        await preferencesChannel.invokeMethod<void>(
          'setSkipGitHubImportConfirmation',
          true,
        );
      } on MissingPluginException {
        /* Older hosts retain this choice for this panel session. */
      } on PlatformException {
        /* A preference failure must not cancel the accepted import. */
      }
    }
    if (!mounted ||
        generation != requestGeneration ||
        !connected ||
        link != current) {
      return;
    }
    setState(() {
      busy = true;
      readProgress = 'Preparing import…';
      error = null;
    });
    try {
      final groups = <String, List<Map<String, dynamic>>>{};
      for (final row in rows) {
        if (!imported.containsKey(issueKey(row))) {
          groups.putIfAbsent(sourceKey(row['_link'] as Map), () => []).add(row);
        }
      }
      final failures = <String>[];
      for (final group in groups.values) {
        if (!mounted || generation != requestGeneration || !connected) return;
        final source = group.first['_link'] as Map;
        final numbers = group.map((v) => v['number'] as int).toList()..sort();
        final identity = '${sourceKey(source)}:${numbers.join(',')}';
        final requestId = importRequests.putIfAbsent(
          identity,
          TaskBoardController.newRequestId,
        );
        try {
          await githubRequest(widget.request, {
            'Import': {
              'project_id': source['project_id'],
              'repository_id': source['repository']['id'],
              'numbers': numbers,
              'request_id': requestId,
            },
          });
          if (!mounted || generation != requestGeneration) return;
          setState(() {
            for (final row in group) {
              selected.remove(issueKey(row));
            }
          });
          importRequests.remove(identity);
        } on Object catch (e) {
          failures.add("${source['repository']['full_name']}: $e");
        }
      }
      await widget.onImported();
      if (mounted && generation == requestGeneration) {
        restoreVisible();
        setState(() {
          error = failures.isEmpty ? null : failures.join('\n');
          busy = false;
        });
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

  Future<void> refresh() async {
    if (busy || refreshing || draggingIssues || confirmingImport) return;
    setState(() => refreshing = true);
    try {
      if (!await checkConnection(force: true)) return;
      if (!mounted || !connected || !linksLoaded) return;
      final generation = requestGeneration;
      var failures = 0;
      if (showGitHubBoardControls && links.isNotEmpty) {
        setState(() {
          busy = true;
          readProgress = 'Refreshing GitHub source information…';
        });
        String? after;
        do {
          final result = await githubRequest(widget.request, {
            'SyncChanges': {'cursor': 0, 'after_task': after},
          });
          if (!mounted || generation != requestGeneration) return;
          failures += result['refresh_failures'] as int? ?? 0;
          after = result['after_task'] as String?;
        } while (after != null);
        await widget.onImported();
        if (!mounted || generation != requestGeneration) return;
        setState(() {
          busy = false;
          readProgress = null;
        });
      }
      // A refresh replaces this query's old pages as one new first page.
      if (link != null) {
        session.reads.removeWhere((key, _) => key.contains('"Issues"'));
        await fetch(force: true);
      }
      if (failures > 0 && mounted) {
        setState(
          () => error =
              'Some linked sources could not refresh. Check repository access and relink if needed.',
        );
      }
    } on Object catch (e) {
      if (mounted) setState(() => error = '$e');
    } finally {
      if (mounted) {
        setState(() {
          busy = false;
          refreshing = false;
          readProgress = null;
        });
      }
    }
  }

  Future<void> createIssue() async {
    final current = link;
    if (current == null || busy || !connected) return;
    final created = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (context) {
        privateDialogContext = context;
        return GitHubCreateIssueDialog(
          request: widget.request,
          link: current['all'] == true ? null : current,
          repositoryLinks: links,
        );
      },
    );
    privateDialogContext = null;
    if (created == true && mounted) {
      await widget.onImported();
      // Creating is an explicit write; list reads still wait for Refresh.
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Issue created. Click Refresh to update the list.'),
          ),
        );
      }
    }
  }

  Widget issueCard(Map<String, dynamic> issue) {
    final number = issue['number'] as int;
    final identity = issueKey(issue);
    final alreadyImported = imported.containsKey(identity);
    final current = link!;
    final generation = requestGeneration;
    final numbers = List<Map<String, dynamic>>.unmodifiable(
      selected.contains(identity)
          ? issues.where((v) => selected.contains(issueKey(v)))
          : [issue],
    );
    final drag = GitHubIssueDrag(
      count: numbers.length,
      canDrop: () =>
          mounted &&
          connected &&
          !busy &&
          !confirmingImport &&
          generation == requestGeneration &&
          link == current &&
          numbers.length <= 50,
      importToBacklog: () => importIssues(current, numbers, generation),
    );
    Widget card({bool ghost = false}) => AnimatedContainer(
      duration: const Duration(milliseconds: 140),
      margin: const EdgeInsets.only(bottom: 8),
      decoration: BoxDecoration(
        color: selected.contains(identity)
            ? Theme.of(context).colorScheme.primary.withValues(alpha: .08)
            : Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(
          color: selected.contains(identity)
              ? Theme.of(context).colorScheme.primary
              : Theme.of(context).dividerColor,
        ),
      ),
      child: InkWell(
        borderRadius: BorderRadius.circular(10),
        onTap: busy || alreadyImported || ghost
            ? null
            : () => selectIssue(identity),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 4, 10),
          child: Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      '${issue['title']}',
                      maxLines: 3,
                      overflow: TextOverflow.ellipsis,
                    ),
                    const SizedBox(height: 4),
                    Text(
                      '${(issue['_link'] as Map)['repository']['full_name']} #$number · ${issue['state']}${issue['type']?['name'] == null ? '' : ' · ${issue['type']['name']}'}${alreadyImported ? ' · In board' : ''}',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
              IconButton(
                tooltip: 'Issue info',
                icon: const Icon(Icons.info_outline, size: 18),
                onPressed: ghost ? null : () => showIssueDetails(issue),
              ),
              if (alreadyImported && widget.onOpenTask != null)
                IconButton(
                  tooltip: 'Open imported task',
                  icon: const Icon(Icons.open_in_new, size: 16),
                  onPressed: ghost
                      ? null
                      : () => widget.onOpenTask!(imported[identity] as String),
                ),
            ],
          ),
        ),
      ),
    );
    return Draggable<GitHubIssueDrag>(
      key: ValueKey(
        links.length == 1 ? 'github-issue-$number' : 'github-issue-$identity',
      ),
      data: drag,
      maxSimultaneousDrags:
          busy || confirmingImport || alreadyImported || numbers.length > 50
          ? 0
          : 1,
      onDragStarted: () {
        setState(() {
          draggingIssues = true;
          if (!selected.contains(identity)) {
            selected
              ..clear()
              ..add(identity);
            selectionAnchor = identity;
            importRequestId = null;
          }
        });
      },
      onDragEnd: (_) {
        if (mounted) setState(() => draggingIssues = false);
      },
      feedback: Transform.rotate(
        angle: -.025,
        child: Material(
          elevation: 12,
          borderRadius: BorderRadius.circular(10),
          child: SizedBox(
            width: 280,
            child: Padding(
              padding: const EdgeInsets.all(14),
              child: Text(
                numbers.length == 1
                    ? '${issue['title']}'
                    : '${numbers.length} GitHub issues',
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
              ),
            ),
          ),
        ),
      ),
      childWhenDragging: Opacity(opacity: .3, child: card(ghost: true)),
      child: MouseRegion(
        cursor: alreadyImported || busy
            ? SystemMouseCursors.basic
            : SystemMouseCursors.grab,
        child: card(),
      ),
    );
  }

  void showIssueDetails(Map<String, dynamic> issue) {
    showDialog<void>(
      context: context,
      builder: (context) {
        privateDialogContext = context;
        return GitHubIssueDetail(
          request: widget.request,
          issue: issue,
          link: issue['_link'] as Map<String, dynamic>,
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) => Container(
    decoration: BoxDecoration(
      border: Border(right: BorderSide(color: Theme.of(context).dividerColor)),
    ),
    padding: const EdgeInsets.all(16),
    child: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                'GitHub',
                style: Theme.of(context).textTheme.titleLarge,
              ),
            ),
            if (checkedConnection && link == null)
              IconButton(
                tooltip: 'Refresh GitHub issues',
                onPressed: busy || refreshing ? null : refresh,
                icon: const Icon(Icons.refresh, size: 18),
              ),
            if (checkedConnection)
              PopupMenuButton<String>(
                tooltip: 'GitHub options',
                icon: const Icon(Icons.more_horiz),
                onSelected: (value) {
                  if (value == 'link') unawaited(linkRepository());
                  if (value == 'connection') unawaited(openConnection());
                  if (value == 'create') unawaited(createIssue());
                  if (value == 'confirm') {
                    unawaited(restoreImportConfirmation());
                  }
                },
                itemBuilder: (_) => [
                  if (connected && links.isNotEmpty)
                    PopupMenuItem(
                      value: 'link',
                      enabled: !busy,
                      child: const Text('Link repository…'),
                    ),
                  if (showGitHubBoardControls && link != null)
                    PopupMenuItem(
                      value: 'create',
                      enabled: !busy,
                      child: const Text('Create issue…'),
                    ),
                  const PopupMenuItem(
                    value: 'connection',
                    child: Text('GitHub account…'),
                  ),
                  if (skipImportConfirmation)
                    const PopupMenuItem(
                      value: 'confirm',
                      child: Text('Show import confirmation'),
                    ),
                ],
              ),
          ],
        ),
        if (connected && links.isNotEmpty) ...[
          const SizedBox(height: 8),
          if (links.length == 1)
            Tooltip(
              message: '${links.single['repository']['full_name']}',
              child: Text(
                '${links.single['repository']['full_name']}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            )
          else
            DropdownButtonHideUnderline(
              child: DropdownButton<String>(
                isExpanded: true,
                value: link?['all'] == true
                    ? 'all'
                    : link == null
                    ? null
                    : '${link!['project_id']}:${link!['repository']['id']}',
                hint: const Text('Choose repository'),
                items: [
                  const DropdownMenuItem(
                    value: 'all',
                    child: Text('All linked repositories'),
                  ),
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
                    : (id) => chooseLink(
                        id == 'all'
                            ? allRepositories
                            : links.firstWhere(
                                (v) =>
                                    '${v['project_id']}:${v['repository']['id']}' ==
                                    id,
                              ),
                      ),
              ),
            ),
        ],
        if (link != null)
          Row(
            children: [
              DropdownButtonHideUnderline(
                child: DropdownButton<String>(
                  value: filter,
                  items: [
                    for (final v in ['Open', 'Closed', 'All'])
                      DropdownMenuItem(value: v, child: Text(v)),
                  ],
                  onChanged: busy
                      ? null
                      : (v) {
                          filter = v!;
                          chooseLink(link!);
                        },
                ),
              ),
              const Spacer(),
              IconButton(
                tooltip: lastSync == null
                    ? 'Refresh GitHub issues'
                    : 'Refresh GitHub issues • $lastSync',
                onPressed: busy || refreshing ? null : refresh,
                icon: const Icon(Icons.refresh, size: 18),
              ),
            ],
          ),
        if (busy)
          Row(
            children: [
              const Expanded(child: LinearProgressIndicator()),
              IconButton(
                tooltip: 'Cancel loading',
                onPressed: cancelLoading,
                icon: const Icon(Icons.close, size: 16),
              ),
            ],
          ),
        if (error != null)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 90),
              child: SingleChildScrollView(
                child: SelectableText(
                  error!,
                  style: TextStyle(
                    color: Theme.of(context).colorScheme.error,
                    fontSize: 12,
                  ),
                ),
              ),
            ),
          ),
        Expanded(
          child:
              !checkedConnection || (connected && !linksLoaded && loadingLinks)
              ? const Center(child: CircularProgressIndicator())
              : !connected
              ? Center(
                  child: FilledButton(
                    onPressed: openConnection,
                    child: const Text('Connect GitHub'),
                  ),
                )
              : !linksLoaded
              ? Center(
                  child: TextButton(
                    onPressed: loadingLinks
                        ? null
                        : () => loadLinks(force: true),
                    child: const Text('Retry repositories'),
                  ),
                )
              : links.isEmpty
              ? Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(
                        Icons.link,
                        size: 28,
                        color: Theme.of(context).colorScheme.onSurfaceVariant,
                      ),
                      const SizedBox(height: 12),
                      const Text(
                        'Bring GitHub issues into your board.',
                        textAlign: TextAlign.center,
                      ),
                      const SizedBox(height: 16),
                      FilledButton(
                        onPressed: busy ? null : linkRepository,
                        child: const Text('Link repository'),
                      ),
                    ],
                  ),
                )
              : link == null
              ? const Center(child: Text('Choose a repository above.'))
              : issues.isEmpty
              ? Center(
                  child: Text(
                    busy
                        ? 'Loading issues…'
                        : !listLoaded
                        ? 'Click Refresh to load issues.'
                        : filter == 'All'
                        ? 'No issues'
                        : 'No ${filter.toLowerCase()} issues',
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.onSurfaceVariant,
                    ),
                  ),
                )
              : ListView(
                  children: [for (final issue in issues) issueCard(issue)],
                ),
        ),
        if (hasMore)
          TextButton(
            onPressed: busy ? null : () => fetch(more: true),
            child: const Text('Load more'),
          ),
        if (issues.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 8),
            child: Text(
              selected.length > 50
                  ? 'Select up to 50 issues.'
                  : selected.isEmpty
                  ? 'Drag to Backlog · Shift-click to select'
                  : '${selected.length} selected · Drag to Backlog',
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ),
      ],
    ),
  );
}
