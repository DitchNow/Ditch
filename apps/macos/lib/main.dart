import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'application/command_center_controller.dart';
import 'data/runtime_models.dart';
import 'data/runtime_transport.dart';
import 'design_system/ditch_theme.dart';

export 'data/runtime_transport.dart'
    show DitchRuntimeException, parseRuntimeResponseLine;

void main() {
  runApp(const TheDitchApp());
}

class TheDitchApp extends StatelessWidget {
  const TheDitchApp({this.connectRuntimeOnStart = true, super.key});

  final bool connectRuntimeOnStart;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'The Ditch',
      debugShowCheckedModeBanner: false,
      theme: DitchTheme.light(),
      darkTheme: DitchTheme.dark(),
      themeMode: ThemeMode.system,
      home: CommandCenterScreen(connectRuntimeOnStart: connectRuntimeOnStart),
    );
  }
}

class DitchProject {
  const DitchProject({
    this.id,
    required this.name,
    required this.path,
    this.gitPolicy = ProjectGitPolicy.requireRepository,
  });

  final String? id;
  final String name;
  final String path;
  final ProjectGitPolicy gitPolicy;
}

enum ProjectGitPolicy {
  requireRepository,
  initializeRepository,
  allowOutsideGit,
}

bool isInsideGitWorkTree(String path) {
  var directory = Directory(path).absolute;
  while (true) {
    if (FileSystemEntity.typeSync('${directory.path}/.git') !=
        FileSystemEntityType.notFound) {
      return true;
    }
    final parent = directory.parent;
    if (parent.path == directory.path) {
      return false;
    }
    directory = parent;
  }
}

String canonicalProjectPath(String path) {
  final directory = Directory(path).absolute;
  try {
    return directory.resolveSymbolicLinksSync();
  } on FileSystemException {
    return directory.path;
  }
}

void upsertProject(List<DitchProject> projects, DitchProject incoming) {
  final incomingPath = canonicalProjectPath(incoming.path);
  final index = projects.indexWhere(
    (existing) =>
        (incoming.id != null && existing.id == incoming.id) ||
        canonicalProjectPath(existing.path) == incomingPath,
  );
  if (index >= 0) {
    projects[index] = incoming;
  } else {
    projects.add(incoming);
  }
}

enum AgentProvider { codex }

class AgentSession {
  AgentSession({
    required this.localId,
    required this.provider,
    required this.status,
    required this.messages,
    this.projectId,
    this.codexThreadId,
    this.codexTitle,
    this.userTitle,
    this.originCodexHome,
    this.currentPrompt,
    this.lastVisibleAction,
    this.resumeBlockReason,
    this.exitCode,
    this.finishedAt,
    DateTime? createdAt,
    DateTime? updatedAt,
  }) : createdAt = createdAt ?? DateTime.now(),
       updatedAt = updatedAt ?? DateTime.now();

  final String localId;
  final String? projectId;
  final AgentProvider provider;
  final DateTime createdAt;
  AgentStatus status;
  String? codexThreadId;
  String? codexTitle;
  String? userTitle;
  final String? originCodexHome;
  String? currentPrompt;
  String? lastVisibleAction;
  final String? resumeBlockReason;
  final int? exitCode;
  final DateTime? finishedAt;
  DateTime updatedAt;
  final List<AgentChatMessage> messages;

  String get displayName {
    final override = userTitle?.trim();
    if (override != null && override.isNotEmpty) return override;
    final native = codexTitle?.trim();
    if (native != null && native.isNotEmpty) return native;
    return 'Codex';
  }

  bool get hasCodexThread => codexThreadId != null;

  bool get isTerminal =>
      status == AgentStatus.failed || status == AgentStatus.stopped;

  bool get cannotResumeWithoutThread => isTerminal && !hasCodexThread;

  bool get isWorking {
    return status == AgentStatus.starting || status == AgentStatus.working;
  }
}

void reconcileAgentSession(List<AgentSession> sessions, AgentSession incoming) {
  final matches = sessions
      .where((session) => session.localId == incoming.localId)
      .toList();
  if (matches.isEmpty) {
    sessions.insert(0, incoming);
    return;
  }

  final existing = matches.first;
  existing.status = incoming.status;
  existing.codexThreadId = incoming.codexThreadId;
  existing.codexTitle = incoming.codexTitle;
  existing.userTitle = incoming.userTitle;
  existing.currentPrompt = incoming.currentPrompt;
  existing.lastVisibleAction = incoming.lastVisibleAction;
  existing.updatedAt = incoming.updatedAt;
  sessions.removeWhere(
    (session) =>
        session.localId == incoming.localId && !identical(session, existing),
  );
}

enum AttentionKind { approvalRequired, blocked, failed, needsInput }

class AttentionEvent {
  const AttentionEvent({
    required this.id,
    required this.kind,
    required this.icon,
    required this.title,
    required this.body,
    required this.createdAt,
    this.sessionLocalId,
    this.projectId,
  });

  final String id;
  final AttentionKind kind;
  final IconData icon;
  final String title;
  final String body;
  final DateTime createdAt;
  final String? sessionLocalId;
  final String? projectId;

  bool get canOpenSession => sessionLocalId != null;
}

enum AgentStatus { idle, starting, working, completed, failed, stopped }

enum ChatMessageRole { user, assistant, system, tool }

enum CodexProcessEventKind { diagnostic }

class CodexProcessDiagnostic {
  const CodexProcessDiagnostic({
    required this.kind,
    required this.text,
    required this.createdAt,
  });

  final CodexProcessEventKind kind;
  final String text;
  final DateTime createdAt;

  bool get isVisibleInChat => true;
}

CodexProcessDiagnostic? codexStderrDiagnosticFromChunk(String chunk) {
  final text = chunk.trim();
  if (text.isEmpty) {
    return null;
  }

  return CodexProcessDiagnostic(
    kind: CodexProcessEventKind.diagnostic,
    text: text,
    createdAt: DateTime.now(),
  );
}

class AgentChatMessage {
  const AgentChatMessage({
    required this.role,
    required this.text,
    required this.createdAt,
  });

  final ChatMessageRole role;
  final String text;
  final DateTime createdAt;
}

List<AgentSession> sessionsForProject(
  Iterable<AgentSession> sessions,
  String? projectId,
) {
  return sessions.where((session) => session.projectId == projectId).toList();
}

List<AttentionEvent> attentionForProject(
  Iterable<AttentionEvent> events,
  String? projectId,
) {
  return events
      .where((event) => event.projectId == null || event.projectId == projectId)
      .toList();
}

class DitchRuntimeClient {
  DitchRuntimeClient({String? socketPath})
    : _transport = RuntimeTransport(socketPath: socketPath);

  final RuntimeTransport _transport;

  String get socketPath => _transport.socketPath;

  Future<Map<String, dynamic>> request(Object body) => _transport.request(body);

  Future<Stream<Map<String, dynamic>>> subscribeEvents() =>
      _transport.subscribeEvents();

  Future<RuntimeStatusDto> runtimeStatus() async {
    return RuntimeStatusDto.fromResponse(await request('RuntimeStatus'));
  }

  Future<Map<String, dynamic>> shutdownRuntime() {
    return request('Shutdown');
  }

  Future<Map<String, dynamic>> snapshot() {
    return request('Snapshot');
  }

  Future<Map<String, dynamic>> createProject({
    required String name,
    required String root,
    required ProjectGitPolicy gitPolicy,
  }) {
    return request({
      'CreateProject': {
        'name': name,
        'root': root,
        'git_policy': switch (gitPolicy) {
          ProjectGitPolicy.requireRepository => 'RequireRepository',
          ProjectGitPolicy.initializeRepository => 'InitializeRepository',
          ProjectGitPolicy.allowOutsideGit => 'AllowOutsideGit',
        },
      },
    });
  }

  Future<Map<String, dynamic>> discoverProjects(String searchRoot) {
    return request({
      'DiscoverProjects': {'search_root': searchRoot},
    });
  }

  Future<Map<String, dynamic>> startCodexSession({
    required String projectName,
    required String projectRoot,
    required String prompt,
  }) {
    return request({
      'StartCodexSession': {
        'project_name': projectName,
        'project_root': projectRoot,
        'prompt': prompt,
        'mode': 'Exec',
      },
    });
  }

  Future<Map<String, dynamic>> resumeCodexSession({
    required String projectName,
    required String projectRoot,
    required String threadId,
    required String prompt,
  }) {
    return request({
      'ResumeCodexSession': {
        'project_name': projectName,
        'project_root': projectRoot,
        'thread_id': threadId,
        'prompt': prompt,
      },
    });
  }

  Future<Map<String, dynamic>> promptAgent({
    required String agentId,
    required String prompt,
  }) {
    return request({
      'PromptAgent': {'agent_id': agentId, 'prompt': prompt},
    });
  }

  Future<Map<String, dynamic>> stopAgent(String agentId) {
    return request({
      'StopAgent': {'agent_id': agentId},
    });
  }

  Future<Map<String, dynamic>> deleteAgent(String agentId) {
    return request({
      'DeleteAgent': {'agent_id': agentId},
    });
  }

  Future<Map<String, dynamic>> renameAgent(String agentId, String? title) {
    return request({
      'RenameAgent': {'agent_id': agentId, 'title': title},
    });
  }

  Future<Map<String, dynamic>> dismissAttention(String attentionId) {
    return request({
      'DismissAttention': {'attention_id': attentionId},
    });
  }
}

DitchProject? parseRuntimeProject(Object? value) {
  if (value is! Map<String, dynamic>) {
    return null;
  }
  final name = value['name']?.toString();
  final root = value['root']?.toString();
  if (name == null || name.isEmpty || root == null || root.isEmpty) {
    return null;
  }
  final gitPolicy = switch (value['git_policy']?.toString()) {
    'AllowOutsideGit' => ProjectGitPolicy.allowOutsideGit,
    'InitializeRepository' => ProjectGitPolicy.initializeRepository,
    _ => ProjectGitPolicy.requireRepository,
  };
  return DitchProject(
    id: value['id']?.toString(),
    name: name,
    path: root,
    gitPolicy: gitPolicy,
  );
}

class CommandCenterScreen extends StatefulWidget {
  const CommandCenterScreen({this.connectRuntimeOnStart = true, super.key});

  final bool connectRuntimeOnStart;

  @override
  State<CommandCenterScreen> createState() => _CommandCenterScreenState();
}

class _CommandCenterScreenState extends State<CommandCenterScreen> {
  static const _defaultStartPrompt =
      'Inspect this project and tell me the next useful engineering step.';
  static const _bootstrapProject = DitchProject(
    name: 'The Ditch',
    path: '/Users/tester/Documents/Personal/The Ditch v2',
  );
  static const _applicationChannel = MethodChannel('the_ditch/application');

  final _chatController = ScrollController();
  final _agentListController = ScrollController();
  final _composerKey = GlobalKey<AgentComposerState>();
  final _runtimeClient = DitchRuntimeClient();
  final _presentation = CommandCenterController();
  int _nextAgentSessionId = 1;
  int _nextAttentionId = 1;
  final _projects = <DitchProject>[_bootstrapProject];
  final _attention = <AttentionEvent>[];

  StreamSubscription<Map<String, dynamic>>? _runtimeEvents;
  String? _runtimeInstanceId;
  bool _runtimeReconnectScheduled = false;
  bool _runtimeHomeMismatchReported = false;
  bool _runtimeCodexHomeMatches = true;
  bool _runtimeSupportsPersistence = true;
  bool _runtimeCompatibilityReported = false;
  String? _effectiveRuntimeCodexHome;
  bool _legacyRecoveryChecked = false;
  String _selectedProjectKey = canonicalProjectPath(_bootstrapProject.path);
  String? _expandedAgentLocalId;
  String? _focusedAgentLocalId;
  final List<AgentSession> _agentSessions = [];

  String _projectKey(DitchProject project) =>
      project.id ?? canonicalProjectPath(project.path);

