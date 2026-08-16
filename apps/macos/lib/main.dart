import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

void main() {
  runApp(const TheDitchApp());
}

class TheDitchApp extends StatelessWidget {
  const TheDitchApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'The Ditch',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xff2563eb),
          brightness: Brightness.light,
        ),
        useMaterial3: true,
        visualDensity: VisualDensity.compact,
      ),
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xff38bdf8),
          brightness: Brightness.dark,
        ),
        useMaterial3: true,
        visualDensity: VisualDensity.compact,
      ),
      home: const CommandCenterScreen(),
    );
  }
}

class DitchProject {
  const DitchProject({required this.name, required this.path});

  final String name;
  final String path;
}

enum AgentProvider { codex }

class AgentSession {
  AgentSession({
    required this.localId,
    required this.provider,
    required this.status,
    required this.messages,
    this.codexThreadId,
    this.currentPrompt,
    DateTime? createdAt,
    DateTime? updatedAt,
  }) : createdAt = createdAt ?? DateTime.now(),
       updatedAt = updatedAt ?? DateTime.now();

  final String localId;
  final AgentProvider provider;
  final DateTime createdAt;
  AgentStatus status;
  String? codexThreadId;
  String? currentPrompt;
  DateTime updatedAt;
  final List<AgentChatMessage> messages;

  String get displayName {
    return switch (provider) {
      AgentProvider.codex => 'Codex',
    };
  }

  bool get hasCodexThread => codexThreadId != null;

  bool get isWorking {
    return status == AgentStatus.starting || status == AgentStatus.working;
  }
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
  });

  final String id;
  final AttentionKind kind;
  final IconData icon;
  final String title;
  final String body;
  final DateTime createdAt;
  final String? sessionLocalId;

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

  bool get isVisibleInChat => false;
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

class CommandCenterScreen extends StatefulWidget {
  const CommandCenterScreen({super.key});

  @override
  State<CommandCenterScreen> createState() => _CommandCenterScreenState();
}

class _CommandCenterScreenState extends State<CommandCenterScreen> {
  static const _defaultStartPrompt =
      'Inspect this project and tell me the next useful engineering step.';

  final _chatController = ScrollController();
  final _composerKey = GlobalKey<AgentComposerState>();
  int _nextAgentSessionId = 1;
  int _nextAttentionId = 1;
  final _projects = <DitchProject>[
    const DitchProject(
      name: 'The Ditch',
      path: '/Users/tester/Documents/Personal/The Ditch v2',
    ),
  ];
  final _attention = <AttentionEvent>[];

  final _codexProcesses = <String, Process>{};
  int _selectedProjectIndex = 0;
  String? _codexBinary;
  String? _expandedAgentLocalId = 'agent-0';
  final _codexDiagnostics = <CodexProcessDiagnostic>[];
  late final List<AgentSession> _agentSessions = [
    AgentSession(
      localId: 'agent-0',
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
    ),
  ];

  DitchProject get _selectedProject => _projects[_selectedProjectIndex];

  AgentSession? get _expandedSession {
    final expandedId = _expandedAgentLocalId;
    if (expandedId == null) {
      return null;
    }
    return _agentSessionByLocalId(expandedId);
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

  @override
  void initState() {
    super.initState();
  }

  @override
  void dispose() {
    for (final process in _codexProcesses.values) {
      process.kill();
    }
    _chatController.dispose();
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

    await _ensureProjectMetadata(project.path);
    setState(() {
      _projects.add(project);
      _selectedProjectIndex = _projects.length - 1;
    });
    _addChatMessage(
      _expandedSession ?? _agentSessions.first,
      ChatMessageRole.system,
      'Added project: ${project.name}',
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

    late final AgentSession session;
    setState(() {
      session = _createAgentSession(expand: true);
    });
    await _beginCodexSession(session, prompt.trim());
  }

  Future<void> _beginCodexSession(AgentSession session, String prompt) async {
    final binary = _codexBinary ?? await _discoverCodexBinary();
    if (binary == null) {
      setState(() {
        session.status = AgentStatus.failed;
        session.updatedAt = DateTime.now();
      });
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Codex not found. Checked GUI PATH, login shell PATH, Homebrew paths, and NVM Node versions.',
      );
      _addAttentionRequired(
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
        icon: Icons.error_outline,
        title: 'Codex not found',
        body:
            'The app could not find the codex binary from the macOS app environment.',
      );
      return;
    }
    _codexBinary = binary;

    setState(() {
      session.codexThreadId = null;
      session.status = AgentStatus.starting;
      session.currentPrompt = prompt;
      session.updatedAt = DateTime.now();
    });
    await _runCodexTurn(session, prompt, resume: false);
  }

  Future<void> _submitComposer(AgentSession session, String prompt) async {
    if (session.isWorking) {
      return;
    }

    final cleanPrompt = prompt.trim();
    if (cleanPrompt.isEmpty) {
      return;
    }

    if (session.codexThreadId == null) {
      await _beginCodexSession(session, cleanPrompt);
      return;
    }

    setState(() {
      session.currentPrompt = cleanPrompt;
      session.updatedAt = DateTime.now();
    });
    await _runCodexTurn(session, cleanPrompt, resume: true);
  }

  Future<void> _stopCodex(AgentSession session) async {
    final process = _codexProcesses[session.localId];
    if (process == null) {
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'No active Codex process to stop.',
      );
      return;
    }

    process.kill();
    _codexProcesses.remove(session.localId);
    _addChatMessage(session, ChatMessageRole.system, 'Stopping Codex...');
    setState(() {
      session.status = AgentStatus.stopped;
      session.updatedAt = DateTime.now();
    });
  }

  Future<void> _runCodexTurn(
    AgentSession session,
    String prompt, {
    required bool resume,
  }) async {
    final binary = _codexBinary ?? await _discoverCodexBinary();
    if (binary == null) {
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Codex binary was not found.',
      );
      setState(() {
        session.status = AgentStatus.failed;
        session.updatedAt = DateTime.now();
      });
      return;
    }
    _codexBinary = binary;

    final args = resume
        ? <String>['exec', 'resume', '--json', session.codexThreadId!, '-']
        : <String>[
            'exec',
            '--json',
            '--color',
            'never',
            '--cd',
            _selectedProject.path,
            '-',
          ];

    _addChatMessage(session, ChatMessageRole.user, prompt);
    setState(() {
      session.status = AgentStatus.working;
      session.currentPrompt = prompt;
      session.updatedAt = DateTime.now();
    });
    final turnDiagnostics = <CodexProcessDiagnostic>[];