  int get _selectedProjectIndex {
    final index = _projects.indexWhere(
      (project) => _projectKey(project) == _selectedProjectKey,
    );
    return index < 0 ? 0 : index;
  }

  DitchProject get _selectedProject => _projects[_selectedProjectIndex];

  List<AgentSession> get _visibleSessions {
    return sessionsForProject(_agentSessions, _selectedProject.id);
  }

  List<AttentionEvent> get _visibleAttention {
    return attentionForProject(_attention, _selectedProject.id);
  }

  void _selectProject(int index) {
    setState(() {
      _selectedProjectKey = _projectKey(_projects[index]);
      final sessions = _visibleSessions;
      _expandedAgentLocalId = sessions.isEmpty ? null : sessions.first.localId;
    });
  }

  void _scheduleStatusBarUpdate() {
    // The integrated ditchd process owns both authoritative runtime state and
    // the menu-bar item. The foreground UI never pushes lifecycle state.
  }

  AgentSession? _agentSessionByLocalId(String localId) {
    for (final session in _agentSessions) {
      if (session.localId == localId) {
        return session;
      }
    }
    return null;
  }

  AgentSession _createAgentSession({required bool expand}) {
    final session = AgentSession(
      localId: 'agent-${_nextAgentSessionId++}',
      projectId: _selectedProject.id,
      provider: AgentProvider.codex,
      status: AgentStatus.idle,
      messages: [
        AgentChatMessage(
          role: ChatMessageRole.system,
          text:
              'Ready. Start Codex to attach an agent to the selected project.',
          createdAt: DateTime.now(),
        ),
      ],
    );
    _agentSessions.insert(0, session);
    if (expand) {
      _expandedAgentLocalId = session.localId;
    }
    return session;
  }

  Future<void> _connectRuntime() async {
    _presentation.connecting(reconnecting: _runtimeInstanceId != null);
    try {
      await _ensureRuntimeStarted();
      final status = await _runtimeClient.runtimeStatus();
      final instanceId = status.instanceId;
      var snapshot = await _runtimeClient.snapshot();
      final snapshotBody = snapshot['Snapshot'];
      final registeredProjects = snapshotBody is Map<String, dynamic>
          ? snapshotBody['projects']
          : null;
      if (registeredProjects is List && registeredProjects.isEmpty) {
        final bootstrap = _projects.first;
        await _runtimeClient.createProject(
          name: bootstrap.name,
          root: bootstrap.path,
          gitPolicy: bootstrap.gitPolicy,
        );
        snapshot = await _runtimeClient.snapshot();
      }
      _hydrateRuntimeSnapshot(snapshot);
      _checkRuntimeCapabilities(status);
      _checkRuntimeCodexHome(status);
      final events = await _runtimeClient.subscribeEvents();
      await _runtimeEvents?.cancel();
      _runtimeInstanceId = instanceId;
      _runtimeEvents = events.listen(
        _handleRuntimeEvent,
        onError: (Object error) {
          _addAttentionRequired(
            kind: AttentionKind.failed,
            icon: Icons.error_outline,
            title: 'Runtime event stream failed',
            body: '$error',
          );
          _scheduleRuntimeReconnect();
        },
        onDone: _scheduleRuntimeReconnect,
      );
      final confirmed = await _runtimeClient.runtimeStatus();
      final confirmedInstanceId = confirmed.instanceId;
      if (_runtimeInstanceId != confirmedInstanceId) {
        await _runtimeEvents?.cancel();
        _scheduleRuntimeReconnect();
      } else if (!_legacyRecoveryChecked) {
        _legacyRecoveryChecked = true;
        unawaited(_offerLegacyProjectRecovery());
      }
      _presentation.connected();
    } on Object catch (error) {
      _presentation.unavailable(error);
      _addAttentionRequired(
        kind: AttentionKind.failed,
        icon: Icons.cloud_off_outlined,
        title: 'Runtime not connected',
        body: '$error',
        global: true,
      );
    }
  }

  void _checkRuntimeCodexHome(RuntimeStatusDto status) {
    final requested = Platform.environment['CODEX_HOME'];
    final effective = status.codexHome;
    _effectiveRuntimeCodexHome = effective;
    if (requested == null || requested.isEmpty || effective == requested) {
      _runtimeHomeMismatchReported = false;
      _runtimeCodexHomeMatches = true;
      return;
    }
    _runtimeCodexHomeMatches = false;
    if (_runtimeHomeMismatchReported) {
      return;
    }
    _runtimeHomeMismatchReported = true;
    _addAttentionRequired(
      kind: AttentionKind.failed,
      icon: Icons.sync_problem_outlined,
      title: 'Codex home mismatch',
      body:
          'The running Ditch Runtime uses ${effective ?? "an unknown/default Codex home"}, but this app was launched with $requested. Stop and restart the runtime before starting Codex.',
      global: true,
    );
  }

  void _checkRuntimeCapabilities(RuntimeStatusDto status) {
    _runtimeSupportsPersistence = status.supportsPersistentSessions;
    if (_runtimeSupportsPersistence) {
      _runtimeCompatibilityReported = false;
      return;
    }
    if (_runtimeCompatibilityReported) {
      return;
    }
    _runtimeCompatibilityReported = true;
    _addAttentionRequired(
      kind: AttentionKind.failed,
      icon: Icons.system_update_alt_outlined,
      title: 'Outdated Ditch Runtime',
      body:
          'This runtime cannot persist agent sessions. Rebuild and restart The Ditch before starting Codex.',
      global: true,
    );
  }