    try {
      final process = await Process.start(
        binary,
        args,
        workingDirectory: _selectedProject.path,
        environment: _agentEnvironment(binary),
        mode: ProcessStartMode.normal,
      );
      _codexProcesses[session.localId] = process;
      process.stdin.write(prompt);
      await process.stdin.close();

      process.stdout
          .transform(utf8.decoder)
          .transform(const LineSplitter())
          .listen((line) => _handleCodexJsonEvent(session, line));
      process.stderr.transform(utf8.decoder).listen((text) {
        final diagnostic = codexStderrDiagnosticFromChunk(text);
        if (diagnostic == null) {
          return;
        }
        turnDiagnostics.add(diagnostic);
        _recordCodexDiagnostic(diagnostic);
      });

      final code = await process.exitCode;
      if (!mounted || _codexProcesses[session.localId] != process) {
        return;
      }
      _codexProcesses.remove(session.localId);
      setState(() {
        session.status = code == 0 ? AgentStatus.completed : AgentStatus.failed;
        session.updatedAt = DateTime.now();
      });
      if (code != 0) {
        final diagnosticCount = turnDiagnostics.length;
        final details = diagnosticCount == 0
            ? 'Exit code: $code.'
            : 'Exit code: $code. $diagnosticCount diagnostic message(s) captured.';
        _ring(
          'Codex failed',
          details,
          kind: AttentionKind.failed,
          sessionLocalId: session.localId,
        );
        _addChatMessage(
          session,
          ChatMessageRole.system,
          'Codex exited with code $code.',
        );
      }
    } on Object catch (error) {
      _codexProcesses.remove(session.localId);
      setState(() {
        session.status = AgentStatus.failed;
        session.updatedAt = DateTime.now();
      });
      _ring(
        'Codex failed to start',
        '$error',
        kind: AttentionKind.failed,
        sessionLocalId: session.localId,
      );
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Failed to start Codex: $error',
      );
    }
  }

  Future<String?> _discoverCodexBinary() async {
    final home = Platform.environment['HOME'];
    final staticCandidates = <String>[
      if (home != null) '$home/.nvm/current/bin/codex',
      if (home != null) '$home/.npm-global/bin/codex',
      if (home != null) '$home/.local/bin/codex',
      '/opt/homebrew/bin/codex',
      '/usr/local/bin/codex',
    ];

    for (final candidate in staticCandidates) {
      if (await File(candidate).exists()) {
        return candidate;
      }
    }

    if (home != null) {
      final nvmRoot = Directory('$home/.nvm/versions/node');
      if (await nvmRoot.exists()) {
        final versions = await nvmRoot
            .list()
            .where((entity) => entity is Directory)
            .cast<Directory>()
            .toList();
        versions.sort((a, b) => b.path.compareTo(a.path));
        for (final version in versions) {
          final candidate = '${version.path}/bin/codex';
          if (await File(candidate).exists()) {
            return candidate;
          }
        }
      }
    }

    final shell = Platform.environment['SHELL'] ?? '/bin/zsh';
    final shellResult = await Process.run(shell, ['-lc', 'command -v codex']);
    if (shellResult.exitCode == 0) {
      final path = shellResult.stdout.toString().trim();
      if (path.isNotEmpty && await File(path).exists()) {
        return path;
      }
    }

    final pathResult = await Process.run('/usr/bin/env', ['which', 'codex']);
    if (pathResult.exitCode == 0) {
      final path = pathResult.stdout.toString().trim();
      if (path.isNotEmpty && await File(path).exists()) {
        return path;
      }
    }

    return null;
  }

  Map<String, String> _agentEnvironment(String binary) {
    final currentPath = Platform.environment['PATH'] ?? '';
    final binaryDir = File(binary).parent.path;
    final pathParts = <String>[
      binaryDir,
      '/opt/homebrew/bin',
      '/usr/local/bin',
      '/usr/bin',
      '/bin',
      '/usr/sbin',
      '/sbin',
      if (currentPath.isNotEmpty) currentPath,
    ];

    final home = Platform.environment['HOME'];
    final shell = Platform.environment['SHELL'];

    final environment = {'PATH': pathParts.toSet().join(':')};
    if (home != null) {
      environment['HOME'] = home;
    }
    if (shell != null) {
      environment['SHELL'] = shell;
    }
    return environment;
  }

  Future<void> _ensureProjectMetadata(String projectPath) async {
    for (final child in ['agents', 'hooks', 'mcp']) {
      await Directory('$projectPath/.ditch/$child').create(recursive: true);
    }
  }

  void _ring(
    String title,
    String body, {
    required AttentionKind kind,
    String? sessionLocalId,
  }) {
    SystemSound.play(SystemSoundType.alert);
    _addAttentionRequired(
      kind: kind,
      sessionLocalId: sessionLocalId,
      icon: Icons.notifications_active_outlined,
      title: title,
      body: body,
    );
  }

  void _recordCodexDiagnostic(CodexProcessDiagnostic diagnostic) {
    _codexDiagnostics.add(diagnostic);
    if (_codexDiagnostics.length > 200) {
      _codexDiagnostics.removeRange(0, _codexDiagnostics.length - 200);
    }
  }

  void _addAttentionRequired({
    required AttentionKind kind,
    required IconData icon,
    required String title,
    required String body,
    String? sessionLocalId,
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
          createdAt: DateTime.now(),
        ),
      );
    });
  }

  void _openAttentionSession(AttentionEvent event) {
    final sessionLocalId = event.sessionLocalId;
    if (sessionLocalId == null ||
        _agentSessionByLocalId(sessionLocalId) == null) {
      return;
    }

    setState(() => _expandedAgentLocalId = sessionLocalId);
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
  }

  bool _canStopAttentionSession(AttentionEvent event) {
    final sessionLocalId = event.sessionLocalId;
    if (sessionLocalId == null) {
      return false;
    }
    return _codexProcesses.containsKey(sessionLocalId) &&
        (_agentSessionByLocalId(sessionLocalId)?.isWorking ?? false);
  }

  void _handleCodexJsonEvent(AgentSession session, String line) {
    if (line.trim().isEmpty) {
      return;
    }
    Object? decoded;
    try {
      decoded = jsonDecode(line);
    } on FormatException {
      return;
    }
    if (decoded is! Map<String, dynamic>) {
      return;
    }

    switch (decoded['type']) {
      case 'thread.started':
        final threadId = decoded['thread_id'];
        if (threadId is String && threadId.isNotEmpty) {
          setState(() {
            session.codexThreadId = threadId;
            session.updatedAt = DateTime.now();
          });
        }
        break;
      case 'item.completed':
        final item = decoded['item'];
        if (item is! Map<String, dynamic>) {
          return;
        }
        switch (item['type']) {
          case 'agent_message':
            final text = item['text'];
            if (text is String && text.trim().isNotEmpty) {
              _addChatMessage(session, ChatMessageRole.assistant, text.trim());
            }
            break;
          case 'error':
            final message = item['message'];
            if (message is String && message.trim().isNotEmpty) {
              final cleanMessage = message.trim();
              _addChatMessage(session, ChatMessageRole.system, cleanMessage);
              _addAttentionRequired(
                kind: AttentionKind.failed,
                sessionLocalId: session.localId,
                icon: Icons.error_outline,
                title: '${session.displayName} needs attention',
                body: cleanMessage,
              );
            }
            break;
          case 'command_execution':
            final command = item['command'];
            if (command is String && command.trim().isNotEmpty) {
              _addChatMessage(session, ChatMessageRole.tool, command.trim());
            }
            break;
        }
        break;
      case 'turn.completed':
        setState(() {
          session.status = AgentStatus.completed;
          session.updatedAt = DateTime.now();
        });
        break;
    }
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
      session.messages.add(
        AgentChatMessage(role: role, text: text, createdAt: DateTime.now()),
      );
      session.updatedAt = DateTime.now();
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_chatController.hasClients) {
        return;
      }
      _chatController.animateTo(
        _chatController.position.maxScrollExtent,
        duration: const Duration(milliseconds: 180),
        curve: Curves.easeOut,
      );
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: LayoutBuilder(
        builder: (context, constraints) {
          final content = [
            ProjectSidebar(
              projects: _projects,
              selectedIndex: _selectedProjectIndex,
              onAddProject: _addProject,
              onSelectProject: (index) =>
                  setState(() => _selectedProjectIndex = index),
            ),
            const VerticalDivider(width: 1),
            Expanded(
              child: AgentsSurface(
                sessions: _agentSessions,
                expandedAgentLocalId: _expandedAgentLocalId,
                chatController: _chatController,
                composerKey: _composerKey,
                initialPrompt: _defaultStartPrompt,
                onStartCodex: _startCodex,
                onSubmitPrompt: _submitComposer,
                onStopCodex: _stopCodex,
                onToggleExpanded: (session) {
                  setState(() {
                    _expandedAgentLocalId =
                        _expandedAgentLocalId == session.localId
                        ? null
                        : session.localId;
                  });
                },
              ),
            ),
          ];

          if (constraints.maxWidth < 1000) {
            return Column(
              children: [
                Expanded(child: Row(children: content)),
                const Divider(height: 1),
                SizedBox(
                  height: 240,
                  child: AttentionPanel(
                    width: double.infinity,
                    events: _attention,
                    canStopSession: _canStopAttentionSession,
                    onOpenSession: _openAttentionSession,
                    onStopSession: (event) {
                      final sessionLocalId = event.sessionLocalId;
                      final session = sessionLocalId == null
                          ? null
                          : _agentSessionByLocalId(sessionLocalId);
                      if (session != null) {
                        _stopCodex(session);
                      }
                    },
                    onDismiss: _dismissAttention,
                  ),
                ),
              ],
            );
          }

          return Row(
            children: [
              ...content,
              const VerticalDivider(width: 1),
              AttentionPanel(
                width: 300,
                events: _attention,
                canStopSession: _canStopAttentionSession,
                onOpenSession: _openAttentionSession,
                onStopSession: (event) {
                  final sessionLocalId = event.sessionLocalId;
                  final session = sessionLocalId == null
                      ? null
                      : _agentSessionByLocalId(sessionLocalId);
                  if (session != null) {
                    _stopCodex(session);
                  }
                },
                onDismiss: _dismissAttention,
              ),
            ],
          );
        },
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

    return SizedBox(
      width: 220,
      child: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('The Ditch', style: theme.textTheme.titleLarge),
              const SizedBox(height: 16),
              FilledButton.icon(
                onPressed: onAddProject,
                icon: const Icon(Icons.add),
                label: const Text('Add Project'),
              ),
              const SizedBox(height: 16),
              Text('Projects', style: theme.textTheme.labelLarge),
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
            ],
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
    final colors = Theme.of(context).colorScheme;

    return InkWell(
      borderRadius: BorderRadius.circular(8),
      onTap: onTap,
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: selected ? colors.secondaryContainer : Colors.transparent,
          borderRadius: BorderRadius.circular(8),
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
    required this.chatController,
    required this.composerKey,
    required this.initialPrompt,
    required this.onStartCodex,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    required this.onToggleExpanded,
    super.key,
  });

  final List<AgentSession> sessions;
  final String? expandedAgentLocalId;
  final ScrollController chatController;
  final GlobalKey<AgentComposerState> composerKey;
  final String initialPrompt;
  final VoidCallback onStartCodex;
  final void Function(AgentSession session, String prompt) onSubmitPrompt;
  final ValueChanged<AgentSession> onStopCodex;
  final ValueChanged<AgentSession> onToggleExpanded;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);

    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Wrap(
              spacing: 12,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Text('Agents', style: theme.textTheme.headlineSmall),
                FilledButton.icon(
                  onPressed: onStartCodex,
                  icon: const Icon(Icons.smart_toy_outlined),
                  label: const Text('Start Codex'),
                ),
              ],
            ),
            const SizedBox(height: 16),
            Expanded(
              child: AgentSessionList(
                sessions: sessions,
                expandedAgentLocalId: expandedAgentLocalId,
                chatController: chatController,
                composerKey: composerKey,
                initialPrompt: initialPrompt,
                onToggleExpanded: onToggleExpanded,
                onSubmitPrompt: onSubmitPrompt,
                onStopCodex: onStopCodex,
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
    required this.chatController,
    required this.composerKey,
    required this.initialPrompt,
    required this.onToggleExpanded,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    super.key,
  });

  final List<AgentSession> sessions;
  final String? expandedAgentLocalId;
  final ScrollController chatController;
  final GlobalKey<AgentComposerState> composerKey;
  final String initialPrompt;
  final ValueChanged<AgentSession> onToggleExpanded;
  final void Function(AgentSession session, String prompt) onSubmitPrompt;
  final ValueChanged<AgentSession> onStopCodex;

  @override
  Widget build(BuildContext context) {
    return ListView.separated(
      itemCount: sessions.length,
      separatorBuilder: (_, _) => const SizedBox(height: 12),
      itemBuilder: (context, index) {
        final session = sessions[index];
        final expanded = session.localId == expandedAgentLocalId;
        return ExpandableAgentPanel(
          session: session,
          expanded: expanded,
          chatController: expanded ? chatController : null,
          composerKey: expanded ? composerKey : null,
          initialPrompt: initialPrompt,
          onTap: () => onToggleExpanded(session),
          onSubmitPrompt: (prompt) => onSubmitPrompt(session, prompt),
          onStopCodex: () => onStopCodex(session),
        );
      },
    );
  }
}