  Future<void> _offerLegacyProjectRecovery() async {
    if (_projects.isEmpty) {
      return;
    }
    final searchRoot = Directory(_bootstrapProject.path).parent.path;
    try {
      final response = await _runtimeClient.discoverProjects(searchRoot);
      final raw = response['Projects'];
      if (raw is! List || !mounted) {
        return;
      }
      final existingPaths = _projects.map((project) => project.path).toSet();
      final discovered = raw
          .map(_projectFromRuntime)
          .whereType<DitchProject>()
          .where((project) => !existingPaths.contains(project.path))
          .toList();
      if (discovered.isEmpty) {
        return;
      }
      final shouldRecover = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Recover existing projects?'),
          content: SizedBox(
            width: 560,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'The Ditch found project folders created by an earlier version:',
                ),
                const SizedBox(height: 12),
                for (final project in discovered)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 8),
                    child: Text('• ${project.name}\n  ${project.path}'),
                  ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Not Now'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('Recover Projects'),
            ),
          ],
        ),
      );
      if (shouldRecover != true) {
        return;
      }
      for (final project in discovered) {
        await _runtimeClient.createProject(
          name: project.name,
          root: project.path,
          gitPolicy: project.gitPolicy,
        );
      }
      _hydrateRuntimeSnapshot(await _runtimeClient.snapshot());
    } on Object catch (error) {
      _addAttentionRequired(
        kind: AttentionKind.failed,
        icon: Icons.error_outline,
        title: 'Project recovery failed',
        body: '$error',
      );
    }
  }

  void _scheduleRuntimeReconnect() {
    if (_runtimeReconnectScheduled || !mounted) {
      return;
    }
    _runtimeReconnectScheduled = true;
    Future<void>.delayed(const Duration(milliseconds: 500), () async {
      if (!mounted) {
        return;
      }
      _runtimeReconnectScheduled = false;
      await _connectRuntime();
    });
  }

  Future<void> _ensureRuntimeStarted() async {
    try {
      final ready = await _applicationChannel.invokeMethod<bool>(
        'runtimeAvailable',
      );
      if (ready == false) {
        throw StateError('The supervised runtime is not available.');
      }
      await _runtimeClient.runtimeStatus();
      return;
    } on Object catch (error) {
      throw StateError(
        'The Ditch Runtime is unavailable. Use the status menu to restart it, then retry. $error',
      );
    }
  }

  void _hydrateRuntimeSnapshot(Map<String, dynamic> responseBody) {
    final snapshot = responseBody['Snapshot'];
    if (snapshot is! Map<String, dynamic>) {
      return;
    }
    final agentsJson = snapshot['agents'];
    final messagesJson = snapshot['messages'];
    if (agentsJson is! List) {
      return;
    }

    final messagesByAgent = <String, List<AgentChatMessage>>{};
    if (messagesJson is List) {
      for (final messageJson in messagesJson) {
        final message = _agentChatMessageFromRuntime(messageJson);
        final agentId = _agentIdFromRuntimeMessage(messageJson);
        if (message != null && agentId != null) {
          messagesByAgent.putIfAbsent(agentId, () => []).add(message);
        }
      }
    }

    final sessionsById = <String, AgentSession>{};
    for (final agentJson in agentsJson) {
      final session = _agentSessionFromRuntime(
        agentJson,
        messagesByAgent[_agentIdFromRuntimeAgent(agentJson)] ?? const [],
      );
      if (session != null) {
        sessionsById[session.localId] = session;
      }
    }
    final sessions = sessionsById.values.toList();
    sessions.sort((a, b) => b.updatedAt.compareTo(a.updatedAt));
    final attentionJson = snapshot['attention'];
    final attention = <AttentionEvent>[];
    if (attentionJson is List) {
      for (final item in attentionJson) {
        final event = _attentionFromRuntime(item);
        if (event != null) {
          attention.add(event);
        }
      }
    }
    final projectsJson = snapshot['projects'];
    final projects = <DitchProject>[];
    if (projectsJson is List) {
      for (final item in projectsJson) {
        final project = _projectFromRuntime(item);
        if (project != null) {
          projects.add(project);
        }
      }
    }
    setState(() {
      final selectedPath = _projects.isEmpty ? null : _selectedProject.path;
      if (projects.isNotEmpty) {
        projects.sort((a, b) => a.name.compareTo(b.name));
        _projects
          ..clear()
          ..addAll(projects);
        final restoredIndex = selectedPath == null
            ? -1
            : _projects.indexWhere((project) => project.path == selectedPath);
        _selectedProjectKey = _projectKey(
          _projects[restoredIndex >= 0 ? restoredIndex : 0],
        );
      }
      _agentSessions
        ..clear()
        ..addAll(sessions);
      _attention
        ..clear()
        ..addAll(attention);
      final visibleSessions = _visibleSessions;
      if (visibleSessions.isNotEmpty) {
        _expandedAgentLocalId =
            visibleSessions.any(
              (session) => session.localId == _expandedAgentLocalId,
            )
            ? _expandedAgentLocalId
            : visibleSessions.first.localId;
      } else {
        _expandedAgentLocalId = null;
      }
    });
    _scheduleStatusBarUpdate();
  }

  DitchProject? _projectFromRuntime(Object? value) {
    return parseRuntimeProject(value);
  }

  @override
  void initState() {
    super.initState();
    _scheduleStatusBarUpdate();
    if (widget.connectRuntimeOnStart) {
      unawaited(_connectRuntime());
    }
  }

  @override
  void dispose() {
    _presentation.dispose();
    _runtimeEvents?.cancel();
    _chatController.dispose();
    _agentListController.dispose();
    super.dispose();
  }

  Future<void> _addProject() async {
    if (!mounted) {
      return;
    }
    final project = await showDialog<DitchProject>(
      context: context,
      builder: (context) => const AddProjectDialog(),
    );
    if (project == null) {
      return;
    }

    final normalizedPath = canonicalProjectPath(project.path);
    if (_projects.any(
      (existing) => canonicalProjectPath(existing.path) == normalizedPath,
    )) {
      _showProjectSetupResult(
        title: 'Project already added',
        message: normalizedPath,
        isError: true,
      );
      return;
    }

    _showProjectSetupProgress();
    try {
      await _ensureRuntimeStarted();
      final response = await _runtimeClient.createProject(
        name: project.name,
        root: normalizedPath,
        gitPolicy: project.gitPolicy,
      );
      if (!mounted) {
        return;
      }
      Navigator.of(context, rootNavigator: true).pop();
      final configuredProject =
          _projectFromRuntime(response['ProjectCreated']) ??
          DitchProject(
            name: project.name,
            path: normalizedPath,
            gitPolicy:
                project.gitPolicy == ProjectGitPolicy.initializeRepository
                ? ProjectGitPolicy.requireRepository
                : project.gitPolicy,
          );
      late final AgentSession projectSession;
      setState(() {
        upsertProject(_projects, configuredProject);
        _selectedProjectKey = _projectKey(configuredProject);
        projectSession = _createAgentSession(expand: true);
      });
      _addChatMessage(
        projectSession,
        ChatMessageRole.system,
        'Project ready: ${project.name}. Verified .ditch/agents, .ditch/hooks, and .ditch/mcp.',
      );
      _showProjectSetupResult(
        title: 'Project ready',
        message:
            'The Ditch verified agents, hooks, and MCP directories in ${configuredProject.path}/.ditch.',
      );
    } on Object catch (error) {
      if (!mounted) {
        return;
      }
      Navigator.of(context, rootNavigator: true).pop();
      _showProjectSetupResult(
        title: 'Setup incomplete',
        message: '$error',
        isError: true,
      );
    }
  }

  void _showProjectSetupProgress() {
    showDialog<void>(
      context: context,
      barrierDismissible: false,
      builder: (context) => const AlertDialog(
        content: Row(
          children: [
            CircularProgressIndicator(),
            SizedBox(width: 20),
            Text('Configuring project…'),
          ],
        ),
      ),
    );
  }

  void _showProjectSetupResult({
    required String title,
    required String message,
    bool isError = false,
  }) {
    if (!mounted) {
      return;
    }
    showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        icon: Icon(isError ? Icons.error_outline : Icons.check_circle_outline),
        title: Text(title),
        content: Text(message),
        actions: [
          FilledButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Done'),
          ),
        ],
      ),
    );
  }

  Future<void> _startCodex() async {
    FocusManager.instance.primaryFocus?.unfocus();
    await _composerKey.currentState?.blur();
    if (!mounted) {
      return;
    }

    final prompt = await showDialog<String>(
      context: context,
      builder: (context) =>
          StartCodexSessionDialog(initialPrompt: _defaultStartPrompt),
    );
    if (prompt == null || prompt.trim().isEmpty) {
      return;
    }

    await _startCodexRuntime(prompt.trim());
  }

  Future<void> _startCodexRuntime(String prompt) async {
    if (!await _prepareSelectedProjectForCodex()) {
      return;
    }
    try {
      final response = await _runtimeClient.startCodexSession(
        projectName: _selectedProject.name,
        projectRoot: _selectedProject.path,
        prompt: prompt,
      );
      final run = response['AgentStarted'];
      final session = _agentSessionFromRuntime(run, [
        AgentChatMessage(
          role: ChatMessageRole.user,
          text: prompt,
          createdAt: DateTime.now(),
        ),
      ]);
      if (session != null) {
        final existing = _agentSessionByLocalId(session.localId);
        setState(() {
          if (existing == null) {
            _agentSessions.insert(0, session);
          }
          _expandedAgentLocalId = session.localId;
        });
        _scheduleStatusBarUpdate();
      }
    } on Object catch (error) {
      late final AgentSession session;
      setState(() {
        session = _createAgentSession(expand: true);
        session.status = AgentStatus.failed;
        session.currentPrompt = prompt;
        session.updatedAt = DateTime.now();
      });
      _scheduleStatusBarUpdate();
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Failed to start Codex through The Ditch Runtime: $error',
      );
      _addAttentionRequired(
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
        icon: Icons.error_outline,
        title: 'Codex failed to start',
        body: '$error',
      );
    }
  }

  Future<bool> _prepareSelectedProjectForCodex() async {
    if (!_runtimeSupportsPersistence) {
      _showProjectSetupResult(
        title: 'Outdated Ditch Runtime',
        message:
            'This runtime cannot persist agent sessions. Rebuild and restart The Ditch, then try again.',
        isError: true,
      );
      return false;
    }
    if (!_runtimeCodexHomeMatches) {
      _showProjectSetupResult(
        title: 'Codex home mismatch',
        message:
            'The app and runtime use different CODEX_HOME values. Restart the runtime from this app before starting Codex.',
        isError: true,
      );
      return false;
    }
    final project = _selectedProject;
    if (project.gitPolicy != ProjectGitPolicy.requireRepository ||
        isInsideGitWorkTree(project.path)) {
      return true;
    }
    final policy = await showDialog<ProjectGitPolicy>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Choose how Codex should run'),
        content: Text(
          '${project.name} is not inside a Git repository. You can initialize Git or explicitly allow Codex to run outside Git for this project.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          OutlinedButton(
            onPressed: () => Navigator.of(
              context,
            ).pop(ProjectGitPolicy.initializeRepository),
            child: const Text('Initialize Git Repository'),
          ),
          FilledButton(
            onPressed: () =>
                Navigator.of(context).pop(ProjectGitPolicy.allowOutsideGit),
            child: const Text('Allow Codex Outside Git'),
          ),
        ],
      ),
    );
    if (policy == null) {
      return false;
    }
    final response = await _runtimeClient.createProject(
      name: project.name,
      root: project.path,
      gitPolicy: policy,
    );
    final created = _projectFromRuntime(response['ProjectCreated']);
    if (created != null && mounted) {
      setState(() {
        _projects[_selectedProjectIndex] = created;
        _selectedProjectKey = _projectKey(created);
      });
    }
    return true;
  }

  Future<void> _submitComposer(AgentSession session, String prompt) async {
    if (session.isWorking) {
      return;
    }

    final cleanPrompt = prompt.trim();
    if (cleanPrompt.isEmpty) {
      return;
    }

    if (session.codexThreadId == null && !session.isTerminal) {
      await _startCodexRuntime(cleanPrompt);
      return;
    }

    if (session.cannotResumeWithoutThread) {
      _addAttentionRequired(
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
        icon: Icons.block_outlined,
        title: 'Session cannot be resumed',
        body:
            'Codex never created a thread for this session. Its failure details remain available, but you must start a new agent to continue.',
      );
      return;
    }

    if (session.originCodexHome != null &&
        _effectiveRuntimeCodexHome != null &&
        session.originCodexHome != _effectiveRuntimeCodexHome) {
      _addAttentionRequired(
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
        icon: Icons.account_circle_outlined,
        title: 'Resume requires the original Codex home',
        body:
            'This session used ${session.originCodexHome}, while the runtime currently uses $_effectiveRuntimeCodexHome. Its history remains available; start a new agent to use the current Codex account.',
      );
      return;
    }

    setState(() {
      session.status = AgentStatus.starting;
      session.currentPrompt = cleanPrompt;
      session.updatedAt = DateTime.now();
    });
    _scheduleStatusBarUpdate();
    try {
      await _runtimeClient.promptAgent(
        agentId: session.localId,
        prompt: cleanPrompt,
      );
      _resolveAttentionForSession(session);
    } on Object catch (error) {
      setState(() {
        session.status = AgentStatus.failed;
        session.updatedAt = DateTime.now();
      });
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Failed to send prompt through The Ditch Runtime: $error',
      );
      _addAttentionRequired(
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
        icon: Icons.error_outline,
        title: 'Codex prompt failed',
        body: '$error',
      );
    }
  }

  Future<void> _stopCodex(AgentSession session) async {
    _addChatMessage(session, ChatMessageRole.system, 'Stopping Codex...');
    try {
      await _runtimeClient.stopAgent(session.localId);
      setState(() {
        session.status = AgentStatus.stopped;
        session.updatedAt = DateTime.now();
      });
      _scheduleStatusBarUpdate();
    } on Object catch (error) {
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Failed to stop Codex through The Ditch Runtime: $error',
      );
    }
  }

  Future<void> _deleteAgent(AgentSession session) async {
    if (session.isWorking || !mounted) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Delete this agent?'),
        content: const Text(
          'This permanently removes the agent, its chat history, and its alerts from The Ditch. Project files are not deleted.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Delete Agent'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    try {
      await _runtimeClient.deleteAgent(session.localId);
    } on Object catch (error) {
      _addAttentionRequired(
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
        icon: Icons.delete_forever_outlined,
        title: 'Agent deletion failed',
        body: '$error',
      );
    }
  }

  Future<void> _renameAgent(AgentSession session, String? title) async {
    final cleanTitle = title?.trim();
    final previous = session.userTitle;
    setState(() {
      session.userTitle = cleanTitle == null || cleanTitle.isEmpty
          ? null
          : cleanTitle;
    });
    try {
      await _runtimeClient.renameAgent(session.localId, session.userTitle);
    } on Object catch (error) {
      if (!mounted) return;
      setState(() => session.userTitle = previous);
      _addAttentionRequired(
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
        icon: Icons.drive_file_rename_outline,
        title: 'Agent rename failed',
        body: '$error',
      );
    }
  }

  void _addAttentionRequired({
    required AttentionKind kind,
    required IconData icon,
    required String title,
    required String body,
    String? sessionLocalId,
    String? projectId,
    bool global = false,
  }) {
    if (!mounted) {
      return;
    }
    setState(() {
      _attention.insert(
        0,
        AttentionEvent(
          id: 'attention-${_nextAttentionId++}',
          kind: kind,
          icon: icon,
          title: title,
          body: body,
          sessionLocalId: sessionLocalId,
          projectId: global ? null : (projectId ?? _selectedProject.id),
          createdAt: DateTime.now(),
        ),
      );
    });
    _scheduleStatusBarUpdate();
  }

  void _openAttentionSession(AttentionEvent event) {
    final sessionLocalId = event.sessionLocalId;
    if (sessionLocalId == null ||
        _agentSessionByLocalId(sessionLocalId) == null) {
      return;
    }

    setState(() => _expandedAgentLocalId = sessionLocalId);
    _scheduleStatusBarUpdate();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_chatController.hasClients) {
        return;
      }
      _chatController.jumpTo(_chatController.position.maxScrollExtent);
    });
  }

  void _dismissAttention(AttentionEvent event) {
    setState(() {
      _attention.removeWhere((candidate) => candidate.id == event.id);
    });
    _scheduleStatusBarUpdate();
    unawaited(_runtimeClient.dismissAttention(event.id));
  }

  void _resolveAttentionForSession(AgentSession resumedSession) {
    final relatedSessionIds = _agentSessions
        .where(
          (session) =>
              session.localId == resumedSession.localId ||
              (resumedSession.codexThreadId != null &&
                  session.codexThreadId == resumedSession.codexThreadId),
        )
        .map((session) => session.localId)
        .toSet();
    final resolved = _attention
        .where((event) => relatedSessionIds.contains(event.sessionLocalId))
        .toList();
    if (resolved.isEmpty) {
      return;
    }
    setState(() {
      _attention.removeWhere(
        (event) => relatedSessionIds.contains(event.sessionLocalId),
      );
    });
    for (final event in resolved) {
      if (!event.id.startsWith('attention-')) {
        unawaited(_runtimeClient.dismissAttention(event.id));
      }
    }
    _scheduleStatusBarUpdate();
  }

  bool _canStopAttentionSession(AttentionEvent event) {
    final sessionLocalId = event.sessionLocalId;
    if (sessionLocalId == null) {
      return false;
    }
    return _agentSessionByLocalId(sessionLocalId)?.isWorking ?? false;
  }

  void _handleRuntimeEvent(Map<String, dynamic> envelopeBody) {
    final eventBody = envelopeBody['event'];
    if (eventBody is! Map<String, dynamic>) {
      return;
    }

    final snapshotReplaced = eventBody['SnapshotReplaced'];
    if (snapshotReplaced is Map<String, dynamic>) {
      _hydrateRuntimeSnapshot({'Snapshot': snapshotReplaced});
      return;
    }

    final projectChanged = eventBody['ProjectChanged'];
    if (projectChanged != null) {
      final project = _projectFromRuntime(projectChanged);
      if (project == null) {
        return;
      }
      setState(() {
        upsertProject(_projects, project);
      });
      return;
    }

    final agentChanged = eventBody['AgentChanged'];
    if (agentChanged != null) {
      final incoming = _agentSessionFromRuntime(agentChanged, const []);
      if (incoming == null) {
        return;
      }
      setState(() {
        final existed = _agentSessionByLocalId(incoming.localId) != null;
        reconcileAgentSession(_agentSessions, incoming);
        if (!existed) {
          _expandedAgentLocalId ??= incoming.localId;
        }
      });
      _scheduleStatusBarUpdate();
      return;
    }

    final agentDeleted = eventBody['AgentDeleted'];
    if (agentDeleted is Map<String, dynamic>) {
      final id = _agentIdToString(agentDeleted['agent_id']);
      if (id != null) {
        setState(() {
          _agentSessions.removeWhere((session) => session.localId == id);
          _attention.removeWhere((event) => event.sessionLocalId == id);
          if (_expandedAgentLocalId == id) _expandedAgentLocalId = null;
          if (_focusedAgentLocalId == id) _focusedAgentLocalId = null;
        });
        _scheduleStatusBarUpdate();
      }
      return;
    }

    final messageAppended = eventBody['AgentMessageAppended'];
    if (messageAppended != null) {
      final agentId = _agentIdFromRuntimeMessage(messageAppended);
      final message = _agentChatMessageFromRuntime(messageAppended);
      if (agentId == null || message == null) {
        return;
      }
      final session = _agentSessionByLocalId(agentId);
      if (session == null) {
        return;
      }
      _addChatMessage(session, message.role, message.text);
      return;
    }

    final attentionRaised = eventBody['AttentionRaised'];
    if (attentionRaised != null) {
      final attention = _attentionFromRuntime(attentionRaised);
      if (attention == null ||
          _attention.any((existing) => existing.id == attention.id)) {
        return;
      }
      setState(() => _attention.insert(0, attention));
      _scheduleStatusBarUpdate();
      return;
    }

    final attentionDismissed = eventBody['AttentionDismissed'];
    if (attentionDismissed is Map<String, dynamic>) {
      final id = attentionDismissed['attention_id']?.toString();
      if (id != null) {
        setState(() => _attention.removeWhere((item) => item.id == id));
        _scheduleStatusBarUpdate();
      }
      return;
    }

    final bell = eventBody['Bell'];
    if (bell is Map<String, dynamic>) {
      _addAttentionRequired(
        kind: AttentionKind.needsInput,
        icon: Icons.notifications_active_outlined,
        title: 'Codex needs attention',
        body: bell['reason']?.toString() ?? 'Agent session needs attention.',
        sessionLocalId: _agentIdToString(bell['agent_id']),
      );
    }
  }

  AgentSession? _agentSessionFromRuntime(
    Object? agentJson,
    List<AgentChatMessage> messages,
  ) {
    if (agentJson is! Map<String, dynamic>) {
      return null;
    }
    final agentId = _agentIdFromRuntimeAgent(agentJson);
    if (agentId == null) {
      return null;
    }
    final state = agentJson['state']?.toString();
    return AgentSession(
      localId: agentId,
      projectId: agentJson['project_id']?.toString(),
      provider: AgentProvider.codex,
      status: _agentStatusFromRuntime(state),
      messages: messages.isEmpty
          ? [
              AgentChatMessage(
                role: ChatMessageRole.system,
                text: 'Runtime session connected.',
                createdAt: DateTime.now(),
              ),
            ]
          : List<AgentChatMessage>.from(messages),
      codexThreadId: agentJson['native_session_id']?.toString(),
      codexTitle: agentJson['codex_title']?.toString(),
      userTitle: agentJson['user_title']?.toString(),
      originCodexHome: agentJson['origin_codex_home']?.toString(),
      currentPrompt: agentJson['current_prompt']?.toString(),
      lastVisibleAction: agentJson['last_visible_action']?.toString(),
      resumeBlockReason: agentJson['resume_block_reason']?.toString(),
      exitCode: agentJson['exit_code'] is int
          ? agentJson['exit_code'] as int
          : null,
      finishedAt: agentJson['finished_at'] == null
          ? null
          : _dateTimeFromRuntime(agentJson['finished_at']),
      createdAt: _dateTimeFromRuntime(agentJson['started_at']),
      updatedAt: _dateTimeFromRuntime(agentJson['updated_at']),
    );
  }

  AttentionEvent? _attentionFromRuntime(Object? value) {
    if (value is! Map<String, dynamic>) {
      return null;
    }
    final id = value['id']?.toString();
    final title = value['title']?.toString();
    final body = value['body']?.toString();
    if (id == null || title == null || body == null) {
      return null;
    }
    final kind = switch (value['kind']?.toString()) {
      'ApprovalRequired' => AttentionKind.approvalRequired,
      'Blocked' => AttentionKind.blocked,
      'Failed' => AttentionKind.failed,
      _ => AttentionKind.needsInput,
    };
    return AttentionEvent(
      id: id,
      kind: kind,
      icon: kind == AttentionKind.failed
          ? Icons.error_outline
          : Icons.notifications_active_outlined,
      title: title,
      body: body,
      sessionLocalId: _agentIdToString(value['agent_id']),
      projectId: value['project_id']?.toString(),
      createdAt: _dateTimeFromRuntime(value['created_at']),
    );
  }

  AgentChatMessage? _agentChatMessageFromRuntime(Object? messageJson) {
    if (messageJson is! Map<String, dynamic>) {
      return null;
    }
    final text = messageJson['text']?.toString();
    if (text == null || text.trim().isEmpty) {
      return null;
    }
    return AgentChatMessage(
      role: _chatRoleFromRuntime(messageJson['role']?.toString()),
      text: text,
      createdAt: _dateTimeFromRuntime(messageJson['created_at']),
    );
  }

  String? _agentIdFromRuntimeAgent(Object? agentJson) {
    if (agentJson is! Map<String, dynamic>) {
      return null;
    }
    return _agentIdToString(agentJson['id']);
  }

  String? _agentIdFromRuntimeMessage(Object? messageJson) {
    if (messageJson is! Map<String, dynamic>) {
      return null;
    }
    return _agentIdToString(messageJson['agent_id']);
  }

  String? _agentIdToString(Object? value) {
    if (value is String && value.isNotEmpty) {
      return value;
    }
    if (value is Map && value['0'] is String) {
      return value['0'] as String;
    }
    return null;
  }

  AgentStatus _agentStatusFromRuntime(String? state) {
    return switch (state) {
      'Starting' => AgentStatus.starting,
      'Working' || 'AwaitingApproval' || 'Blocked' => AgentStatus.working,
      'Completed' => AgentStatus.completed,
      'Failed' || 'Stale' || 'Unknown' => AgentStatus.failed,
      'Interrupted' => AgentStatus.stopped,
      _ => AgentStatus.idle,
    };
  }

  ChatMessageRole _chatRoleFromRuntime(String? role) {
    return switch (role) {
      'User' => ChatMessageRole.user,
      'Assistant' => ChatMessageRole.assistant,
      'Tool' => ChatMessageRole.tool,
      _ => ChatMessageRole.system,
    };
  }

  DateTime _dateTimeFromRuntime(Object? value) {
    if (value is String) {
      return DateTime.tryParse(value)?.toLocal() ?? DateTime.now();
    }
    return DateTime.now();
  }

  void _addChatMessage(
    AgentSession session,
    ChatMessageRole role,
    String text,
  ) {
    if (!mounted) {
      return;
    }
    setState(() {
      if (session.messages.isNotEmpty) {
        final lastMessage = session.messages.last;
        if (lastMessage.role == role && lastMessage.text == text) {
          return;
        }
      }
      session.messages.add(
        AgentChatMessage(role: role, text: text, createdAt: DateTime.now()),
      );
      session.updatedAt = DateTime.now();
    });
    _scheduleStatusBarUpdate();
  }

  void _focusComposerAfterLayout() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _composerKey.currentState?.focus();
    });
  }

  void _toggleExpandedAgent(AgentSession session) {
    final collapsing = _expandedAgentLocalId == session.localId;
    setState(() {
      _expandedAgentLocalId = collapsing ? null : session.localId;
    });
    if (collapsing) return;
    WidgetsBinding.instance.addPostFrameCallback((_) async {
      if (!mounted) return;
      final headerContext = GlobalObjectKey(
        'agent-header-${session.localId}',
      ).currentContext;
      if (headerContext != null) {
        await Scrollable.ensureVisible(
          headerContext,
          alignment: 0,
          duration: const Duration(milliseconds: 180),
          curve: Curves.easeOut,
        );
      }
      if (mounted) _composerKey.currentState?.focus();
    });
  }

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<CommandCenterPresentationState>(
      valueListenable: _presentation,
      builder: (context, presentation, _) {
        return Scaffold(
          body: LayoutBuilder(
            builder: (context, constraints) {
              final compact = constraints.maxWidth < 760;
              final showSidebar =
                  presentation.sidebarVisible && _focusedAgentLocalId == null;
              final showInspector =
                  presentation.inspectorVisible &&
                  !compact &&
                  _focusedAgentLocalId == null;
              final agentsSurface = AgentsSurface(
                sessions: _visibleSessions,
                expandedAgentLocalId: _expandedAgentLocalId,
                focusedAgentLocalId: _focusedAgentLocalId,
                chatController: _chatController,
                agentListController: _agentListController,
                composerKey: _composerKey,
                initialPrompt: _defaultStartPrompt,
                effectiveCodexHome: _effectiveRuntimeCodexHome,
                onStartCodex: _startCodex,
                onStartPrompt: _startCodexRuntime,
                onSubmitPrompt: _submitComposer,
                onStopCodex: _stopCodex,
                onDeleteAgent: _deleteAgent,
                onRenameAgent: _renameAgent,
                onFocusAgent: (session) {
                  setState(() {
                    _focusedAgentLocalId = session?.localId;
                    if (session != null) {
                      _expandedAgentLocalId = session.localId;
                    }
                  });
                  if (session != null) _focusComposerAfterLayout();
                },
                onToggleExpanded: _toggleExpandedAgent,
              );
              final attentionPanel = AttentionPanel(
                width: 320,
                events: _visibleAttention,
                canStopSession: _canStopAttentionSession,
                onOpenSession: _openAttentionSession,
                onStopSession: (event) {
                  final id = event.sessionLocalId;
                  final session = id == null
                      ? null
                      : _agentSessionByLocalId(id);
                  if (session != null) _stopCodex(session);
                },
                onDismiss: _dismissAttention,
              );

              return ColoredBox(
                color: context.ditch.workspace,
                child: Column(
                  children: [
                    DitchToolbar(
                      projectName: _selectedProject.name,
                      connection: presentation.connection,
                      attentionCount: _visibleAttention.length,
                      sidebarVisible: showSidebar,
                      inspectorVisible: showInspector,
                      onToggleSidebar: _presentation.toggleSidebar,
                      onToggleInspector: _presentation.toggleInspector,
                      onNewAgent: _startCodex,
                    ),
                    const Divider(height: 1),
                    Expanded(
                      child:
                          presentation.connection ==
                              RuntimeConnectionPhase.unavailable
                          ? RuntimeRecoveryView(
                              socketPath: _runtimeClient.socketPath,
                              error: presentation.connectionError,
                              onRetry: _connectRuntime,
                              onOpenActivityMonitor: () => _applicationChannel
                                  .invokeMethod<bool>('openActivityMonitor'),
                              onQuit: () => _applicationChannel
                                  .invokeMethod<bool>('quitUI'),
                            )
                          : Row(
                              children: [
                                if (showSidebar) ...[
                                  ProjectSidebar(
                                    projects: _projects,
                                    selectedIndex: _selectedProjectIndex,
                                    onAddProject: _addProject,
                                    onSelectProject: _selectProject,
                                  ),
                                  const VerticalDivider(width: 1),
                                ],
                                Expanded(child: agentsSurface),
                                if (showInspector) ...[
                                  const VerticalDivider(width: 1),
                                  attentionPanel,
                                ],
                              ],
                            ),
                    ),
                  ],
                ),
              );
            },
          ),
        );
      },
    );
  }
}