class ExpandableAgentPanel extends StatelessWidget {
  const ExpandableAgentPanel({
    required this.session,
    required this.expanded,
    required this.chatController,
    required this.composerKey,
    required this.initialPrompt,
    required this.onTap,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    super.key,
  });

  final AgentSession session;
  final bool expanded;
  final ScrollController? chatController;
  final GlobalKey<AgentComposerState>? composerKey;
  final String initialPrompt;
  final VoidCallback onTap;
  final ValueChanged<String> onSubmitPrompt;
  final VoidCallback onStopCodex;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;

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
                  Text(session.displayName),
                  const SizedBox(height: 2),
                  Text(_statusLabel(session.status)),
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
          ],
        );

        return DecoratedBox(
          decoration: BoxDecoration(
            border: Border.all(color: colors.outlineVariant),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: Column(
              children: [
                InkWell(
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
                if (expanded)
                  Padding(
                    padding: const EdgeInsets.only(top: 12),
                    child: SizedBox(
                      height: 560,
                      child: AgentChatPanel(
                        messages: session.messages,
                        controller: chatController!,
                        composerKey: composerKey!,
                        initialPrompt: session.hasCodexThread
                            ? ''
                            : initialPrompt,
                        hasSession: session.hasCodexThread,
                        isWorking: session.isWorking,
                        onSubmitPrompt: onSubmitPrompt,
                        onStopCodex: onStopCodex,
                      ),
                    ),
                  ),
              ],
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
    required this.composerKey,
    required this.initialPrompt,
    required this.hasSession,
    required this.isWorking,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    super.key,
  });

  final List<AgentChatMessage> messages;
  final ScrollController controller;
  final GlobalKey<AgentComposerState> composerKey;
  final String initialPrompt;
  final bool hasSession;
  final bool isWorking;
  final ValueChanged<String> onSubmitPrompt;
  final VoidCallback onStopCodex;

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;

    return DecoratedBox(
      decoration: BoxDecoration(
        color: colors.surfaceContainerHighest,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        children: [
          Expanded(
            child: Scrollbar(
              controller: controller,
              thumbVisibility: true,
              child: ListView.separated(
                controller: controller,
                padding: const EdgeInsets.all(12),
                itemCount: messages.length,
                separatorBuilder: (_, _) => const SizedBox(height: 10),
                itemBuilder: (context, index) {
                  return AgentChatBubble(message: messages[index]);
                },
              ),
            ),
          ),
          const Divider(height: 1),
          ThinkingStatusStrip(visible: isWorking),
          AgentComposer(
            key: composerKey,
            initialText: initialPrompt,
            hasSession: hasSession,
            isWorking: isWorking,
            onSubmit: onSubmitPrompt,
            onStop: onStopCodex,
          ),
        ],
      ),
    );
  }
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
    required this.onSubmit,
    required this.onStop,
    super.key,
  });

  final String initialText;
  final bool hasSession;
  final bool isWorking;
  final ValueChanged<String> onSubmit;
  final VoidCallback onStop;

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
    if (widget.isWorking) {
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
  }

  void _handleChanged(String text) {
    if (_draftText == text) {
      return;
    }
    setState(() => _draftText = text);
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final hasText = _draftText.trim().isNotEmpty;
    final actionLabel = widget.hasSession ? 'Send' : 'Start';
    final actionIcon = widget.hasSession ? Icons.send : Icons.play_arrow;
    final enabled = !widget.isWorking;

    return Padding(
      padding: const EdgeInsets.all(12),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: colors.surface,
          border: Border.all(color: colors.outlineVariant),
          borderRadius: BorderRadius.circular(8),
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
    super.key,
  });

  final String initialText;
  final bool enabled;
  final String placeholder;
  final ValueChanged<String> onChanged;

  @override
  State<NativeComposerTextView> createState() => NativeComposerTextViewState();
}

class NativeComposerTextViewState extends State<NativeComposerTextView> {
  MethodChannel? _channel;
  bool? _lastSentEnabled;
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
    final channel = _channel;
    if (channel == null ||
        kIsWeb ||
        defaultTargetPlatform != TargetPlatform.macOS) {
      _fallbackFocusNode.requestFocus();
      return;
    }

    try {
      await channel.invokeMethod<void>('focus');
    } on MissingPluginException {
      return;
    }
  }

  Future<void> blur() async {
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
      return TextField(
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
      );
    }

    return AppKitView(
      viewType: 'the_ditch/composer_text_view',
      creationParams: {
        'text': widget.initialText,
        'enabled': widget.enabled,
        'placeholder': widget.placeholder,
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
          }
        });
      },
    );
  }
}

class AgentChatBubble extends StatelessWidget {
  const AgentChatBubble({required this.message, super.key});

  final AgentChatMessage message;

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
        CrossAxisAlignment.end,
      ),
      ChatMessageRole.assistant => (
        'Codex',
        Icons.smart_toy_outlined,
        colors.onSurface,
        colors.surface,
        CrossAxisAlignment.start,
      ),
      ChatMessageRole.tool => (
        'Tool',
        Icons.terminal,
        colors.onTertiaryContainer,
        colors.tertiaryContainer,
        CrossAxisAlignment.start,
      ),
      ChatMessageRole.system => (
        'The Ditch',
        Icons.info_outline,
        colors.onSecondaryContainer,
        colors.secondaryContainer,
        CrossAxisAlignment.start,
      ),
    };

    return Column(
      crossAxisAlignment: alignment,
      children: [
        ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 820),
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: background,
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: colors.outlineVariant),
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
                    ],
                  ),
                  const SizedBox(height: 8),
                  SelectableText(
                    message.text,
                    style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                      color: foreground,
                      height: 1.35,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
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

    return SizedBox(
      width: width,
      child: SafeArea(
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
    );
  }
}

class AttentionEmptyState extends StatelessWidget {
  const AttentionEmptyState({super.key});

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;

    return DecoratedBox(
      decoration: BoxDecoration(
        border: Border.all(color: colors.outlineVariant),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Padding(
        padding: const EdgeInsets.all(12),
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
    final colors = Theme.of(context).colorScheme;

    return DecoratedBox(
      decoration: BoxDecoration(
        border: Border.all(color: colors.outlineVariant),
        borderRadius: BorderRadius.circular(8),
      ),
      child: Padding(
        padding: const EdgeInsets.all(12),
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
                          icon: const Icon(
                            Icons.stop_circle_outlined,
                            size: 16,
                          ),
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
  final _name = TextEditingController();
  final _path = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    _path.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Add Project'),
      content: SizedBox(
        width: 520,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: _name,
              decoration: const InputDecoration(labelText: 'Project name'),
              autofocus: true,
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _path,
              decoration: const InputDecoration(labelText: 'Project path'),
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
          onPressed: () {
            final name = _name.text.trim();
            final path = _path.text.trim();
            if (name.isEmpty || path.isEmpty) {
              return;
            }
            Navigator.of(context).pop(DitchProject(name: name, path: path));
          },
          child: const Text('Add'),
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