class DitchToolbar extends StatelessWidget {
  const DitchToolbar({
    required this.projectName,
    required this.connection,
    required this.attentionCount,
    required this.sidebarVisible,
    required this.inspectorVisible,
    required this.onToggleSidebar,
    required this.onToggleInspector,
    required this.onNewAgent,
    super.key,
  });

  final String projectName;
  final RuntimeConnectionPhase connection;
  final int attentionCount;
  final bool sidebarVisible;
  final bool inspectorVisible;
  final VoidCallback onToggleSidebar;
  final VoidCallback onToggleInspector;
  final VoidCallback onNewAgent;

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;
    final (label, color) = switch (connection) {
      RuntimeConnectionPhase.connected => (
        'Connected',
        const Color(0xff34c759),
      ),
      RuntimeConnectionPhase.connecting => (
        'Connecting',
        const Color(0xffff9f0a),
      ),
      RuntimeConnectionPhase.reconnecting => (
        'Reconnecting',
        const Color(0xffff9f0a),
      ),
      RuntimeConnectionPhase.unavailable => (
        'Runtime unavailable',
        Theme.of(context).colorScheme.error,
      ),
    };
    return SizedBox(
      height: tokens.toolbarHeight,
      child: Padding(
        padding: const EdgeInsets.only(left: 78, right: 10),
        child: Row(
          children: [
            IconButton(
              tooltip: sidebarVisible ? 'Hide sidebar' : 'Show sidebar',
              onPressed: onToggleSidebar,
              icon: Icon(
                sidebarVisible
                    ? Icons.view_sidebar
                    : Icons.view_sidebar_outlined,
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                projectName,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: Theme.of(context).textTheme.titleMedium,
              ),
            ),
            Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(color: color, shape: BoxShape.circle),
            ),
            const SizedBox(width: 7),
            Text(label, style: Theme.of(context).textTheme.bodySmall),
            const SizedBox(width: 12),
            FilledButton.icon(
              onPressed: onNewAgent,
              icon: const Icon(Icons.add, size: 16),
              label: const Text('New Agent'),
            ),
            const SizedBox(width: 6),
            Badge(
              isLabelVisible: attentionCount > 0,
              label: Text('$attentionCount'),
              child: IconButton(
                tooltip: inspectorVisible ? 'Hide attention' : 'Show attention',
                onPressed: onToggleInspector,
                icon: Icon(
                  inspectorVisible
                      ? Icons.vertical_split
                      : Icons.vertical_split_outlined,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class RuntimeConnectionBanner extends StatelessWidget {
  const RuntimeConnectionBanner({
    this.message,
    required this.onRetry,
    super.key,
  });

  final String? message;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      color: Theme.of(context).colorScheme.error.withValues(alpha: 0.08),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 9),
        child: Row(
          children: [
            Icon(
              Icons.warning_amber_rounded,
              color: Theme.of(context).colorScheme.error,
            ),
            const SizedBox(width: 9),
            Expanded(
              child: Text(
                message == null
                    ? 'The Ditch Runtime is not responding. Agents may still be running.'
                    : 'The Ditch Runtime is not responding. Agents may still be running. $message',
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            TextButton(onPressed: onRetry, child: const Text('Retry')),
          ],
        ),
      ),
    );
  }
}

class RuntimeRecoveryView extends StatelessWidget {
  const RuntimeRecoveryView({
    required this.socketPath,
    required this.error,
    required this.onRetry,
    required this.onOpenActivityMonitor,
    required this.onQuit,
    super.key,
  });

  final String socketPath;
  final String? error;
  final VoidCallback onRetry;
  final Future<bool?> Function() onOpenActivityMonitor;
  final Future<bool?> Function() onQuit;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(32),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 620),
          child: DitchSurface(
            padding: const EdgeInsets.all(24),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(
                  Icons.sync_problem_rounded,
                  size: 30,
                  color: Theme.of(context).colorScheme.error,
                ),
                const SizedBox(height: 16),
                Text(
                  'The Ditch Runtime is not responding',
                  style: Theme.of(context).textTheme.headlineSmall,
                ),
                const SizedBox(height: 8),
                const Text(
                  'Agent processes may still be running. The Ditch could not reconnect to its local runtime service.',
                ),
                const SizedBox(height: 20),
                Text(
                  'Expected process',
                  style: Theme.of(context).textTheme.labelLarge,
                ),
                const SizedBox(height: 4),
                const SelectableText('The Ditch Runtime'),
                const SizedBox(height: 12),
                Text('Socket', style: Theme.of(context).textTheme.labelLarge),
                const SizedBox(height: 4),
                SelectableText(socketPath),
                if (error != null) ...[
                  const SizedBox(height: 12),
                  Text(
                    'Last error',
                    style: Theme.of(context).textTheme.labelLarge,
                  ),
                  const SizedBox(height: 4),
                  SelectableText(error!),
                ],
                const SizedBox(height: 20),
                const Text(
                  'Search for “The Ditch Runtime” in Activity Monitor. Force Quit Applications does not list macOS background services.',
                ),
                const SizedBox(height: 20),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    FilledButton.icon(
                      onPressed: onRetry,
                      icon: const Icon(Icons.refresh, size: 16),
                      label: const Text('Retry Connection'),
                    ),
                    OutlinedButton.icon(
                      onPressed: onOpenActivityMonitor,
                      icon: const Icon(Icons.monitor_heart_outlined, size: 16),
                      label: const Text('Open Activity Monitor'),
                    ),
                    TextButton(onPressed: onQuit, child: const Text('Quit UI')),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class ProjectSidebar extends StatelessWidget {
  const ProjectSidebar({
    required this.projects,
    required this.selectedIndex,
    required this.onAddProject,
    required this.onSelectProject,
    super.key,
  });

  final List<DitchProject> projects;
  final int selectedIndex;
  final VoidCallback onAddProject;
  final ValueChanged<int> onSelectProject;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return ColoredBox(
      color: context.ditch.sidebar,
      child: SizedBox(
        width: 240,
        child: SafeArea(
          top: false,
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'PROJECTS',
                  style: theme.textTheme.labelSmall?.copyWith(
                    color: context.ditch.mutedText,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.6,
                  ),
                ),
                const SizedBox(height: 8),
                Expanded(
                  child: ListView.separated(
                    itemCount: projects.length,
                    separatorBuilder: (_, _) => const SizedBox(height: 8),
                    itemBuilder: (context, index) {
                      final project = projects[index];
                      return ProjectTile(
                        name: project.name,
                        path: project.path,
                        selected: index == selectedIndex,
                        onTap: () => onSelectProject(index),
                      );
                    },
                  ),
                ),
                const SizedBox(height: 8),
                TextButton.icon(
                  onPressed: onAddProject,
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

class ProjectTile extends StatelessWidget {
  const ProjectTile({
    required this.name,
    required this.path,
    required this.selected,
    required this.onTap,
    super.key,
  });

  final String name;
  final String path;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;

    return InkWell(
      borderRadius: BorderRadius.circular(tokens.radiusSmall),
      onTap: onTap,
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: selected ? tokens.selection : Colors.transparent,
          borderRadius: BorderRadius.circular(tokens.radiusSmall),
        ),
        child: Padding(
          padding: const EdgeInsets.all(10),
          child: Row(
            children: [
              const Icon(Icons.folder_open, size: 18),
              const SizedBox(width: 8),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(name, maxLines: 1, overflow: TextOverflow.ellipsis),
                    Text(
                      path,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class AgentsSurface extends StatelessWidget {
  const AgentsSurface({
    required this.sessions,
    required this.expandedAgentLocalId,
    required this.focusedAgentLocalId,
    required this.chatController,
    required this.agentListController,
    required this.composerKey,
    required this.initialPrompt,
    this.effectiveCodexHome,
    required this.onStartCodex,
    this.onStartPrompt,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    required this.onDeleteAgent,
    required this.onRenameAgent,
    required this.onFocusAgent,
    required this.onToggleExpanded,
    super.key,
  });

  final List<AgentSession> sessions;
  final String? expandedAgentLocalId;
  final String? focusedAgentLocalId;
  final ScrollController chatController;
  final ScrollController agentListController;
  final GlobalKey<AgentComposerState> composerKey;
  final String initialPrompt;
  final String? effectiveCodexHome;
  final VoidCallback onStartCodex;
  final ValueChanged<String>? onStartPrompt;
  final void Function(AgentSession session, String prompt) onSubmitPrompt;
  final ValueChanged<AgentSession> onStopCodex;
  final ValueChanged<AgentSession> onDeleteAgent;
  final void Function(AgentSession session, String? title) onRenameAgent;
  final ValueChanged<AgentSession?> onFocusAgent;
  final ValueChanged<AgentSession> onToggleExpanded;

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      top: false,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(18, 14, 18, 18),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (focusedAgentLocalId == null) ...[
              Text('Agents', style: Theme.of(context).textTheme.headlineSmall),
              const SizedBox(height: 12),
            ],
            Expanded(
              child: AgentSessionList(
                sessions: sessions,
                expandedAgentLocalId: expandedAgentLocalId,
                focusedAgentLocalId: focusedAgentLocalId,
                chatController: chatController,
                agentListController: agentListController,
                composerKey: composerKey,
                initialPrompt: initialPrompt,
                effectiveCodexHome: effectiveCodexHome,
                onStartPrompt: onStartPrompt ?? (_) {},
                onToggleExpanded: onToggleExpanded,
                onSubmitPrompt: onSubmitPrompt,
                onStopCodex: onStopCodex,
                onDeleteAgent: onDeleteAgent,
                onRenameAgent: onRenameAgent,
                onFocusAgent: onFocusAgent,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class AgentSessionList extends StatelessWidget {
  const AgentSessionList({
    required this.sessions,
    required this.expandedAgentLocalId,
    required this.focusedAgentLocalId,
    required this.chatController,
    required this.agentListController,
    required this.composerKey,
    required this.initialPrompt,
    this.effectiveCodexHome,
    required this.onStartPrompt,
    required this.onToggleExpanded,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    required this.onDeleteAgent,
    required this.onRenameAgent,
    required this.onFocusAgent,
    super.key,
  });

  final List<AgentSession> sessions;
  final String? expandedAgentLocalId;
  final String? focusedAgentLocalId;
  final ScrollController chatController;
  final ScrollController agentListController;
  final GlobalKey<AgentComposerState> composerKey;
  final String initialPrompt;
  final String? effectiveCodexHome;
  final ValueChanged<String> onStartPrompt;
  final ValueChanged<AgentSession> onToggleExpanded;
  final void Function(AgentSession session, String prompt) onSubmitPrompt;
  final ValueChanged<AgentSession> onStopCodex;
  final ValueChanged<AgentSession> onDeleteAgent;
  final void Function(AgentSession session, String? title) onRenameAgent;
  final ValueChanged<AgentSession?> onFocusAgent;

  @override
  Widget build(BuildContext context) {
    if (sessions.isEmpty) {
      return DitchSurface(
        key: const Key('ready-agent-card'),
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Row(
              children: [
                Icon(Icons.memory, size: 24),
                SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [Text('Codex'), Text('Ready for a new prompt')],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 12),
            Expanded(
              child: AgentChatPanel(
                messages: const [],
                controller: chatController,
                agentListController: agentListController,
                enlarged: false,
                composerKey: composerKey,
                initialPrompt: initialPrompt,
                hasSession: false,
                isWorking: false,
                onSubmitPrompt: onStartPrompt,
                onStopCodex: () {},
              ),
            ),
          ],
        ),
      );
    }
    AgentSession? focused;
    if (focusedAgentLocalId != null) {
      for (final item in sessions) {
        if (item.localId == focusedAgentLocalId) {
          focused = item;
          break;
        }
      }
    }
    if (focused != null) {
      final focusedSession = focused;
      return Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 1180),
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: ExpandableAgentPanel(
              key: ValueKey('focused-${focusedSession.localId}'),
              session: focusedSession,
              expanded: true,
              enlarged: true,
              chatController: chatController,
              agentListController: agentListController,
              composerKey: composerKey,
              initialPrompt: initialPrompt,
              effectiveCodexHome: effectiveCodexHome,
              onTap: () {},
              onEnlarge: () => onFocusAgent(null),
              onDelete: () => onDeleteAgent(focusedSession),
              onRename: (title) => onRenameAgent(focusedSession, title),
              onSubmitPrompt: (prompt) =>
                  onSubmitPrompt(focusedSession, prompt),
              onStopCodex: () => onStopCodex(focusedSession),
            ),
          ),
        ),
      );
    }
    return LayoutBuilder(
      builder: (context, constraints) => Scrollbar(
        controller: agentListController,
        interactive: true,
        child: ListView.separated(
          key: const PageStorageKey<String>('agent-session-list'),
          controller: agentListController,
          physics: expandedAgentLocalId == null
              ? null
              : const NeverScrollableScrollPhysics(),
          itemCount: sessions.length,
          separatorBuilder: (_, _) => const SizedBox(height: 10),
          itemBuilder: (context, index) {
            final session = sessions[index];
            final expanded = expandedAgentLocalId == session.localId;
            final panel = ExpandableAgentPanel(
              key: ValueKey(session.localId),
              session: session,
              expanded: expanded,
              enlarged: false,
              fillAvailable: expanded,
              chatController: expanded ? chatController : null,
              agentListController: agentListController,
              composerKey: expanded ? composerKey : null,
              initialPrompt: initialPrompt,
              effectiveCodexHome: effectiveCodexHome,
              onTap: () => onToggleExpanded(session),
              onEnlarge: () => onFocusAgent(session),
              onDelete: () => onDeleteAgent(session),
              onRename: (title) => onRenameAgent(session, title),
              onSubmitPrompt: (prompt) => onSubmitPrompt(session, prompt),
              onStopCodex: () => onStopCodex(session),
            );
            if (!expanded) return panel;
            return SizedBox(height: constraints.maxHeight, child: panel);
          },
        ),
      ),
    );
  }
}

class EditableAgentTitle extends StatefulWidget {
  const EditableAgentTitle({
    required this.title,
    required this.hasOverride,
    required this.onRename,
    super.key,
  });

  final String title;
  final bool hasOverride;
  final ValueChanged<String?> onRename;

  @override
  State<EditableAgentTitle> createState() => _EditableAgentTitleState();
}

class _EditableAgentTitleState extends State<EditableAgentTitle> {
  late final TextEditingController _controller;
  bool _editing = false;

  @override
  void initState() {
    super.initState();
    _controller = TextEditingController(text: widget.title);
  }

  @override
  void didUpdateWidget(EditableAgentTitle oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (!_editing && oldWidget.title != widget.title) {
      _controller.text = widget.title;
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _save() {
    final value = _controller.text.trim();
    setState(() => _editing = false);
    widget.onRename(value.isEmpty ? null : value);
  }

  void _cancel() {
    _controller.text = widget.title;
    setState(() => _editing = false);
  }

  @override
  Widget build(BuildContext context) {
    if (_editing) {
      return CallbackShortcuts(
        bindings: {const SingleActivator(LogicalKeyboardKey.escape): _cancel},
        child: TextField(
          key: const Key('agent-title-editor'),
          controller: _controller,
          autofocus: true,
          maxLines: 1,
          onSubmitted: (_) => _save(),
          decoration: const InputDecoration(isDense: true),
        ),
      );
    }
    return Tooltip(
      message: widget.hasOverride
          ? 'Click to rename. Clear to restore the Codex title.'
          : 'Click to rename',
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => setState(() => _editing = true),
        child: Text(widget.title),
      ),
    );
  }
}

class ExpandableAgentPanel extends StatelessWidget {
  const ExpandableAgentPanel({
    required this.session,
    required this.expanded,
    required this.enlarged,
    this.fillAvailable = false,
    required this.chatController,
    required this.agentListController,
    required this.composerKey,
    required this.initialPrompt,
    this.effectiveCodexHome,
    required this.onTap,
    required this.onEnlarge,
    required this.onDelete,
    required this.onRename,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    super.key,
  });

  final AgentSession session;
  final bool expanded;
  final bool enlarged;
  final bool fillAvailable;
  final ScrollController? chatController;
  final ScrollController agentListController;
  final GlobalKey<AgentComposerState>? composerKey;
  final String initialPrompt;
  final String? effectiveCodexHome;
  final VoidCallback onTap;
  final VoidCallback onEnlarge;
  final VoidCallback onDelete;
  final ValueChanged<String?> onRename;
  final ValueChanged<String> onSubmitPrompt;
  final VoidCallback onStopCodex;

  @override
  Widget build(BuildContext context) {
    final homeMismatch =
        session.hasCodexThread &&
        session.originCodexHome != null &&
        effectiveCodexHome != null &&
        session.originCodexHome != effectiveCodexHome;
    final noThread =
        session.cannotResumeWithoutThread ||
        session.resumeBlockReason == 'NoCodexThread';
    final resumeBlocked = noThread || homeMismatch;
    final resumeBlockedMessage = noThread
        ? 'This session cannot be resumed because Codex never created a thread. Start a new agent to continue.'
        : homeMismatch
        ? 'This session used ${session.originCodexHome}. Switch the runtime to that CODEX_HOME to resume it, or start a new agent.'
        : null;

    return LayoutBuilder(
      builder: (context, constraints) {
        final details = Row(
          children: [
            Icon(_providerIcon(session.provider), size: 24),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  EditableAgentTitle(
                    title: session.displayName,
                    hasOverride: session.userTitle?.trim().isNotEmpty ?? false,
                    onRename: onRename,
                  ),
                  const SizedBox(height: 2),
                  Text(_statusLabel(session.status)),
                  if (session.lastVisibleAction != null) ...[
                    const SizedBox(height: 2),
                    Text(
                      session.lastVisibleAction!,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                  if (session.currentPrompt != null) ...[
                    const SizedBox(height: 2),
                    Text(
                      session.currentPrompt!,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ],
                ],
              ),
            ),
          ],
        );
        final actions = Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            IconButton(
              onPressed: onEnlarge,
              tooltip: enlarged
                  ? 'Return to agents (Esc)'
                  : 'Focus agent (⇧⌘F)',
              icon: Icon(
                enlarged ? Icons.close_fullscreen : Icons.open_in_full,
              ),
            ),
            IconButton(
              onPressed: onTap,
              tooltip: expanded ? 'Collapse agent' : 'Expand agent',
              icon: Icon(
                expanded ? Icons.keyboard_arrow_up : Icons.keyboard_arrow_down,
              ),
            ),
            OutlinedButton.icon(
              onPressed: session.isWorking ? onStopCodex : null,
              icon: const Icon(Icons.stop_circle_outlined),
              label: const Text('Stop'),
            ),
            IconButton(
              onPressed: session.isWorking ? null : onDelete,
              tooltip: 'Delete agent permanently',
              icon: const Icon(Icons.delete_outline),
            ),
          ],
        );
        Widget buildChatPanel() => AgentChatPanel(
          messages: session.messages,
          controller: chatController!,
          agentListController: agentListController,
          enlarged: enlarged,
          embedded: !(enlarged || fillAvailable),
          composerKey: composerKey!,
          initialPrompt: session.hasCodexThread ? '' : initialPrompt,
          hasSession: session.hasCodexThread,
          isWorking: session.isWorking,
          enabled: !resumeBlocked,
          disabledMessage: resumeBlockedMessage,
          onSubmitPrompt: onSubmitPrompt,
          onStopCodex: onStopCodex,
          onEnlarge: onEnlarge,
        );

        return CallbackShortcuts(
          bindings: {
            const SingleActivator(
              LogicalKeyboardKey.keyF,
              meta: true,
              shift: true,
            ): onEnlarge,
            if (enlarged)
              const SingleActivator(LogicalKeyboardKey.escape): onEnlarge,
          },
          child: Focus(
            autofocus: enlarged,
            child: DitchSurface(
              padding: const EdgeInsets.all(12),
              bordered: false,
              child: Column(
                children: [
                  InkWell(
                    key: GlobalObjectKey('agent-header-${session.localId}'),
                    onTap: onTap,
                    borderRadius: BorderRadius.circular(8),
                    child: Padding(
                      padding: const EdgeInsets.all(4),
                      child: constraints.maxWidth < 500
                          ? Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                details,
                                const SizedBox(height: 12),
                                actions,
                              ],
                            )
                          : Row(
                              children: [
                                Expanded(child: details),
                                const SizedBox(width: 12),
                                actions,
                              ],
                            ),
                    ),
                  ),
                  if (expanded && (enlarged || fillAvailable))
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.only(top: 12),
                        child: buildChatPanel(),
                      ),
                    )
                  else if (expanded)
                    Padding(
                      padding: const EdgeInsets.only(top: 12),
                      child: buildChatPanel(),
                    ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  IconData _providerIcon(AgentProvider provider) {
    return switch (provider) {
      AgentProvider.codex => Icons.memory,
    };
  }

  String _statusLabel(AgentStatus status) {
    return switch (status) {
      AgentStatus.idle => 'No active run yet',
      AgentStatus.starting => 'Starting',
      AgentStatus.working => 'Working',
      AgentStatus.completed => 'Completed',
      AgentStatus.failed => 'Failed',
      AgentStatus.stopped => 'Stopped',
    };
  }
}

class AgentChatPanel extends StatelessWidget {
  const AgentChatPanel({
    required this.messages,
    required this.controller,
    required this.agentListController,
    required this.enlarged,
    this.embedded = false,
    required this.composerKey,
    required this.initialPrompt,
    required this.hasSession,
    required this.isWorking,
    this.enabled = true,
    this.disabledMessage,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    this.onEnlarge,
    super.key,
  });

  final List<AgentChatMessage> messages;
  final ScrollController controller;
  final ScrollController agentListController;
  final bool enlarged;
  final bool embedded;
  final GlobalKey<AgentComposerState> composerKey;
  final String initialPrompt;
  final bool hasSession;
  final bool isWorking;
  final bool enabled;
  final String? disabledMessage;
  final ValueChanged<String> onSubmitPrompt;
  final VoidCallback onStopCodex;
  final VoidCallback? onEnlarge;

  @override
  Widget build(BuildContext context) {
    final toolbar = SizedBox(
      height: 36,
      child: Row(
        mainAxisAlignment: MainAxisAlignment.end,
        children: [
          IconButton(
            tooltip: 'Copy conversation',
            icon: const Icon(Icons.copy_all_outlined, size: 18),
            onPressed: messages.isEmpty
                ? null
                : () => Clipboard.setData(
                    ClipboardData(
                      text: messages
                          .map(
                            (message) =>
                                '${message.role.name}: ${message.text}',
                          )
                          .join('\n\n'),
                    ),
                  ),
          ),
        ],
      ),
    );
    final footer = <Widget>[
      const Divider(height: 1),
      ThinkingStatusStrip(visible: isWorking),
      if (disabledMessage != null)
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Text(disabledMessage!),
        ),
      AgentComposer(
        key: composerKey,
        initialText: initialPrompt,
        hasSession: hasSession,
        isWorking: isWorking,
        enabled: enabled,
        onSubmit: onSubmitPrompt,
        onStop: onStopCodex,
        onEnlarge: onEnlarge,
        onEscape: enlarged ? onEnlarge : null,
      ),
    ];

    if (embedded) {
      return ColoredBox(
        color: context.ditch.workspace,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            toolbar,
            const Divider(height: 1),
            Padding(
              padding: const EdgeInsets.all(12),
              child: Column(
                children: [
                  for (var index = 0; index < messages.length; index++) ...[
                    if (index > 0) const SizedBox(height: 10),
                    AgentChatBubble(
                      message: messages[index],
                      chatController: controller,
                      agentListController: agentListController,
                      embedded: true,
                    ),
                  ],
                ],
              ),
            ),
            ...footer,
          ],
        ),
      );
    }

    return ConversationViewportLifecycle(
      controller: controller,
      composerKey: composerKey,
      messageCount: messages.length,
      child: ColoredBox(
        color: context.ditch.workspace,
        child: Column(
          children: [
            toolbar,
            const Divider(height: 1),
            Expanded(
              child: Scrollbar(
                controller: controller,
                interactive: true,
                child: ListView.separated(
                  controller: controller,
                  padding: const EdgeInsets.all(12),
                  itemCount: messages.length,
                  separatorBuilder: (_, _) => const SizedBox(height: 10),
                  itemBuilder: (context, index) {
                    return AgentChatBubble(
                      message: messages[index],
                      chatController: controller,
                      agentListController: agentListController,
                      embedded: false,
                    );
                  },
                ),
              ),
            ),
            ...footer,
          ],
        ),
      ),
    );
  }
}

class ConversationViewportLifecycle extends StatefulWidget {
  const ConversationViewportLifecycle({
    required this.controller,
    required this.composerKey,
    required this.messageCount,
    required this.child,
    super.key,
  });

  final ScrollController controller;
  final GlobalKey<AgentComposerState> composerKey;
  final int messageCount;
  final Widget child;

  @override
  State<ConversationViewportLifecycle> createState() =>
      _ConversationViewportLifecycleState();
}

class _ConversationViewportLifecycleState
    extends State<ConversationViewportLifecycle> {
  static const _nearBottomThreshold = 72.0;

  @override
  void initState() {
    super.initState();
    _scheduleBottom(focusComposer: true, animate: false);
  }

  @override
  void didUpdateWidget(ConversationViewportLifecycle oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.messageCount == oldWidget.messageCount) return;

    final shouldFollow =
        !widget.controller.hasClients ||
        widget.controller.position.maxScrollExtent -
                widget.controller.position.pixels <=
            _nearBottomThreshold;
    if (shouldFollow) {
      _scheduleBottom(focusComposer: false, animate: true);
    }
  }

  void _scheduleBottom({required bool focusComposer, required bool animate}) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      if (widget.controller.hasClients) {
        final target = widget.controller.position.maxScrollExtent;
        if (animate) {
          widget.controller.animateTo(
            target,
            duration: const Duration(milliseconds: 180),
            curve: Curves.easeOut,
          );
        } else {
          widget.controller.jumpTo(target);
        }
      }
      if (focusComposer) widget.composerKey.currentState?.focus();
    });
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

class ThinkingStatusStrip extends StatefulWidget {
  const ThinkingStatusStrip({required this.visible, super.key});

  final bool visible;

  @override
  State<ThinkingStatusStrip> createState() => _ThinkingStatusStripState();
}

class _ThinkingStatusStripState extends State<ThinkingStatusStrip>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 900),
    );
    if (widget.visible) {
      _controller.repeat();
    }
  }

  @override
  void didUpdateWidget(ThinkingStatusStrip oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.visible && !_controller.isAnimating) {
      _controller.repeat();
    } else if (!widget.visible && _controller.isAnimating) {
      _controller.stop();
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;

    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 160),
      child: widget.visible
          ? Container(
              key: const ValueKey('thinking-status'),
              width: double.infinity,
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
              child: Align(
                alignment: Alignment.centerLeft,
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: colors.inverseSurface,
                    borderRadius: BorderRadius.circular(8),
                  ),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 7,
                    ),
                    child: AnimatedBuilder(
                      animation: _controller,
                      builder: (context, _) {
                        final dots = 1 + (_controller.value * 3).floor();
                        return Text(
                          'Thinking${'.' * dots}',
                          style: Theme.of(context).textTheme.labelLarge
                              ?.copyWith(color: colors.onInverseSurface),
                        );
                      },
                    ),
                  ),
                ),
              ),
            )
          : const SizedBox.shrink(key: ValueKey('thinking-status-hidden')),
    );
  }
}

class AgentComposer extends StatefulWidget {
  const AgentComposer({
    required this.initialText,
    required this.hasSession,
    required this.isWorking,
    this.enabled = true,
    required this.onSubmit,
    required this.onStop,
    this.onEnlarge,
    this.onEscape,
    super.key,
  });

  final String initialText;
  final bool hasSession;
  final bool isWorking;
  final bool enabled;
  final ValueChanged<String> onSubmit;
  final VoidCallback onStop;
  final VoidCallback? onEnlarge;
  final VoidCallback? onEscape;

  @override
  State<AgentComposer> createState() => AgentComposerState();
}

class AgentComposerState extends State<AgentComposer> {
  final _nativeComposerKey = GlobalKey<NativeComposerTextViewState>();
  late String _draftText;

  @override
  void initState() {
    super.initState();
    _draftText = widget.initialText;
  }

  @override
  void didUpdateWidget(AgentComposer oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.initialText != widget.initialText &&
        _draftText.trim().isEmpty) {
      _draftText = widget.initialText;
    }
  }

  void focus() {
    _nativeComposerKey.currentState?.focus();
  }

  Future<void> blur() async {
    await _nativeComposerKey.currentState?.blur();
  }

  Future<void> submit() async {
    if (!widget.enabled || widget.isWorking) {
      return;
    }

    final nativeText =
        await _nativeComposerKey.currentState?.currentText() ?? _draftText;
    final prompt = nativeText.trim();
    if (prompt.isEmpty) {
      return;
    }

    setState(() => _draftText = '');
    await _nativeComposerKey.currentState?.clearText();
    widget.onSubmit(prompt);
    focus();
  }

  void _handleChanged(String text) {
    if (_draftText == text) {
      return;
    }
    setState(() => _draftText = text);
  }

  @override
  Widget build(BuildContext context) {
    final hasText = _draftText.trim().isNotEmpty;
    final actionLabel = widget.hasSession ? 'Send' : 'Start';
    final actionIcon = widget.hasSession ? Icons.send : Icons.play_arrow;
    final enabled = widget.enabled && !widget.isWorking;

    return Padding(
      padding: const EdgeInsets.all(12),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: context.ditch.surfaceHover,
          borderRadius: BorderRadius.circular(context.ditch.radiusMedium),
        ),
        child: Padding(
          padding: const EdgeInsets.all(10),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Expanded(
                child: SizedBox(
                  height: 72,
                  child: NativeComposerTextView(
                    key: _nativeComposerKey,
                    initialText: widget.initialText,
                    enabled: enabled,
                    placeholder: widget.hasSession
                        ? 'Send a follow-up to Codex'
                        : 'Tell Codex what to do',
                    onChanged: _handleChanged,
                    onSubmitRequested: submit,
                    onEnlarge: widget.onEnlarge,
                    onEscape: widget.onEscape,
                  ),
                ),
              ),
              const SizedBox(width: 10),
              if (widget.isWorking)
                IconButton.filledTonal(
                  onPressed: widget.onStop,
                  tooltip: 'Stop Codex',
                  icon: const Icon(Icons.stop_circle_outlined),
                )
              else
                FilledButton.icon(
                  onPressed: hasText && enabled ? submit : null,
                  icon: Icon(actionIcon),
                  label: Text(actionLabel),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

class NativeComposerTextView extends StatefulWidget {
  const NativeComposerTextView({
    required this.initialText,
    required this.enabled,
    required this.placeholder,
    required this.onChanged,
    required this.onSubmitRequested,
    this.onEnlarge,
    this.onEscape,
    super.key,
  });

  final String initialText;
  final bool enabled;
  final String placeholder;
  final ValueChanged<String> onChanged;
  final VoidCallback onSubmitRequested;
  final VoidCallback? onEnlarge;
  final VoidCallback? onEscape;

  @override
  State<NativeComposerTextView> createState() => NativeComposerTextViewState();
}

class NativeComposerTextViewState extends State<NativeComposerTextView> {
  MethodChannel? _channel;
  bool? _lastSentEnabled;
  bool _focusRequested = false;
  late final TextEditingController _fallbackController;
  late final FocusNode _fallbackFocusNode;

  @override
  void initState() {
    super.initState();
    _fallbackController = TextEditingController(text: widget.initialText);
    _fallbackFocusNode = FocusNode();
  }

  @override
  void didUpdateWidget(NativeComposerTextView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.initialText != widget.initialText &&
        _fallbackController.text.trim().isEmpty) {
      _fallbackController.text = widget.initialText;
    }
    _syncNativeEnabled();
  }

  @override
  void dispose() {
    _fallbackController.dispose();
    _fallbackFocusNode.dispose();
    super.dispose();
  }

  Future<void> focus() async {
    _focusRequested = true;
    final channel = _channel;
    if (kIsWeb || defaultTargetPlatform != TargetPlatform.macOS) {
      _fallbackFocusNode.requestFocus();
      return;
    }
    if (channel == null) return;

    try {
      await channel.invokeMethod<void>('focus');
    } on MissingPluginException {
      return;
    }
  }

  Future<void> blur() async {
    _focusRequested = false;
    _fallbackFocusNode.unfocus();
    final channel = _channel;
    if (channel == null ||
        kIsWeb ||
        defaultTargetPlatform != TargetPlatform.macOS) {
      return;
    }

    try {
      await channel.invokeMethod<void>('blur');
    } on MissingPluginException {
      return;
    }
  }

  Future<String> currentText() async {
    final channel = _channel;
    if (channel == null ||
        kIsWeb ||
        defaultTargetPlatform != TargetPlatform.macOS) {
      return _fallbackController.text;
    }

    return await channel.invokeMethod<String>('getText') ?? '';
  }

  Future<void> clearText() async {
    _fallbackController.clear();
    widget.onChanged('');

    final channel = _channel;
    if (channel == null ||
        kIsWeb ||
        defaultTargetPlatform != TargetPlatform.macOS) {
      return;
    }

    await channel.invokeMethod<void>('clearText');
  }

  Future<void> _syncNativeEnabled() async {
    final channel = _channel;
    if (channel == null ||
        kIsWeb ||
        defaultTargetPlatform != TargetPlatform.macOS) {
      return;
    }

    if (_lastSentEnabled != widget.enabled) {
      _lastSentEnabled = widget.enabled;
      await channel.invokeMethod<void>('setEnabled', widget.enabled);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (kIsWeb || defaultTargetPlatform != TargetPlatform.macOS) {
      return CallbackShortcuts(
        bindings: {
          const SingleActivator(LogicalKeyboardKey.enter):
              widget.onSubmitRequested,
          const SingleActivator(LogicalKeyboardKey.numpadEnter):
              widget.onSubmitRequested,
        },
        child: TextField(
          controller: _fallbackController,
          focusNode: _fallbackFocusNode,
          enabled: widget.enabled,
          minLines: 1,
          maxLines: 5,
          onChanged: widget.onChanged,
          decoration: InputDecoration(
            hintText: widget.placeholder,
            border: InputBorder.none,
            isDense: true,
          ),
        ),
      );
    }

    return AppKitView(
      viewType: 'the_ditch/composer_text_view',
      creationParams: {
        'text': widget.initialText,
        'enabled': widget.enabled,
        'placeholder': widget.placeholder,
        'fontSize': 14.0,
        'escapeEnabled': widget.onEscape != null,
      },
      creationParamsCodec: const StandardMessageCodec(),
      onPlatformViewCreated: (id) {
        final channel = MethodChannel('the_ditch/composer_text_view/$id');
        _channel = channel;
        _lastSentEnabled = widget.enabled;
        channel.setMethodCallHandler((call) async {
          if (call.method == 'textChanged') {
            final text = call.arguments as String? ?? '';
            widget.onChanged(text);
          } else if (call.method == 'enlargeRequested') {
            widget.onEnlarge?.call();
          } else if (call.method == 'escapePressed') {
            widget.onEscape?.call();
          } else if (call.method == 'submitRequested') {
            widget.onSubmitRequested();
          }
        });
        if (_focusRequested) {
          channel.invokeMethod<void>('focus');
        }
      },
    );
  }
}

class AgentChatBubble extends StatelessWidget {
  const AgentChatBubble({
    required this.message,
    required this.chatController,
    required this.agentListController,
    this.embedded = false,
    super.key,
  });

  final AgentChatMessage message;
  final ScrollController chatController;
  final ScrollController agentListController;
  final bool embedded;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final (
      label,
      icon,
      foreground,
      background,
      alignment,
    ) = switch (message.role) {
      ChatMessageRole.user => (
        'You',
        Icons.person_outline,
        colors.onPrimaryContainer,
        colors.primaryContainer,
        Alignment.centerRight,
      ),
      ChatMessageRole.assistant => (
        'Codex',
        Icons.smart_toy_outlined,
        colors.onSurface,
        context.ditch.surfaceHover,
        Alignment.centerLeft,
      ),
      ChatMessageRole.tool => (
        'Tool',
        Icons.terminal,
        colors.onTertiaryContainer,
        colors.tertiaryContainer,
        Alignment.centerLeft,
      ),
      ChatMessageRole.system => (
        'The Ditch',
        Icons.info_outline,
        colors.onSecondaryContainer,
        colors.secondaryContainer,
        Alignment.centerLeft,
      ),
    };

    return LayoutBuilder(
      builder: (context, constraints) {
        final availableWidth = constraints.maxWidth;
        final maxBubbleWidth = availableWidth < 520
            ? availableWidth * 0.86
            : (availableWidth * 0.68).clamp(340.0, 720.0);
        return Align(
          alignment: alignment,
          child: ConstrainedBox(
            constraints: BoxConstraints(maxWidth: maxBubbleWidth),
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: background,
                borderRadius: BorderRadius.circular(context.ditch.radiusMedium),
              ),
              child: Padding(
                padding: const EdgeInsets.all(12),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Icon(icon, size: 16, color: foreground),
                        const SizedBox(width: 6),
                        Text(
                          label,
                          style: Theme.of(context).textTheme.labelMedium
                              ?.copyWith(
                                color: foreground,
                                fontWeight: FontWeight.w700,
                              ),
                        ),
                        const SizedBox(width: 10),
                        IconButton(
                          visualDensity: VisualDensity.compact,
                          tooltip: 'Copy message',
                          icon: const Icon(Icons.copy_outlined, size: 16),
                          onPressed: () => Clipboard.setData(
                            ClipboardData(text: message.text),
                          ),
                        ),
                      ],
                    ),
                    const SizedBox(height: 8),
                    if (message.role == ChatMessageRole.tool)
                      SelectionArea(
                        child: Text(
                          message.text,
                          style: Theme.of(context).textTheme.bodySmall
                              ?.copyWith(
                                color: foreground,
                                height: 1.4,
                                fontFamily: 'SF Mono',
                                fontSize: 12.5,
                              ),
                        ),
                      )
                    else
                      SelectionArea(
                        child: Text(
                          message.text,
                          style: Theme.of(context).textTheme.bodyMedium
                              ?.copyWith(
                                color: foreground,
                                height: 1.45,
                                fontSize: 14,
                              ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class AttentionPanel extends StatelessWidget {
  const AttentionPanel({
    required this.width,
    required this.events,
    required this.canStopSession,
    required this.onOpenSession,
    required this.onStopSession,
    required this.onDismiss,
    super.key,
  });

  final double width;
  final List<AttentionEvent> events;
  final bool Function(AttentionEvent event) canStopSession;
  final ValueChanged<AttentionEvent> onOpenSession;
  final ValueChanged<AttentionEvent> onStopSession;
  final ValueChanged<AttentionEvent> onDismiss;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return ColoredBox(
      color: context.ditch.inspector,
      child: SizedBox(
        width: width,
        child: SafeArea(
          top: false,
          child: SingleChildScrollView(
            padding: const EdgeInsets.all(16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('Attention', style: theme.textTheme.titleLarge),
                const SizedBox(height: 12),
                if (events.isEmpty)
                  const AttentionEmptyState()
                else
                  for (final event in events) ...[
                    AttentionItem(
                      event: event,
                      canStop: canStopSession(event),
                      onOpen: event.canOpenSession
                          ? () => onOpenSession(event)
                          : null,
                      onStop: canStopSession(event)
                          ? () => onStopSession(event)
                          : null,
                      onDismiss: () => onDismiss(event),
                    ),
                    const SizedBox(height: 12),
                  ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class AttentionEmptyState extends StatelessWidget {
  const AttentionEmptyState({super.key});

  @override
  Widget build(BuildContext context) {
    return DitchSurface(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(Icons.check_circle_outline, size: 20),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              'No agent sessions need attention.',
              style: Theme.of(context).textTheme.bodyMedium,
            ),
          ),
        ],
      ),
    );
  }
}

class AttentionItem extends StatelessWidget {
  const AttentionItem({
    required this.event,
    required this.canStop,
    required this.onDismiss,
    this.onOpen,
    this.onStop,
    super.key,
  });

  final AttentionEvent event;
  final bool canStop;
  final VoidCallback? onOpen;
  final VoidCallback? onStop;
  final VoidCallback onDismiss;

  @override
  Widget build(BuildContext context) {
    return DitchSurface(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(event.icon, size: 20),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  event.title,
                  style: Theme.of(context).textTheme.titleSmall,
                ),
                const SizedBox(height: 4),
                Text(event.body),
                const SizedBox(height: 10),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    if (onOpen != null)
                      OutlinedButton.icon(
                        onPressed: onOpen,
                        icon: const Icon(Icons.open_in_full, size: 16),
                        label: const Text('Open'),
                      ),
                    if (canStop && onStop != null)
                      OutlinedButton.icon(
                        onPressed: onStop,
                        icon: const Icon(Icons.stop_circle_outlined, size: 16),
                        label: const Text('Stop'),
                      ),
                    TextButton.icon(
                      onPressed: onDismiss,
                      icon: const Icon(Icons.done, size: 16),
                      label: const Text('Dismiss'),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class AddProjectDialog extends StatefulWidget {
  const AddProjectDialog({super.key});

  @override
  State<AddProjectDialog> createState() => _AddProjectDialogState();
}

class _AddProjectDialogState extends State<AddProjectDialog> {
  static const _projectPickerChannel = MethodChannel(
    'the_ditch/project_picker',
  );
  final _name = TextEditingController();
  final _path = TextEditingController();
  bool _checkingGit = false;
  bool? _isGitRepository;
  ProjectGitPolicy? _gitPolicy;
  bool _showValidation = false;

  Future<void> _browseForFolder() async {
    final path = await _projectPickerChannel.invokeMethod<String>(
      'chooseDirectory',
    );
    if (path == null || path.trim().isEmpty || !mounted) {
      return;
    }
    final normalized = path.trim();
    final segments = Uri.directory(
      normalized,
    ).pathSegments.where((segment) => segment.isNotEmpty).toList();
    setState(() {
      _path.text = normalized;
      _checkingGit = true;
      _isGitRepository = null;
      _gitPolicy = null;
      if (_name.text.trim().isEmpty && segments.isNotEmpty) {
        _name.text = segments.last;
      }
    });
    final isGitRepository = isInsideGitWorkTree(normalized);
    if (!mounted || _path.text != normalized) {
      return;
    }
    setState(() {
      _checkingGit = false;
      _isGitRepository = isGitRepository;
      _gitPolicy = isGitRepository ? ProjectGitPolicy.requireRepository : null;
      _showValidation = !isGitRepository;
    });
  }

  @override
  void dispose() {
    _name.dispose();
    _path.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final canSubmit =
        !_checkingGit &&
        _name.text.trim().isNotEmpty &&
        _path.text.trim().isNotEmpty &&
        _gitPolicy != null;
    return AlertDialog(
      title: const Text('Add Project'),
      content: SizedBox(
        width: 520,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Align(
              alignment: Alignment.centerLeft,
              child: FilledButton.tonalIcon(
                onPressed: _browseForFolder,
                icon: const Icon(Icons.folder_open),
                label: const Text('Browse Folder…'),
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _path,
              decoration: const InputDecoration(labelText: 'Selected folder'),
              readOnly: true,
            ),
            if (_checkingGit) ...[
              const SizedBox(height: 12),
              const LinearProgressIndicator(),
            ] else if (_isGitRepository == false) ...[
              const SizedBox(height: 12),
              DropdownButtonFormField<ProjectGitPolicy>(
                initialValue: _gitPolicy,
                decoration: InputDecoration(
                  labelText: 'This folder is not a Git repository',
                  errorText: _showValidation && _gitPolicy == null
                      ? 'Choose how this project should handle Git.'
                      : null,
                ),
                hint: const Text('Choose how Codex should run'),
                items: const [
                  DropdownMenuItem(
                    value: ProjectGitPolicy.initializeRepository,
                    child: Text('Initialize Git Repository'),
                  ),
                  DropdownMenuItem(
                    value: ProjectGitPolicy.allowOutsideGit,
                    child: Text('Allow Codex Outside Git'),
                  ),
                ],
                onChanged: (value) => setState(() {
                  _gitPolicy = value;
                  _showValidation = false;
                }),
              ),
              const SizedBox(height: 8),
              const Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  'Allowing outside Git applies --skip-git-repo-check only to this project.',
                ),
              ),
            ],
            const SizedBox(height: 12),
            TextField(
              controller: _name,
              decoration: const InputDecoration(labelText: 'Project name'),
              onChanged: (_) => setState(() {}),
            ),
            const SizedBox(height: 16),
            const Align(
              alignment: Alignment.centerLeft,
              child: Text(
                'The Ditch will create and verify:\n.ditch/agents  •  .ditch/hooks  •  .ditch/mcp',
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: !canSubmit
              ? null
              : () {
                  final name = _name.text.trim();
                  final path = _path.text.trim();
                  final gitPolicy = _gitPolicy;
                  if (gitPolicy == null) return;
                  Navigator.of(context).pop(
                    DitchProject(name: name, path: path, gitPolicy: gitPolicy),
                  );
                },
          child: const Text('Add & Configure'),
        ),
      ],
    );
  }
}

class StartCodexSessionDialog extends StatefulWidget {
  const StartCodexSessionDialog({required this.initialPrompt, super.key});

  final String initialPrompt;

  @override
  State<StartCodexSessionDialog> createState() =>
      _StartCodexSessionDialogState();
}

class _StartCodexSessionDialogState extends State<StartCodexSessionDialog> {
  late final TextEditingController _prompt;
  late final FocusNode _promptFocusNode;

  @override
  void initState() {
    super.initState();
    _prompt = TextEditingController(text: widget.initialPrompt);
    _promptFocusNode = FocusNode();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) {
        return;
      }
      _promptFocusNode.requestFocus();
    });
  }

  @override
  void dispose() {
    _prompt.dispose();
    _promptFocusNode.dispose();
    super.dispose();
  }

  void _submit() {
    final prompt = _prompt.text.trim();
    if (prompt.isEmpty) {
      return;
    }
    Navigator.of(context).pop(prompt);
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Start Codex Session'),
      content: SizedBox(
        width: 640,
        child: TextField(
          controller: _prompt,
          focusNode: _promptFocusNode,
          autofocus: true,
          minLines: 4,
          maxLines: 8,
          textInputAction: TextInputAction.newline,
          decoration: const InputDecoration(
            labelText: 'Initial prompt',
            alignLabelWithHint: true,
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton.icon(
          onPressed: _submit,
          icon: const Icon(Icons.play_arrow),
          label: const Text('Start'),
        ),
      ],
    );
  }
}
