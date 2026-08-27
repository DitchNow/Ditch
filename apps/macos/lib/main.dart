import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show ScrollDirection;
import 'package:flutter/services.dart';
import 'package:xterm/xterm.dart';
import 'package:qr_flutter/qr_flutter.dart';

import 'application/command_center_controller.dart';
import 'data/runtime_models.dart';
import 'data/runtime_transport.dart';
import 'design_system/ditch_theme.dart';

export 'data/runtime_transport.dart'
    show DitchRuntimeException, parseRuntimeResponseLine;

void main() {
  runApp(const TheDitchApp());
}

final ditchThemeMode = ValueNotifier<ThemeMode>(ThemeMode.system);

class TheDitchApp extends StatefulWidget {
  const TheDitchApp({
    this.connectRuntimeOnStart = true,
    this.initialProjects = const [],
    super.key,
  });

  final bool connectRuntimeOnStart;
  final List<DitchProject> initialProjects;

  @override
  State<TheDitchApp> createState() => _TheDitchAppState();
}

class _TheDitchAppState extends State<TheDitchApp> {
  static const _applicationChannel = MethodChannel('the_ditch/application');

  @override
  void initState() {
    super.initState();
    unawaited(_loadThemeMode());
  }

  Future<void> _loadThemeMode() async {
    try {
      final stored = await _applicationChannel.invokeMethod<String>(
        'getThemeMode',
      );
      ditchThemeMode.value = switch (stored) {
        'light' => ThemeMode.light,
        'dark' => ThemeMode.dark,
        _ => ThemeMode.system,
      };
    } on MissingPluginException {
      // Widget tests and non-macOS hosts use the system default.
    }
  }

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<ThemeMode>(
      valueListenable: ditchThemeMode,
      builder: (context, themeMode, _) => MaterialApp(
        title: 'Ditch',
        debugShowCheckedModeBanner: false,
        theme: DitchTheme.light(),
        darkTheme: DitchTheme.dark(),
        themeMode: themeMode,
        home: CommandCenterScreen(
          connectRuntimeOnStart: widget.connectRuntimeOnStart,
          initialProjects: widget.initialProjects,
        ),
      ),
    );
  }
}

Future<void> setDitchThemeMode(ThemeMode mode) async {
  ditchThemeMode.value = mode;
  try {
    await const MethodChannel('the_ditch/application').invokeMethod<void>(
      'setThemeMode',
      switch (mode) {
        ThemeMode.light => 'light',
        ThemeMode.dark => 'dark',
        ThemeMode.system => 'system',
      },
    );
  } on MissingPluginException {
    // Tests and non-macOS hosts have no persistence channel.
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

enum TerminalPresentation { docked, horizontal, vertical, maximized }

enum WorkspaceToolKind { terminal, editor }

enum ProjectFileEntryKind { directory, file, symlink }

class ProjectFileEntry {
  const ProjectFileEntry({
    required this.name,
    required this.relativePath,
    required this.kind,
    required this.size,
  });

  final String name;
  final String relativePath;
  final ProjectFileEntryKind kind;
  final int size;

  bool get isDirectory => kind == ProjectFileEntryKind.directory;
  bool get isFile => kind == ProjectFileEntryKind.file;
}

class ProjectEditorDocument {
  ProjectEditorDocument({
    required this.relativePath,
    required String content,
    required this.revision,
  }) : originalContent = content,
       controller = TextEditingController(text: content);

  final String relativePath;
  final TextEditingController controller;
  String originalContent;
  String revision;
  bool saving = false;
  bool conflict = false;
  String? error;

  bool get dirty => controller.text != originalContent;

  void dispose() => controller.dispose();
}

class ProjectFilesState {
  final Map<String, List<ProjectFileEntry>> directories = {};
  final Set<String> expandedDirectories = {};
  final Set<String> loadingDirectories = {};
  String? error;
  ProjectEditorDocument? document;

  void dispose() => document?.dispose();
}

enum AgentApprovalPreset { ask, approveForMe, fullAccess }

class AgentModelOption {
  const AgentModelOption({
    required this.id,
    required this.displayName,
    this.isDefault = false,
    this.contextWindowTokens,
  });
  final String id;
  final String displayName;
  final bool isDefault;
  final int? contextWindowTokens;
}

class CodexInstallationOption {
  const CodexInstallationOption({
    required this.path,
    required this.version,
    required this.selected,
  });

  final String path;
  final String version;
  final bool selected;
}

class CodexReadinessReport {
  const CodexReadinessReport({
    required this.path,
    required this.version,
    required this.compatible,
    required this.authenticated,
    required this.updateSupported,
    required this.doctorSupported,
    required this.issues,
    required this.diagnostics,
  });

  factory CodexReadinessReport.fromResponse(Map<String, dynamic> response) {
    final value = response['CodexReadiness'];
    if (value is! Map) {
      throw const FormatException('Runtime returned invalid Codex readiness.');
    }
    final rawIssues = value['issues'];
    return CodexReadinessReport(
      path: value['path']?.toString(),
      version: value['version']?.toString(),
      compatible: value['compatible'] == true,
      authenticated: value['authenticated'] == true,
      updateSupported: value['update_supported'] == true,
      doctorSupported: value['doctor_supported'] == true,
      issues: rawIssues is List
          ? rawIssues.map((item) => item.toString()).toList()
          : const [],
      diagnostics: value['diagnostics']?.toString(),
    );
  }

  final String? path;
  final String? version;
  final bool compatible;
  final bool authenticated;
  final bool updateSupported;
  final bool doctorSupported;
  final List<String> issues;
  final String? diagnostics;

  bool get ready => path != null && compatible && authenticated;
}

enum NotificationAuthorizationState {
  notDetermined,
  denied,
  authorized,
  provisional,
  ephemeral,
  unknown,
}

class NotificationReadiness {
  const NotificationReadiness({
    required this.authorization,
    required this.alertsEnabled,
    required this.notificationCenterEnabled,
    required this.soundsEnabled,
  });

  factory NotificationReadiness.fromMap(Map<Object?, Object?> value) {
    final authorization = switch (value['authorizationStatus']?.toString()) {
      'notDetermined' => NotificationAuthorizationState.notDetermined,
      'denied' => NotificationAuthorizationState.denied,
      'authorized' => NotificationAuthorizationState.authorized,
      'provisional' => NotificationAuthorizationState.provisional,
      'ephemeral' => NotificationAuthorizationState.ephemeral,
      _ => NotificationAuthorizationState.unknown,
    };
    return NotificationReadiness(
      authorization: authorization,
      alertsEnabled: value['alertsEnabled'] == true,
      notificationCenterEnabled: value['notificationCenterEnabled'] == true,
      soundsEnabled: value['soundsEnabled'] == true,
    );
  }

  final NotificationAuthorizationState authorization;
  final bool alertsEnabled;
  final bool notificationCenterEnabled;
  final bool soundsEnabled;

  bool get authorized => switch (authorization) {
    NotificationAuthorizationState.authorized ||
    NotificationAuthorizationState.provisional ||
    NotificationAuthorizationState.ephemeral => true,
    _ => false,
  };

  bool get ready => authorized && (alertsEnabled || notificationCenterEnabled);
}

class AgentExecutionSettings extends ChangeNotifier {
  AgentApprovalPreset approval = AgentApprovalPreset.approveForMe;
  String? model;
  List<AgentModelOption> models = const [];

  Map<String, dynamic> get protocolValue => {
    'model': model,
    'reasoning_effort': null,
    'approval': switch (approval) {
      AgentApprovalPreset.ask => 'Ask',
      AgentApprovalPreset.approveForMe => 'ApproveForMe',
      AgentApprovalPreset.fullAccess => 'FullAccess',
    },
  };

  void setApproval(AgentApprovalPreset value) {
    approval = value;
    notifyListeners();
  }

  void setModel(String? value) {
    model = value;
    notifyListeners();
  }

  AgentModelOption? get selectedModel =>
      models.where((item) => item.id == model).firstOrNull;

  void setModels(List<AgentModelOption> value) {
    models = value;
    if (model == null) {
      for (final item in value) {
        if (item.isDefault) {
          model = item.id;
          break;
        }
      }
    }
    notifyListeners();
  }
}

final agentExecutionSettings = AgentExecutionSettings();

class ProjectTerminalSession {
  ProjectTerminalSession({
    required this.id,
    required this.projectId,
    required this.shell,
  });

  final String id;
  final String projectId;
  final String shell;
  final Terminal terminal = Terminal(
    maxLines: 10000,
    platform: TerminalTargetPlatform.macos,
  );
}

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
    this.canStop = false,
    this.messagesLoaded = true,
    this.messagesLoading = false,
    this.hasOlderMessages = false,
    this.nextBeforeSequence,
    this.historyError,
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
  bool canStop;
  DateTime updatedAt;
  final List<AgentChatMessage> messages;
  bool messagesLoaded;
  bool messagesLoading;
  bool hasOlderMessages;
  int? nextBeforeSequence;
  String? historyError;

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

  bool get isActive => isWorking || status == AgentStatus.stopping || canStop;
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
  existing.canStop = incoming.canStop;
  existing.updatedAt = incoming.updatedAt;
  sessions.removeWhere(
    (session) =>
        session.localId == incoming.localId && !identical(session, existing),
  );
}

List<AgentChatMessage> uniqueRuntimeMessages(
  Iterable<AgentChatMessage> existing,
  Iterable<AgentChatMessage> incoming,
) {
  final identities = existing.map((message) => message.identity).toSet();
  final content = existing.map(agentMessageContentKey).toSet();
  return incoming
      .where(
        (message) =>
            !identities.contains(message.identity) &&
            !content.contains(agentMessageContentKey(message)),
      )
      .toList();
}

String agentMessageContentKey(AgentChatMessage message) =>
    '${message.role.name}\u0000${message.createdAt.toUtc().microsecondsSinceEpoch}\u0000${message.text}';

enum AttentionKind { approvalRequired, blocked, completed, failed, needsInput }

class AgentNotificationTarget {
  const AgentNotificationTarget({
    required this.projectId,
    required this.agentId,
    this.attentionId,
  });

  final String projectId;
  final String agentId;
  final String? attentionId;

  static AgentNotificationTarget? fromArguments(Object? arguments) {
    if (arguments is! Map) return null;
    final projectId = arguments['projectId']?.toString();
    final agentId = arguments['agentId']?.toString();
    if (projectId == null ||
        projectId.isEmpty ||
        agentId == null ||
        agentId.isEmpty) {
      return null;
    }
    return AgentNotificationTarget(
      projectId: projectId,
      agentId: agentId,
      attentionId: arguments['attentionId']?.toString(),
    );
  }
}

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
    this.projectName,
    this.agentName,
    this.isRead = false,
  });

  final String id;
  final AttentionKind kind;
  final IconData icon;
  final String title;
  final String body;
  final DateTime createdAt;
  final String? sessionLocalId;
  final String? projectId;
  final String? projectName;
  final String? agentName;
  final bool isRead;

  bool get canOpenSession => sessionLocalId != null;
}

enum AgentStatus {
  idle,
  starting,
  working,
  stopping,
  completed,
  failed,
  stopped,
}

class ProjectAgentSummary {
  const ProjectAgentSummary({
    required this.runningCount,
    required this.stoppedCount,
    required this.hasUnreadResult,
  });

  final int runningCount;
  final int stoppedCount;
  final bool hasUnreadResult;
}

ProjectAgentSummary summarizeProjectAgents({
  required Iterable<AgentSession> sessions,
  required Iterable<AttentionEvent> attention,
  required String? projectId,
  Set<String> readAttentionIds = const {},
}) {
  final projectSessions = sessions.where(
    (session) => session.projectId == projectId,
  );
  return ProjectAgentSummary(
    runningCount: projectSessions.where((session) => session.isActive).length,
    stoppedCount: projectSessions.where((session) {
      return session.status == AgentStatus.completed ||
          session.status == AgentStatus.failed ||
          session.status == AgentStatus.stopped;
    }).length,
    hasUnreadResult: attention.any(
      (event) =>
          event.projectId == projectId &&
          !event.isRead &&
          !readAttentionIds.contains(event.id) &&
          (event.kind == AttentionKind.completed ||
              event.kind == AttentionKind.failed),
    ),
  );
}

Set<String> unreadResultAttentionIdsForAgent({
  required Iterable<AttentionEvent> attention,
  required String agentId,
  Set<String> readAttentionIds = const {},
}) {
  return attention
      .where(
        (event) =>
            event.sessionLocalId == agentId &&
            !event.isRead &&
            !readAttentionIds.contains(event.id) &&
            (event.kind == AttentionKind.completed ||
                event.kind == AttentionKind.failed),
      )
      .map((event) => event.id)
      .toSet();
}

enum ChatMessageRole { user, assistant, system, tool }

class AgentChatMessage {
  AgentChatMessage({
    required this.role,
    required this.text,
    required this.createdAt,
    String? identity,
  }) : identity = identity ?? 'local-message-${_nextChatMessageIdentity++}';

  final String identity;
  final ChatMessageRole role;
  final String text;
  final DateTime createdAt;
}

int _nextChatMessageIdentity = 1;

class ConversationItem {
  ConversationItem.message(AgentChatMessage message)
    : this._(
        identity: message.identity,
        message: message,
        toolMessages: const [],
        isActiveToolGroup: false,
      );

  const ConversationItem.toolGroup({
    required String identity,
    required List<AgentChatMessage> toolMessages,
    required bool isActiveToolGroup,
  }) : this._(
         identity: identity,
         message: null,
         toolMessages: toolMessages,
         isActiveToolGroup: isActiveToolGroup,
       );

  const ConversationItem._({
    required this.identity,
    required this.message,
    required this.toolMessages,
    required this.isActiveToolGroup,
  });

  final String identity;
  final AgentChatMessage? message;
  final List<AgentChatMessage> toolMessages;
  final bool isActiveToolGroup;

  bool get isToolGroup => toolMessages.isNotEmpty;
}

List<ConversationItem> buildConversationItems(
  List<AgentChatMessage> messages, {
  required bool isWorking,
}) {
  final items = <ConversationItem>[];
  var segmentStart = 0;
  while (segmentStart < messages.length) {
    var segmentEnd = segmentStart + 1;
    while (segmentEnd < messages.length &&
        messages[segmentEnd].role != ChatMessageRole.user) {
      segmentEnd += 1;
    }
    final tools = messages
        .sublist(segmentStart, segmentEnd)
        .where((message) => message.role == ChatMessageRole.tool)
        .toList(growable: false);
    var emittedTools = false;
    for (var index = segmentStart; index < segmentEnd; index++) {
      final message = messages[index];
      if (message.role == ChatMessageRole.tool) {
        if (!emittedTools) {
          items.add(
            ConversationItem.toolGroup(
              identity: 'tool-activity:${tools.first.identity}',
              toolMessages: tools,
              isActiveToolGroup: isWorking && segmentEnd == messages.length,
            ),
          );
          emittedTools = true;
        }
      } else {
        items.add(ConversationItem.message(message));
      }
    }
    segmentStart = segmentEnd;
  }
  return items;
}

enum ConversationViewportMode { initializing, following, detached }

class ConversationViewportController extends ChangeNotifier {
  ConversationViewportController({ScrollController? scrollController})
    : scrollController = scrollController ?? ScrollController();

  static const nearLatestThreshold = 72.0;
  static const _arrivalDuration = Duration(milliseconds: 180);

  final ScrollController scrollController;
  ConversationViewportMode _mode = ConversationViewportMode.initializing;
  final Set<String> _knownItemIds = {};
  List<String> _orderedItemIds = const [];
  final Set<String> _unseenItemIds = {};
  bool _frameScheduled = false;
  bool _disposed = false;
  int _openingGeneration = 0;
  VoidCallback? _onInitialPositioned;

  ConversationViewportMode get mode => _mode;
  int get unseenCount => _unseenItemIds.length;
  bool get isDetached => _mode == ConversationViewportMode.detached;

  // The transcript is reversed. All coordinate assumptions stay here:
  // visual latest = minScrollExtent; visual oldest = maxScrollExtent.
  double get _distanceFromLatest {
    if (!scrollController.hasClients) return 0;
    final position = scrollController.position;
    return position.pixels - position.minScrollExtent;
  }

  bool get isNearLatest =>
      !scrollController.hasClients ||
      _distanceFromLatest <= nearLatestThreshold;

  bool get isNearOldest {
    if (!scrollController.hasClients) return false;
    final position = scrollController.position;
    return position.maxScrollExtent - position.pixels <= nearLatestThreshold;
  }

  void beginOpening({VoidCallback? onInitialPositioned}) {
    _openingGeneration += 1;
    _mode = ConversationViewportMode.initializing;
    _knownItemIds.clear();
    _orderedItemIds = const [];
    _unseenItemIds.clear();
    _onInitialPositioned = onInitialPositioned;
    _scheduleLatest(immediate: true, generation: _openingGeneration);
    notifyListeners();
  }

  void attach({VoidCallback? onInitialPositioned}) {
    if (_mode == ConversationViewportMode.initializing) {
      _onInitialPositioned ??= onInitialPositioned;
      _scheduleLatest(immediate: true, generation: _openingGeneration);
    } else if (_mode == ConversationViewportMode.following) {
      _scheduleLatest(immediate: true, generation: _openingGeneration);
    }
  }

  void synchronizeItems(List<String> itemIds) {
    if (_disposed) return;
    if (_mode == ConversationViewportMode.initializing) {
      _knownItemIds
        ..clear()
        ..addAll(itemIds);
      _orderedItemIds = List.of(itemIds);
      _scheduleLatest(immediate: true, generation: _openingGeneration);
      return;
    }

    final previousOrder = _orderedItemIds;
    final added = itemIds.where((id) => !_knownItemIds.contains(id)).toList();
    final olderHistoryOnly =
        added.isNotEmpty &&
        itemIds.length >= previousOrder.length &&
        _listSuffixEquals(itemIds, previousOrder);
    _knownItemIds
      ..clear()
      ..addAll(itemIds);
    _orderedItemIds = List.of(itemIds);
    if (added.isEmpty) {
      if (_mode == ConversationViewportMode.following) {
        _scheduleLatest(immediate: true, generation: _openingGeneration);
      }
      return;
    }
    if (olderHistoryOnly) return;

    if (_mode == ConversationViewportMode.detached) {
      _preserveDetachedAnchor(added);
    } else {
      _scheduleLatest(immediate: true, generation: _openingGeneration);
    }
  }

  bool _listSuffixEquals(List<String> values, List<String> suffix) {
    if (suffix.length > values.length) return false;
    final start = values.length - suffix.length;
    for (var index = 0; index < suffix.length; index++) {
      if (values[start + index] != suffix[index]) return false;
    }
    return true;
  }

  bool handleScrollNotification(ScrollNotification notification) {
    if (_disposed || !scrollController.hasClients) return false;
    final userDriven = switch (notification) {
      ScrollStartNotification(:final dragDetails) => dragDetails != null,
      ScrollUpdateNotification(:final dragDetails) => dragDetails != null,
      UserScrollNotification(:final direction) =>
        direction != ScrollDirection.idle,
      _ => false,
    };

    if (isNearLatest) {
      _setFollowing();
    } else if (userDriven && _mode != ConversationViewportMode.detached) {
      _mode = ConversationViewportMode.detached;
      notifyListeners();
    }
    return false;
  }

  void showLatest() {
    if (_disposed) return;
    _mode = ConversationViewportMode.following;
    _unseenItemIds.clear();
    notifyListeners();
    if (!scrollController.hasClients) {
      _scheduleLatest(immediate: true, generation: _openingGeneration);
      return;
    }
    final position = scrollController.position;
    final distance = _distanceFromLatest;
    if (distance > position.viewportDimension * 4) {
      scrollController.jumpTo(position.minScrollExtent);
    } else {
      unawaited(
        scrollController.animateTo(
          position.minScrollExtent,
          duration: _arrivalDuration,
          curve: Curves.easeOut,
        ),
      );
    }
  }

  void _setFollowing() {
    if (_mode == ConversationViewportMode.following && _unseenItemIds.isEmpty) {
      return;
    }
    _mode = ConversationViewportMode.following;
    _unseenItemIds.clear();
    notifyListeners();
  }

  void _preserveDetachedAnchor(List<String> added) {
    if (!scrollController.hasClients) {
      _unseenItemIds.addAll(added);
      notifyListeners();
      return;
    }
    final position = scrollController.position;
    final oldPixels = position.pixels;
    final oldMaxExtent = position.maxScrollExtent;
    _unseenItemIds.addAll(added);
    notifyListeners();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_disposed || !scrollController.hasClients) return;
      final current = scrollController.position;
      final extentGrowth = current.maxScrollExtent - oldMaxExtent;
      if (extentGrowth <= 0) return;
      scrollController.jumpTo(
        (oldPixels + extentGrowth).clamp(
          current.minScrollExtent,
          current.maxScrollExtent,
        ),
      );
    });
  }

  void _scheduleLatest({required bool immediate, required int generation}) {
    if (_disposed || _frameScheduled) return;
    _frameScheduled = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _frameScheduled = false;
      if (_disposed || generation != _openingGeneration) return;
      if (!scrollController.hasClients) return;
      final position = scrollController.position;
      scrollController.jumpTo(position.minScrollExtent);
      if (_mode == ConversationViewportMode.initializing) {
        _mode = ConversationViewportMode.following;
        final callback = _onInitialPositioned;
        _onInitialPositioned = null;
        notifyListeners();
        callback?.call();
      }
    });
  }

  @override
  void dispose() {
    _disposed = true;
    _openingGeneration += 1;
    _onInitialPositioned = null;
    scrollController.dispose();
    super.dispose();
  }
}

List<AgentSession> sessionsForProject(
  Iterable<AgentSession> sessions,
  String? projectId,
) {
  return sessions.where((session) => session.projectId == projectId).toList();
}

void _ignoreAgentSession(AgentSession _) {}
void _ignoreCallback() {}
bool _neverUnreadAgentResult(AgentSession _) => false;

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

  Future<Map<String, dynamic>> listAgentMessages({
    required String agentId,
    int? beforeSequence,
    int limit = 100,
  }) {
    return request({
      'ListAgentMessages': {
        'agent_id': agentId,
        'before_sequence': beforeSequence,
        'limit': limit,
      },
    });
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

  Future<Map<String, dynamic>> deleteProject(String projectId) {
    return request({
      'DeleteProject': {'project_id': projectId},
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
        'execution_profile': agentExecutionSettings.protocolValue,
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
        'execution_profile': agentExecutionSettings.protocolValue,
      },
    });
  }

  Future<Map<String, dynamic>> promptAgent({
    required String agentId,
    required String prompt,
  }) {
    return request({
      'PromptAgent': {
        'agent_id': agentId,
        'prompt': prompt,
        'execution_profile': agentExecutionSettings.protocolValue,
      },
    });
  }

  Future<List<AgentModelOption>> listCodexModels() async {
    final response = await request({
      'ListAgentModels': {'provider': 'Codex'},
    });
    final values = response['AgentModels'];
    if (values is! List) return const [];
    return values.whereType<Map>().map((value) {
      return AgentModelOption(
        id: value['id'].toString(),
        displayName:
            value['display_name']?.toString() ?? value['id'].toString(),
        isDefault: value['is_default'] == true,
        contextWindowTokens: value['context_window_tokens'] is int
            ? value['context_window_tokens'] as int
            : int.tryParse(value['context_window_tokens']?.toString() ?? ''),
      );
    }).toList();
  }

  Future<List<CodexInstallationOption>> discoverCodexInstallations() async {
    final response = await request('DiscoverCodexInstallations');
    final values = response['CodexInstallations'];
    if (values is! List) return const [];
    return values
        .whereType<Map>()
        .map((value) {
          return CodexInstallationOption(
            path: value['path']?.toString() ?? '',
            version: value['version']?.toString() ?? 'Unknown version',
            selected: value['selected'] == true,
          );
        })
        .where((installation) => installation.path.isNotEmpty)
        .toList();
  }

  Future<CodexReadinessReport> checkCodexReadiness() async {
    return CodexReadinessReport.fromResponse(
      await request('CheckCodexReadiness'),
    );
  }

  Future<CodexReadinessReport> updateSelectedCodex() async {
    return CodexReadinessReport.fromResponse(
      await request('UpdateSelectedCodex'),
    );
  }

  Future<Map<String, dynamic>> selectCodexBinary(String path) {
    return request({
      'SelectCodexBinary': {'path': path},
    });
  }

  Future<Map<String, dynamic>> openProjectTerminal({
    required String projectId,
    required int columns,
    required int rows,
  }) => request({
    'OpenProjectTerminal': {
      'project_id': projectId,
      'columns': columns,
      'rows': rows,
    },
  });

  Future<void> writeProjectTerminal(String terminalId, List<int> data) async {
    await request({
      'WriteProjectTerminal': {'terminal_id': terminalId, 'data': data},
    });
  }

  Future<void> resizeProjectTerminal(
    String terminalId,
    int columns,
    int rows,
  ) async {
    await request({
      'ResizeProjectTerminal': {
        'terminal_id': terminalId,
        'columns': columns,
        'rows': rows,
      },
    });
  }

  Future<void> closeProjectTerminal(String terminalId) async {
    await request({
      'CloseProjectTerminal': {'terminal_id': terminalId},
    });
  }

  Future<Map<String, dynamic>> listProjectDirectory({
    required String projectId,
    required String relativePath,
  }) => request({
    'ListProjectDirectory': {
      'project_id': projectId,
      'relative_path': relativePath,
    },
  });

  Future<Map<String, dynamic>> readProjectFile({
    required String projectId,
    required String relativePath,
  }) => request({
    'ReadProjectFile': {'project_id': projectId, 'relative_path': relativePath},
  });

  Future<Map<String, dynamic>> writeProjectFile({
    required String projectId,
    required String relativePath,
    required String? expectedRevision,
    required String content,
  }) => request({
    'WriteProjectFile': {
      'project_id': projectId,
      'relative_path': relativePath,
      'expected_revision': expectedRevision,
      'content': content,
    },
  });

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

  Future<Map<String, dynamic>> markAllAttentionRead() {
    return request('MarkAllAttentionRead');
  }

  Future<Map<String, dynamic>> markAttentionRead(List<String> attentionIds) {
    return request({
      'MarkAttentionRead': {'attention_ids': attentionIds},
    });
  }

  Future<Map<String, dynamic>> remoteControlStatus() =>
      request('RemoteControlStatus');

  Future<Map<String, dynamic>> createRemotePairing() =>
      request('CreateRemotePairing');

  Future<Map<String, dynamic>> getRemotePairing(String pairingId) => request({
    'GetRemotePairing': {'pairing_id': pairingId},
  });

  Future<Map<String, dynamic>> confirmRemotePairing(String pairingId) =>
      request({
        'ConfirmRemotePairing': {'pairing_id': pairingId},
      });

  Future<Map<String, dynamic>> cancelRemotePairing(String pairingId) =>
      request({
        'CancelRemotePairing': {'pairing_id': pairingId},
      });

  Future<Map<String, dynamic>> revokeRemoteDevice(String deviceId) => request({
    'RevokeRemoteDevice': {'device_id': deviceId},
  });

  Future<Map<String, dynamic>> disableRemoteControl() =>
      request('DisableRemoteControl');
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
  const CommandCenterScreen({
    this.connectRuntimeOnStart = true,
    this.initialProjects = const [],
    super.key,
  });

  final bool connectRuntimeOnStart;
  final List<DitchProject> initialProjects;

  @override
  State<CommandCenterScreen> createState() => _CommandCenterScreenState();
}

class _CommandCenterScreenState extends State<CommandCenterScreen> {
  static const _defaultProjectSidebarWidth = 240.0;
  static const _defaultInspectorWidth = 320.0;
  static const _minimumProjectSidebarWidth = 160.0;
  static const _minimumInspectorWidth = 220.0;
  static const _minimumAgentsWidth = 320.0;
  static const _splitterExtent = 9.0;
  static const _defaultStartPrompt = '';
  static const _applicationChannel = MethodChannel('the_ditch/application');

  final _idleChatViewport = ConversationViewportController();
  final _chatViewports = <String, ConversationViewportController>{};
  final _agentListController = ScrollController();
  final _idleComposerKey = GlobalKey<AgentComposerState>();
  final _composerKeys = <String, GlobalKey<AgentComposerState>>{};
  int _composerFocusGeneration = 0;
  final _runtimeClient = DitchRuntimeClient();
  final _presentation = CommandCenterController();
  int _nextAgentSessionId = 1;
  int _nextAttentionId = 1;
  late final List<DitchProject> _projects;
  final _attention = <AttentionEvent>[];
  final _readAttentionIds = <String>{};
  final Map<String, ProjectTerminalSession> _projectTerminals = {};
  final Map<String, ProjectFilesState> _projectFiles = {};

  StreamSubscription<Map<String, dynamic>>? _runtimeEvents;
  String? _runtimeInstanceId;
  bool _runtimeReconnectScheduled = false;
  bool _runtimeHomeMismatchReported = false;
  bool _runtimeCodexHomeMatches = true;
  bool _runtimeSupportsPersistence = true;
  bool _runtimeSupportsAlwaysOnWebAccess = true;
  bool _runtimeCompatibilityReported = false;
  String? _effectiveRuntimeCodexHome;
  String? _codexBinary;
  CodexReadinessReport? _codexReadiness;
  bool _codexSetupChecking = false;
  bool _codexSetupUpdating = false;
  bool _onboardingIntroduced = false;
  String? _codexSetupError;
  NotificationReadiness? _notificationReadiness;
  bool _notificationSetupChecking = false;
  String? _notificationSetupError;
  TerminalPresentation _terminalPresentation = TerminalPresentation.docked;
  TerminalPresentation _terminalRestorePresentation =
      TerminalPresentation.docked;
  WorkspaceToolKind _presentedTool = WorkspaceToolKind.terminal;
  bool _dockedTerminalExpanded = true;
  bool _dockedFilesExpanded = true;
  double _projectSidebarWidth = _defaultProjectSidebarWidth;
  double _inspectorWidth = _defaultInspectorWidth;
  String? _selectedProjectKey;
  String? _expandedAgentLocalId;
  String? _focusedAgentLocalId;
  AgentNotificationTarget? _pendingNotificationTarget;
  bool _runtimeSnapshotHydrated = false;
  final List<AgentSession> _agentSessions = [];

  ConversationViewportController get _chatViewport {
    final agentId = _focusedAgentLocalId ?? _expandedAgentLocalId;
    if (agentId == null) return _idleChatViewport;
    return _chatViewports.putIfAbsent(
      agentId,
      () => ConversationViewportController()..beginOpening(),
    );
  }

  String _projectKey(DitchProject project) =>
      project.id ?? canonicalProjectPath(project.path);

  int get _selectedProjectIndex {
    if (_projects.isEmpty) return -1;
    final index = _projects.indexWhere(
      (project) => _projectKey(project) == _selectedProjectKey,
    );
    return index < 0 ? 0 : index;
  }

  DitchProject? get _selectedProjectOrNull =>
      _projects.isEmpty ? null : _projects[_selectedProjectIndex];

  DitchProject get _selectedProject => _selectedProjectOrNull!;

  List<AgentSession> get _visibleSessions {
    return sessionsForProject(_agentSessions, _selectedProjectOrNull?.id);
  }

  int get _unreadNotificationCount =>
      _attention.where((event) => !_readAttentionIds.contains(event.id)).length;

  Key _agentHeaderKey(String agentId) => ValueKey('agent-header-$agentId');

  GlobalKey<AgentComposerState> _composerKeyForAgent(String agentId) =>
      _composerKeys.putIfAbsent(
        agentId,
        () => GlobalKey<AgentComposerState>(
          debugLabel: 'agent-composer-$agentId',
        ),
      );

  GlobalKey<AgentComposerState> get _activeComposerKey {
    final agentId = _focusedAgentLocalId ?? _expandedAgentLocalId;
    return agentId == null ? _idleComposerKey : _composerKeyForAgent(agentId);
  }

  Future<void> _selectProject(int index) async {
    final currentProjectId = _selectedProjectOrNull?.id;
    final currentFiles = currentProjectId == null
        ? null
        : _projectFiles[currentProjectId];
    if (currentFiles != null && !await _confirmDiscardEditor(currentFiles)) {
      return;
    }
    if (currentFiles?.document?.dirty == true) {
      currentFiles!.document!.dispose();
      currentFiles.document = null;
    }
    if (!mounted) return;
    setState(() {
      _selectedProjectKey = _projectKey(_projects[index]);
      _expandedAgentLocalId = null;
      _focusedAgentLocalId = null;
      _terminalPresentation = TerminalPresentation.docked;
      _presentedTool = WorkspaceToolKind.terminal;
      _dockedTerminalExpanded = true;
    });
    unawaited(_ensureSelectedProjectTerminal());
  }

  Future<void> _revealProjectInFinder(DitchProject project) async {
    try {
      final revealed =
          await _applicationChannel.invokeMethod<bool>(
            'revealInFinder',
            project.path,
          ) ??
          false;
      if (!revealed && mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Could not reveal ${project.name} in Finder.'),
          ),
        );
      }
    } on MissingPluginException {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Finder integration is unavailable.')),
        );
      }
    } on PlatformException {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Could not reveal ${project.name} in Finder.'),
          ),
        );
      }
    }
  }

  Future<void> _revealProjectEntry(ProjectFileEntry entry) async {
    final path = '${_selectedProject.path}/${entry.relativePath}';
    try {
      await _applicationChannel.invokeMethod<bool>('revealInFinder', path);
    } on Object {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not reveal ${entry.name} in Finder.')),
        );
      }
    }
  }

  void _setTerminalPresentation(TerminalPresentation presentation) {
    _setToolPresentation(WorkspaceToolKind.terminal, presentation);
  }

  void _setToolPresentation(
    WorkspaceToolKind tool,
    TerminalPresentation presentation,
  ) {
    setState(() {
      if (presentation == TerminalPresentation.maximized) {
        _terminalRestorePresentation =
            _terminalPresentation == TerminalPresentation.maximized
            ? TerminalPresentation.docked
            : _terminalPresentation;
      }
      _presentedTool = tool;
      _terminalPresentation = presentation;
      if (presentation != TerminalPresentation.docked) {
        if (tool == WorkspaceToolKind.terminal) {
          _dockedTerminalExpanded = true;
        } else {
          _dockedFilesExpanded = true;
        }
      }
    });
  }

  void _restoreMaximizedTerminal() {
    _setToolPresentation(_presentedTool, _terminalRestorePresentation);
  }

  Future<void> _loadPaneWidths() async {
    try {
      final stored = await _applicationChannel.invokeMapMethod<String, dynamic>(
        'getPaneWidths',
      );
      final projects = (stored?['projects'] as num?)?.toDouble() ?? 0;
      final inspector = (stored?['inspector'] as num?)?.toDouble() ?? 0;
      if (!mounted) return;
      setState(() {
        if (projects >= _minimumProjectSidebarWidth) {
          _projectSidebarWidth = projects;
        }
        if (inspector >= _minimumInspectorWidth) {
          _inspectorWidth = inspector;
        }
      });
    } on MissingPluginException {
      // Widget tests and non-macOS hosts keep the defaults.
    }
  }

  Future<void> _persistPaneWidths() async {
    try {
      await _applicationChannel.invokeMethod<void>('setPaneWidths', {
        'projects': _projectSidebarWidth,
        'inspector': _inspectorWidth,
      });
    } on MissingPluginException {
      // Persistence is native-only.
    }
  }

  double _maximumProjectSidebarWidth(
    double workspaceWidth,
    bool showInspector,
  ) {
    final reservedInspector = showInspector
        ? _inspectorWidth + _splitterExtent
        : 0.0;
    return (workspaceWidth -
            reservedInspector -
            _minimumAgentsWidth -
            _splitterExtent)
        .clamp(_minimumProjectSidebarWidth, 520.0);
  }

  double _maximumInspectorWidth(double workspaceWidth, bool showSidebar) {
    final reservedProjects = showSidebar
        ? _projectSidebarWidth + _splitterExtent
        : 0.0;
    return (workspaceWidth -
            reservedProjects -
            _minimumAgentsWidth -
            _splitterExtent)
        .clamp(_minimumInspectorWidth, 620.0);
  }

  Future<void> _ensureSelectedProjectTerminal() async {
    final project = _selectedProjectOrNull;
    if (project == null) return;
    final projectId = project.id;
    if (projectId == null || _projectTerminals.containsKey(projectId)) return;
    try {
      final response = await _runtimeClient.openProjectTerminal(
        projectId: projectId,
        columns: 90,
        rows: 24,
      );
      final value = response['ProjectTerminal'];
      if (value is! Map || !mounted) return;
      final id = value['id']?.toString();
      if (id == null || id.isEmpty) return;
      final terminal = ProjectTerminalSession(
        id: id,
        projectId: projectId,
        shell: value['shell']?.toString() ?? 'shell',
      );
      terminal.terminal.onOutput = (data) {
        unawaited(_runtimeClient.writeProjectTerminal(id, utf8.encode(data)));
      };
      terminal.terminal.onResize = (columns, rows, _, _) {
        unawaited(_runtimeClient.resizeProjectTerminal(id, columns, rows));
      };
      setState(() => _projectTerminals[projectId] = terminal);
    } on Object catch (_) {
      // Runtime connection failures already surface through its recovery UI.
    }
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
      projectId: _selectedProjectOrNull?.id,
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
      _codexBinary = status.codexBinary;
      final instanceId = status.instanceId;
      final snapshot = await _runtimeClient.snapshot();
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
      }
      _presentation.connected();
      unawaited(_ensureSelectedProjectTerminal());
      unawaited(_refreshNotificationReadiness());
      await _refreshCodexReadiness();
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

  Future<void> _refreshCodexReadiness() async {
    if (_codexSetupChecking) return;
    if (mounted) {
      setState(() {
        _codexSetupChecking = true;
        _codexSetupError = null;
      });
    }
    try {
      final report = await _runtimeClient.checkCodexReadiness();
      if (!mounted) return;
      setState(() {
        _codexReadiness = report;
        _codexBinary = report.path;
        _codexSetupChecking = false;
      });
      if (report.ready) {
        unawaited(
          _runtimeClient
              .listCodexModels()
              .then(agentExecutionSettings.setModels)
              .onError((_, _) {}),
        );
      }
    } on Object catch (error) {
      if (!mounted) return;
      setState(() {
        _codexSetupChecking = false;
        _codexSetupError = '$error';
      });
    }
  }

  Future<void> _refreshNotificationReadiness({bool request = false}) async {
    if (_notificationSetupChecking || !mounted) return;
    setState(() {
      _notificationSetupChecking = true;
      _notificationSetupError = null;
    });
    try {
      final values = await _applicationChannel
          .invokeMapMethod<Object?, Object?>(
            request
                ? 'requestNotificationAuthorization'
                : 'notificationAuthorizationStatus',
          );
      if (!mounted) return;
      if (values == null) {
        throw const FormatException(
          'Ditch Runtime returned no notification status.',
        );
      }
      setState(() {
        _notificationReadiness = NotificationReadiness.fromMap(values);
        _notificationSetupChecking = false;
      });
    } on MissingPluginException {
      if (mounted) setState(() => _notificationSetupChecking = false);
    } on Object catch (error) {
      if (!mounted) return;
      setState(() {
        _notificationSetupChecking = false;
        _notificationSetupError = '$error';
      });
    }
  }

  Future<void> _manageNotificationSetup() async {
    final state = _notificationReadiness?.authorization;
    if (state == NotificationAuthorizationState.denied ||
        (_notificationReadiness?.authorized == true &&
            _notificationReadiness?.ready == false)) {
      try {
        await _applicationChannel.invokeMethod<bool>(
          'openNotificationSettings',
        );
      } on Object catch (error) {
        if (mounted) setState(() => _notificationSetupError = '$error');
      }
      return;
    }
    await _refreshNotificationReadiness(request: true);
  }

  Future<void> _updateCodex() async {
    final report = _codexReadiness;
    if (report == null || !report.updateSupported || !mounted) return;
    final approved = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        icon: const Icon(Icons.system_update_alt),
        title: const Text('Update Codex?'),
        content: Text(
          'Ditch will run `codex update` using your selected installation.\n\n${report.path}\n${report.version ?? "Unknown version"}\n\nIt will not modify PATH, Homebrew, npm, or another installation.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Update Codex'),
          ),
        ],
      ),
    );
    if (approved != true || !mounted) return;
    setState(() {
      _codexSetupUpdating = true;
      _codexSetupError = null;
    });
    try {
      final updated = await _runtimeClient.updateSelectedCodex();
      if (!mounted) return;
      setState(() {
        _codexReadiness = updated;
        _codexBinary = updated.path;
        _codexSetupUpdating = false;
      });
    } on Object catch (error) {
      if (!mounted) return;
      setState(() {
        _codexSetupUpdating = false;
        _codexSetupError = '$error';
      });
    }
  }

  Future<void> _startCodexLogin() async {
    final binary = _codexReadiness?.path;
    if (binary == null) return;
    try {
      final opened =
          await _applicationChannel.invokeMethod<bool>(
            'openCodexLogin',
            binary,
          ) ??
          false;
      if (!opened && mounted) {
        setState(() => _codexSetupError = 'Could not open Codex sign-in.');
        return;
      }
      for (var attempt = 0; attempt < 60 && mounted; attempt += 1) {
        await Future<void>.delayed(const Duration(seconds: 2));
        if (!mounted) return;
        await _refreshCodexReadiness();
        if (_codexReadiness?.authenticated == true) return;
      }
    } on Object catch (error) {
      if (mounted) setState(() => _codexSetupError = '$error');
    }
  }

  Future<void> _openCodexInstallInstructions() async {
    try {
      await _applicationChannel.invokeMethod<bool>(
        'openURL',
        'https://learn.chatgpt.com/docs/codex/cli',
      );
    } on Object catch (error) {
      if (mounted) setState(() => _codexSetupError = '$error');
    }
  }

  void _repairCodexSetup() {
    final report = _codexReadiness;
    if (report != null && !report.compatible && report.updateSupported) {
      unawaited(_updateCodex());
    } else if (report != null && report.compatible && !report.authenticated) {
      unawaited(_startCodexLogin());
    } else {
      unawaited(_openCodexSettings());
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

  Future<void> _openCodexSettings() async {
    try {
      final installations = await _runtimeClient.discoverCodexInstallations();
      if (!mounted) return;
      final selected = await showDialog<String>(
        context: context,
        builder: (context) => AlertDialog(
          icon: const Icon(Icons.terminal),
          title: const Text('Codex installation'),
          content: SizedBox(
            width: 560,
            child: installations.isEmpty
                ? const Text(
                    'No working Codex CLI installation was found. Install Codex for your user account, then reopen this window.',
                  )
                : ConstrainedBox(
                    constraints: const BoxConstraints(maxHeight: 420),
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        const Text(
                          'Ditch uses one of your existing Codex installations. It never creates a private toolchain or changes your shell PATH.',
                        ),
                        const SizedBox(height: 12),
                        Flexible(
                          child: ListView(
                            shrinkWrap: true,
                            children: installations.map((installation) {
                              return ListTile(
                                contentPadding: EdgeInsets.zero,
                                leading: Icon(
                                  installation.path == _codexBinary ||
                                          installation.selected
                                      ? Icons.check_circle
                                      : Icons.circle_outlined,
                                ),
                                title: Text(installation.version),
                                subtitle: SelectableText(installation.path),
                                onTap: () => Navigator.of(
                                  context,
                                ).pop(installation.path),
                              );
                            }).toList(),
                          ),
                        ),
                      ],
                    ),
                  ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(),
              child: const Text('Close'),
            ),
          ],
        ),
      );
      if (selected == null || selected == _codexBinary) return;
      await _runtimeClient.selectCodexBinary(selected);
      if (!mounted) return;
      setState(() => _codexBinary = selected);
      await _refreshCodexReadiness();
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Codex installation updated.')),
      );
    } on Object catch (error) {
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Could not load Codex installations: $error')),
      );
    }
  }

  Future<void> _openAppSettings() async {
    final section = await showDialog<String>(
      context: context,
      builder: (context) => SimpleDialog(
        title: const Text('Settings'),
        children: [
          SimpleDialogOption(
            onPressed: () => Navigator.pop(context, 'codex'),
            child: const ListTile(
              leading: Icon(Icons.terminal),
              title: Text('Codex'),
              subtitle: Text('Installation and runtime setup'),
            ),
          ),
          SimpleDialogOption(
            key: const Key('settings-remote-mobile'),
            onPressed: () => Navigator.pop(context, 'remote'),
            child: const ListTile(
              leading: Icon(Icons.phone_iphone),
              title: Text('Remote / Mobile'),
              subtitle: Text('Pair and manage iPhones'),
            ),
          ),
        ],
      ),
    );
    if (!mounted) return;
    if (section == 'codex') {
      await _openCodexSettings();
    } else if (section == 'remote') {
      await showDialog<void>(
        context: context,
        builder: (context) => RemoteSettingsDialog(client: _runtimeClient),
      );
    }
  }

  void _checkRuntimeCapabilities(RuntimeStatusDto status) {
    _runtimeSupportsPersistence = status.supportsPersistentSessions;
    _runtimeSupportsAlwaysOnWebAccess = status.supportsAlwaysOnWebAccess;
    if (_runtimeSupportsPersistence && _runtimeSupportsAlwaysOnWebAccess) {
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
          'This runtime does not support the current agent execution profile. Rebuild and restart Ditch before starting Codex.',
      global: true,
    );
  }

  void _scheduleRuntimeReconnect() {
    if (_runtimeReconnectScheduled || !mounted) {
      return;
    }
    _runtimeSnapshotHydrated = false;
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
        'Ditch Runtime is unavailable. Use the status menu to restart it, then retry. $error',
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
    final messageOrdinals = <String, int>{};
    if (messagesJson is List) {
      for (final messageJson in messagesJson) {
        final agentId = _agentIdFromRuntimeMessage(messageJson);
        final ordinal = agentId == null ? null : messageOrdinals[agentId] ?? 0;
        final message = _agentChatMessageFromRuntime(
          messageJson,
          identity: agentId == null ? null : 'persisted:$agentId:$ordinal',
        );
        if (message != null && agentId != null) {
          messagesByAgent.putIfAbsent(agentId, () => []).add(message);
          messageOrdinals[agentId] = ordinal! + 1;
        }
      }
    }

    final existingSessions = {
      for (final session in _agentSessions) session.localId: session,
    };
    final sessionsById = <String, AgentSession>{};
    for (final agentJson in agentsJson) {
      final session = _agentSessionFromRuntime(
        agentJson,
        messagesByAgent[_agentIdFromRuntimeAgent(agentJson)] ?? const [],
      );
      if (session != null) {
        final existing = existingSessions[session.localId];
        if (existing != null) {
          session.messages
            ..clear()
            ..addAll(existing.messages);
          session.messagesLoaded = existing.messagesLoaded;
          session.messagesLoading = existing.messagesLoading;
          session.hasOlderMessages = existing.hasOlderMessages;
          session.nextBeforeSequence = existing.nextBeforeSequence;
          session.historyError = existing.historyError;
        }
        sessionsById[session.localId] = session;
      }
    }
    final sessions = sessionsById.values.toList();
    sessions.sort((a, b) => b.updatedAt.compareTo(a.updatedAt));
    final removedViewportIds = _chatViewports.keys
        .where((agentId) => !sessionsById.containsKey(agentId))
        .toList();
    final removedViewports = removedViewportIds
        .map(_chatViewports.remove)
        .whereType<ConversationViewportController>()
        .toList();
    final removedComposerKeyIds = _composerKeys.keys
        .where((agentId) => !sessionsById.containsKey(agentId))
        .toList();
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
      final selectedPath = _selectedProjectOrNull?.path;
      projects.sort((a, b) => a.name.compareTo(b.name));
      _projects
        ..clear()
        ..addAll(projects);
      if (_projects.isEmpty) {
        _selectedProjectKey = null;
      } else {
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
      for (final agentId in removedComposerKeyIds) {
        _composerKeys.remove(agentId);
      }
      _attention
        ..clear()
        ..addAll(attention);
      _readAttentionIds
        ..clear()
        ..addAll(
          attention.where((event) => event.isRead).map((event) => event.id),
        );
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
      _runtimeSnapshotHydrated = true;
    });
    if (removedViewports.isNotEmpty) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        for (final viewport in removedViewports) {
          viewport.dispose();
        }
      });
    }
    final expanded = _expandedAgentLocalId == null
        ? null
        : sessionsById[_expandedAgentLocalId];
    if (expanded != null) unawaited(_loadAgentMessages(expanded));
    _scheduleStatusBarUpdate();
    final pendingTarget = _pendingNotificationTarget;
    if (pendingTarget != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _openNotificationTarget(pendingTarget);
      });
    }
  }

  DitchProject? _projectFromRuntime(Object? value) {
    return parseRuntimeProject(value);
  }

  @override
  void initState() {
    super.initState();
    _projects = List<DitchProject>.of(widget.initialProjects);
    _selectedProjectKey = _projects.isEmpty
        ? null
        : _projectKey(_projects.first);
    _configureNativeNavigation();
    _scheduleStatusBarUpdate();
    unawaited(_loadPaneWidths());
    if (widget.connectRuntimeOnStart) {
      unawaited(_connectRuntime());
    }
  }

  @override
  void dispose() {
    _applicationChannel.setMethodCallHandler(null);
    _presentation.dispose();
    _runtimeEvents?.cancel();
    _idleChatViewport.dispose();
    for (final viewport in _chatViewports.values) {
      viewport.dispose();
    }
    for (final files in _projectFiles.values) {
      files.dispose();
    }
    _agentListController.dispose();
    super.dispose();
  }

  void _configureNativeNavigation() {
    _applicationChannel.setMethodCallHandler((call) async {
      if (call.method != 'openAgent') return false;
      final target = AgentNotificationTarget.fromArguments(call.arguments);
      if (target == null) return false;
      _receiveNotificationTarget(target);
      return true;
    });
    unawaited(
      _applicationChannel
          .invokeMapMethod<String, dynamic>('consumePendingNavigation')
          .then((arguments) {
            final target = AgentNotificationTarget.fromArguments(arguments);
            if (target != null) _receiveNotificationTarget(target);
          })
          .onError((_, _) {}),
    );
  }

  void _receiveNotificationTarget(AgentNotificationTarget target) {
    _pendingNotificationTarget = target;
    if (_runtimeSnapshotHydrated) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _openNotificationTarget(target);
      });
    }
  }

  void _openNotificationTarget(AgentNotificationTarget target) {
    if (_pendingNotificationTarget != target) return;
    final projectIndex = _projects.indexWhere(
      (project) => project.id == target.projectId,
    );
    final session = _agentSessions
        .where(
          (candidate) =>
              candidate.localId == target.agentId &&
              candidate.projectId == target.projectId,
        )
        .firstOrNull;
    if (projectIndex < 0 || session == null) {
      _pendingNotificationTarget = null;
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'This notification refers to an agent session that is no longer available.',
          ),
        ),
      );
      return;
    }

    setState(() {
      _selectedProjectKey = _projectKey(_projects[projectIndex]);
      _expandedAgentLocalId = session.localId;
      _focusedAgentLocalId = null;
      _pendingNotificationTarget = null;
      if (target.attentionId != null) {
        _attention.removeWhere((event) => event.id == target.attentionId);
        _readAttentionIds.remove(target.attentionId);
      }
    });
    _chatViewports
        .putIfAbsent(session.localId, ConversationViewportController.new)
        .beginOpening();
    _focusAgentComposerAfterLayout(session.localId);
    unawaited(_loadAgentMessages(session));
    _scheduleStatusBarUpdate();
    unawaited(_ensureSelectedProjectTerminal());
    final attentionId = target.attentionId;
    if (attentionId != null) {
      unawaited(_runtimeClient.dismissAttention(attentionId));
    }
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
            'Ditch verified agents, hooks, and MCP directories in ${configuredProject.path}/.ditch.',
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

  Future<void> _copyProjectPath(DitchProject project) async {
    await Clipboard.setData(ClipboardData(text: project.path));
    if (!mounted) return;
    ScaffoldMessenger.of(
      context,
    ).showSnackBar(const SnackBar(content: Text('Project path copied.')));
  }

  Future<void> _deleteProject(DitchProject project) async {
    final projectId = project.id;
    if (projectId == null || !mounted) return;
    final hasActiveAgent = _agentSessions.any(
      (session) => session.projectId == projectId && session.isActive,
    );
    if (hasActiveAgent) {
      _showProjectSetupResult(
        title: 'Project has active agents',
        message: 'Stop this project\'s active agents before deleting it.',
        isError: true,
      );
      return;
    }
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Delete ${project.name}?'),
        content: const Text(
          'This removes the project, its agents, chat history, and alerts from Ditch. The project folder and its files will not be deleted.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Delete Project'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    try {
      await _runtimeClient.deleteProject(projectId);
      _removeProjectFromUi(projectId);
    } on Object catch (error) {
      if (!mounted) return;
      _showProjectSetupResult(
        title: 'Project deletion failed',
        message: '$error',
        isError: true,
      );
    }
  }

  void _removeProjectFromUi(String projectId) {
    if (!_projects.any((project) => project.id == projectId)) return;
    final removedSessionIds = _agentSessions
        .where((session) => session.projectId == projectId)
        .map((session) => session.localId)
        .toSet();
    final removedViewports = removedSessionIds
        .map(_chatViewports.remove)
        .whereType<ConversationViewportController>()
        .toList();
    final removedFiles = _projectFiles.remove(projectId);
    setState(() {
      _projects.removeWhere((project) => project.id == projectId);
      _projectTerminals.remove(projectId);
      _agentSessions.removeWhere((session) => session.projectId == projectId);
      _attention.removeWhere((event) => event.projectId == projectId);
      for (final id in removedSessionIds) {
        _composerKeys.remove(id);
      }
      if (removedSessionIds.contains(_expandedAgentLocalId)) {
        _expandedAgentLocalId = null;
      }
      if (removedSessionIds.contains(_focusedAgentLocalId)) {
        _focusedAgentLocalId = null;
      }
      if (_projects.isEmpty) {
        _selectedProjectKey = null;
      } else if (!_projects.any(
        (project) => _projectKey(project) == _selectedProjectKey,
      )) {
        _selectedProjectKey = _projectKey(_projects.first);
      }
    });
    removedFiles?.dispose();
    if (removedViewports.isNotEmpty) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        for (final viewport in removedViewports) {
          viewport.dispose();
        }
      });
    }
    _scheduleStatusBarUpdate();
    if (_projects.isNotEmpty) {
      unawaited(_ensureSelectedProjectTerminal());
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
    await _activeComposerKey.currentState?.blur();
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
      final session = _agentSessionFromRuntime(run, const []);
      if (session != null) {
        final existing = _agentSessionByLocalId(session.localId);
        final target = existing ?? session;
        setState(() {
          if (existing == null) {
            _agentSessions.insert(0, session);
          }
          _expandedAgentLocalId = session.localId;
        });
        _focusAgentComposerAfterLayout(session.localId);
        if (target.messages.isEmpty) {
          unawaited(_loadAgentMessages(target));
        }
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
      _focusAgentComposerAfterLayout(session.localId);
      _scheduleStatusBarUpdate();
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Failed to start Codex through Ditch Runtime: $error',
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
    if (_codexReadiness?.ready != true) {
      await _refreshCodexReadiness();
      if (_codexReadiness?.ready != true) {
        _showProjectSetupResult(
          title: 'Codex setup required',
          message:
              _codexReadiness?.issues.join('\n') ??
              _codexSetupError ??
              'Complete Codex setup before starting an agent.',
          isError: true,
        );
        return false;
      }
    }
    if (!mounted) return false;
    if (!_runtimeSupportsPersistence || !_runtimeSupportsAlwaysOnWebAccess) {
      _showProjectSetupResult(
        title: 'Outdated Ditch Runtime',
        message:
            'This runtime does not support the current agent launch policy. Rebuild and restart Ditch, then try again.',
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
    if (session.isActive) {
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
        'Failed to send prompt through Ditch Runtime: $error',
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
    setState(() {
      session.status = AgentStatus.stopping;
      session.canStop = false;
      session.updatedAt = DateTime.now();
    });
    try {
      await _runtimeClient.stopAgent(session.localId);
      setState(() {
        session.status = AgentStatus.stopped;
        session.canStop = false;
        session.updatedAt = DateTime.now();
      });
      _scheduleStatusBarUpdate();
    } on Object catch (error) {
      _addChatMessage(
        session,
        ChatMessageRole.system,
        'Failed to stop Codex through Ditch Runtime: $error',
      );
    }
  }

  Future<void> _deleteAgent(AgentSession session) async {
    if (session.isActive || !mounted) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Delete this agent?'),
        content: const Text(
          'This permanently removes the agent, its chat history, and its alerts from Ditch. Project files are not deleted.',
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
    final session = sessionLocalId == null
        ? null
        : _agentSessionByLocalId(sessionLocalId);
    final eventProjectId = global
        ? null
        : (projectId ?? session?.projectId ?? _selectedProjectOrNull?.id);
    final project = _projects
        .where((candidate) => candidate.id == eventProjectId)
        .firstOrNull;
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
          projectId: eventProjectId,
          projectName: project?.name,
          agentName: session?.displayName,
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

    final project = _projects
        .where((candidate) => candidate.id == event.projectId)
        .firstOrNull;
    setState(() {
      if (project != null) _selectedProjectKey = _projectKey(project);
      _expandedAgentLocalId = sessionLocalId;
      _focusedAgentLocalId = null;
    });
    _chatViewports
        .putIfAbsent(sessionLocalId, ConversationViewportController.new)
        .beginOpening();
    _focusAgentComposerAfterLayout(sessionLocalId);
    final session = _agentSessionByLocalId(sessionLocalId);
    if (session != null) unawaited(_loadAgentMessages(session));
    _markAttentionIdsRead({event.id});
    _scheduleStatusBarUpdate();
  }

  void _dismissAttention(AttentionEvent event) {
    setState(() {
      _attention.removeWhere((candidate) => candidate.id == event.id);
      _readAttentionIds.remove(event.id);
    });
    _scheduleStatusBarUpdate();
    unawaited(_runtimeClient.dismissAttention(event.id));
  }

  void _markNotificationsRead() {
    final newlyRead = _attention
        .where((event) => !_readAttentionIds.contains(event.id))
        .map((event) => event.id)
        .toSet();
    if (newlyRead.isEmpty) return;
    setState(() => _readAttentionIds.addAll(newlyRead));
    unawaited(
      _runtimeClient.markAllAttentionRead().catchError((Object error) {
        if (!mounted) return <String, dynamic>{};
        setState(() => _readAttentionIds.removeAll(newlyRead));
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not mark notifications read: $error')),
        );
        return <String, dynamic>{};
      }),
    );
  }

  void _markAttentionIdsRead(Set<String> attentionIds) {
    final newlyRead = attentionIds
        .where(
          (id) =>
              _attention.any((event) => event.id == id && !event.isRead) &&
              !_readAttentionIds.contains(id),
        )
        .toSet();
    if (newlyRead.isEmpty) return;
    setState(() => _readAttentionIds.addAll(newlyRead));
    final persistedIds = newlyRead
        .where((id) => !id.startsWith('attention-'))
        .toList();
    if (persistedIds.isEmpty) return;
    unawaited(
      _runtimeClient.markAttentionRead(persistedIds).catchError((Object error) {
        if (!mounted) return <String, dynamic>{};
        setState(() => _readAttentionIds.removeAll(persistedIds));
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not mark agent result read: $error')),
        );
        return <String, dynamic>{};
      }),
    );
  }

  void _markAgentResultsRead(AgentSession session) {
    _markAttentionIdsRead(
      unreadResultAttentionIdsForAgent(
        attention: _attention,
        agentId: session.localId,
        readAttentionIds: _readAttentionIds,
      ),
    );
  }

  void _dismissAllNotifications() {
    final events = List<AttentionEvent>.from(_attention);
    setState(() {
      _attention.clear();
      _readAttentionIds.clear();
    });
    for (final event in events) {
      if (!event.id.startsWith('attention-')) {
        unawaited(_runtimeClient.dismissAttention(event.id));
      }
    }
    _scheduleStatusBarUpdate();
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
      _readAttentionIds.removeAll(resolved.map((event) => event.id));
    });
    for (final event in resolved) {
      if (!event.id.startsWith('attention-')) {
        unawaited(_runtimeClient.dismissAttention(event.id));
      }
    }
    _scheduleStatusBarUpdate();
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

    final terminalOutput = eventBody['ProjectTerminalOutput'];
    if (terminalOutput is Map) {
      final id = terminalOutput['terminal_id']?.toString();
      final raw = terminalOutput['data'];
      final session = id == null
          ? null
          : _projectTerminals.values.where((item) => item.id == id).firstOrNull;
      if (session != null && raw is List) {
        final bytes = raw.whereType<num>().map((byte) => byte.toInt()).toList();
        session.terminal.write(utf8.decode(bytes, allowMalformed: true));
      }
      return;
    }

    final terminalExited = eventBody['ProjectTerminalExited'];
    if (terminalExited is Map) {
      final id = terminalExited['terminal_id']?.toString();
      if (id != null) {
        final keys = _projectTerminals.entries
            .where((entry) => entry.value.id == id)
            .map((entry) => entry.key)
            .toList();
        if (keys.isNotEmpty) {
          setState(() => keys.forEach(_projectTerminals.remove));
        }
      }
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

    final projectDeleted = eventBody['ProjectDeleted'];
    if (projectDeleted is Map<String, dynamic>) {
      final id = _agentIdToString(projectDeleted['project_id']);
      if (id != null) _removeProjectFromUi(id);
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
        final removedViewport = _chatViewports.remove(id);
        setState(() {
          _agentSessions.removeWhere((session) => session.localId == id);
          _composerKeys.remove(id);
          _attention.removeWhere((event) => event.sessionLocalId == id);
          if (_expandedAgentLocalId == id) _expandedAgentLocalId = null;
          if (_focusedAgentLocalId == id) _focusedAgentLocalId = null;
        });
        if (removedViewport != null) {
          WidgetsBinding.instance.addPostFrameCallback(
            (_) => removedViewport.dispose(),
          );
        }
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
      _addChatMessage(
        session,
        message.role,
        message.text,
        identity: message.identity,
        createdAt: message.createdAt,
      );
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

    final attentionRead = eventBody['AttentionRead'];
    if (attentionRead is Map<String, dynamic>) {
      final values = attentionRead['attention_ids'];
      if (values is List) {
        setState(() {
          _readAttentionIds.addAll(values.map((value) => value.toString()));
        });
      }
      return;
    }

    final attentionDismissed = eventBody['AttentionDismissed'];
    if (attentionDismissed is Map<String, dynamic>) {
      final id = attentionDismissed['attention_id']?.toString();
      if (id != null) {
        setState(() {
          _attention.removeWhere((item) => item.id == id);
          _readAttentionIds.remove(id);
        });
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
      messages: List<AgentChatMessage>.from(messages),
      messagesLoaded: messages.isNotEmpty,
      hasOlderMessages: messages.isNotEmpty,
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
      canStop: agentJson['can_stop'] == true,
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
      'Completed' => AttentionKind.completed,
      'Failed' => AttentionKind.failed,
      _ => AttentionKind.needsInput,
    };
    return AttentionEvent(
      id: id,
      kind: kind,
      icon: switch (kind) {
        AttentionKind.failed => Icons.error_outline,
        AttentionKind.completed => Icons.task_alt_outlined,
        _ => Icons.notifications_active_outlined,
      },
      title: title,
      body: body,
      sessionLocalId: _agentIdToString(value['agent_id']),
      projectId: value['project_id']?.toString(),
      projectName: value['project_name']?.toString(),
      agentName: value['agent_name']?.toString(),
      createdAt: _dateTimeFromRuntime(value['created_at']),
      isRead: value['read_at'] != null,
    );
  }

  AgentChatMessage? _agentChatMessageFromRuntime(
    Object? messageJson, {
    String? identity,
  }) {
    if (messageJson is! Map<String, dynamic>) {
      return null;
    }
    final text = messageJson['text']?.toString();
    if (text == null || text.trim().isEmpty) {
      return null;
    }
    final role = _chatRoleFromRuntime(messageJson['role']?.toString());
    final createdAt = _dateTimeFromRuntime(messageJson['created_at']);
    final agentId = _agentIdFromRuntimeMessage(messageJson) ?? 'unknown-agent';
    final runtimeIdentity = messageJson['id']?.toString();
    return AgentChatMessage(
      identity:
          identity ??
          runtimeIdentity ??
          '$agentId:${createdAt.toUtc().microsecondsSinceEpoch}:${role.name}',
      role: role,
      text: text,
      createdAt: createdAt,
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
      'Stopping' => AgentStatus.stopping,
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
    String text, {
    String? identity,
    DateTime? createdAt,
  }) {
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
        AgentChatMessage(
          identity: identity,
          role: role,
          text: text,
          createdAt: createdAt ?? DateTime.now(),
        ),
      );
      session.updatedAt = DateTime.now();
    });
    _scheduleStatusBarUpdate();
  }

  Future<void> _loadAgentMessages(AgentSession session) async {
    if (session.messagesLoading ||
        (session.messagesLoaded && !session.hasOlderMessages)) {
      return;
    }
    final initial = !session.messagesLoaded;
    setState(() {
      session.messagesLoading = true;
      session.historyError = null;
    });
    try {
      final response = await _runtimeClient.listAgentMessages(
        agentId: session.localId,
        beforeSequence: initial ? null : session.nextBeforeSequence,
      );
      final page = response['AgentMessages'];
      if (page is! Map<String, dynamic>) {
        throw const FormatException(
          'Runtime returned an invalid message page.',
        );
      }
      final pageItems = <AgentChatMessage>[];
      final rawMessages = page['messages'];
      if (rawMessages is List) {
        for (final rawItem in rawMessages) {
          if (rawItem is! Map<String, dynamic>) continue;
          final sequence = rawItem['sequence'];
          final message = _agentChatMessageFromRuntime(
            rawItem['message'],
            identity: sequence is int
                ? 'persisted:${session.localId}:$sequence'
                : null,
          );
          if (message != null) pageItems.add(message);
        }
      }
      if (!mounted) return;
      final target = _agentSessionByLocalId(session.localId);
      if (target == null) return;
      setState(() {
        final additions = uniqueRuntimeMessages(target.messages, pageItems);
        target.messages.insertAll(0, additions);
        target.messagesLoaded = true;
        target.messagesLoading = false;
        target.hasOlderMessages = page['has_more'] == true;
        target.nextBeforeSequence = page['next_before_sequence'] is int
            ? page['next_before_sequence'] as int
            : null;
      });
    } on Object catch (error) {
      if (!mounted) return;
      final target = _agentSessionByLocalId(session.localId);
      if (target == null) return;
      setState(() {
        target.messagesLoading = false;
        target.historyError = 'Could not load message history. $error';
      });
    }
  }

  void _focusAgentComposerAfterLayout(String agentId) {
    final generation = ++_composerFocusGeneration;
    final composerKey = _composerKeyForAgent(agentId);

    void focusWhenMounted(int remainingAttempts) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted || generation != _composerFocusGeneration) return;
        final activeAgentId = _focusedAgentLocalId ?? _expandedAgentLocalId;
        if (activeAgentId != agentId) return;
        final composer = composerKey.currentState;
        if (composer != null) {
          composer.focus();
        } else if (remainingAttempts > 0) {
          focusWhenMounted(remainingAttempts - 1);
        }
      });
    }

    focusWhenMounted(2);
  }

  void _toggleExpandedAgent(AgentSession session) {
    final collapsing = _expandedAgentLocalId == session.localId;
    _markAgentResultsRead(session);
    setState(() {
      _expandedAgentLocalId = collapsing ? null : session.localId;
    });
    if (collapsing) return;
    _chatViewports
        .putIfAbsent(session.localId, ConversationViewportController.new)
        .beginOpening();
    _focusAgentComposerAfterLayout(session.localId);
    unawaited(_loadAgentMessages(session));
  }

  Future<void> _loadProjectDirectory(
    String projectId,
    String relativePath, {
    bool refresh = false,
  }) async {
    final files = _projectFiles.putIfAbsent(projectId, ProjectFilesState.new);
    if (files.loadingDirectories.contains(relativePath) ||
        (!refresh && files.directories.containsKey(relativePath))) {
      return;
    }
    setState(() {
      files.loadingDirectories.add(relativePath);
      files.error = null;
    });
    try {
      final response = await _runtimeClient.listProjectDirectory(
        projectId: projectId,
        relativePath: relativePath,
      );
      final value = response['ProjectDirectory'];
      if (value is! Map<String, dynamic> || value['entries'] is! List) {
        throw const FormatException('Runtime returned an invalid directory.');
      }
      final entries = (value['entries'] as List)
          .whereType<Map>()
          .map((entry) {
            final kind = switch (entry['kind']?.toString()) {
              'Directory' => ProjectFileEntryKind.directory,
              'File' => ProjectFileEntryKind.file,
              _ => ProjectFileEntryKind.symlink,
            };
            return ProjectFileEntry(
              name: entry['name']?.toString() ?? '',
              relativePath: entry['relative_path']?.toString() ?? '',
              kind: kind,
              size: (entry['size'] as num?)?.toInt() ?? 0,
            );
          })
          .where((entry) => entry.name.isNotEmpty)
          .toList();
      if (!mounted) return;
      setState(() {
        files.directories[relativePath] = entries;
        files.loadingDirectories.remove(relativePath);
      });
    } on Object catch (error) {
      if (!mounted) return;
      setState(() {
        files.loadingDirectories.remove(relativePath);
        files.error = '$error';
      });
    }
  }

  Future<void> _toggleProjectDirectory(
    String projectId,
    ProjectFileEntry entry,
  ) async {
    final files = _projectFiles.putIfAbsent(projectId, ProjectFilesState.new);
    if (files.expandedDirectories.contains(entry.relativePath)) {
      setState(() => files.expandedDirectories.remove(entry.relativePath));
      return;
    }
    setState(() => files.expandedDirectories.add(entry.relativePath));
    await _loadProjectDirectory(projectId, entry.relativePath);
  }

  Future<bool> _confirmDiscardEditor(ProjectFilesState files) async {
    final document = files.document;
    if (document == null || !document.dirty) return true;
    final result = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Unsaved changes'),
        content: Text('Save changes to ${document.relativePath}?'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, 'cancel'),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.pop(context, 'discard'),
            child: const Text('Discard'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, 'save'),
            child: const Text('Save'),
          ),
        ],
      ),
    );
    if (result == 'save') return _saveProjectFile(files);
    return result == 'discard';
  }

  Future<void> _openProjectFile(
    String projectId,
    ProjectFileEntry entry,
  ) async {
    final files = _projectFiles.putIfAbsent(projectId, ProjectFilesState.new);
    if (!await _confirmDiscardEditor(files)) return;
    try {
      final response = await _runtimeClient.readProjectFile(
        projectId: projectId,
        relativePath: entry.relativePath,
      );
      final value = response['ProjectFile'];
      if (value is! Map<String, dynamic>) {
        throw const FormatException('Runtime returned an invalid file.');
      }
      final document = ProjectEditorDocument(
        relativePath: entry.relativePath,
        content: value['content']?.toString() ?? '',
        revision: value['revision']?.toString() ?? '',
      );
      document.controller.addListener(() {
        if (mounted && identical(files.document, document)) setState(() {});
      });
      if (!mounted) {
        document.dispose();
        return;
      }
      setState(() {
        files.document?.dispose();
        files.document = document;
        files.error = null;
      });
    } on Object catch (error) {
      if (!mounted) return;
      setState(() => files.error = '$error');
    }
  }

  Future<bool> _saveProjectFile(
    ProjectFilesState files, {
    bool overwrite = false,
  }) async {
    final projectId = _selectedProject.id;
    final document = files.document;
    if (projectId == null || document == null || document.saving) return false;
    setState(() {
      document.saving = true;
      document.conflict = false;
      document.error = null;
    });
    try {
      final response = await _runtimeClient.writeProjectFile(
        projectId: projectId,
        relativePath: document.relativePath,
        expectedRevision: overwrite ? null : document.revision,
        content: document.controller.text,
      );
      final value = response['ProjectFileSaved'];
      if (value is! Map<String, dynamic>) {
        throw const FormatException('Runtime returned an invalid save result.');
      }
      if (!mounted) return false;
      setState(() {
        document.revision = value['revision']?.toString() ?? document.revision;
        document.originalContent = document.controller.text;
        document.saving = false;
        document.conflict = false;
      });
      return true;
    } on Object catch (error) {
      if (!mounted) return false;
      setState(() {
        document.saving = false;
        document.conflict =
            error is DitchRuntimeException && error.code == 'file_changed';
        document.error = document.conflict
            ? 'This file changed on disk.'
            : '$error';
      });
      return false;
    }
  }

  Future<void> _closeProjectFile(ProjectFilesState files) async {
    if (!await _confirmDiscardEditor(files)) return;
    setState(() {
      files.document?.dispose();
      files.document = null;
      if (_presentedTool == WorkspaceToolKind.editor) {
        _presentedTool = WorkspaceToolKind.terminal;
        _terminalPresentation = TerminalPresentation.docked;
      }
    });
  }

  Future<void> _reloadProjectFile(
    String projectId,
    ProjectFilesState files,
  ) async {
    final path = files.document?.relativePath;
    if (path == null) return;
    await _openProjectFile(
      projectId,
      ProjectFileEntry(
        name: path.split('/').last,
        relativePath: path,
        kind: ProjectFileEntryKind.file,
        size: 0,
      ),
    );
  }

  Future<void> _overwriteProjectFile(ProjectFilesState files) async {
    final document = files.document;
    if (document == null) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Overwrite external changes?'),
        content: Text(
          '${document.relativePath} changed on disk. Overwriting will replace those external changes.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Overwrite'),
          ),
        ],
      ),
    );
    if (confirmed == true) {
      await _saveProjectFile(files, overwrite: true);
    }
  }

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<CommandCenterPresentationState>(
      valueListenable: _presentation,
      builder: (context, presentation, _) {
        return Scaffold(
          body: LayoutBuilder(
            builder: (context, constraints) {
              if (_projects.isEmpty) {
                return CodexOnboardingView(
                  introduced: _onboardingIntroduced,
                  checking: _codexSetupChecking,
                  updating: _codexSetupUpdating,
                  readiness: _codexReadiness,
                  error: _codexSetupError,
                  notificationReadiness: _notificationReadiness,
                  notificationChecking: _notificationSetupChecking,
                  notificationError: _notificationSetupError,
                  onContinue: () {
                    setState(() => _onboardingIntroduced = true);
                    unawaited(_refreshCodexReadiness());
                  },
                  onCheckAgain: _refreshCodexReadiness,
                  onChooseInstallation: _openCodexSettings,
                  onUpdate: _updateCodex,
                  onSignIn: _startCodexLogin,
                  onOpenInstallInstructions: _openCodexInstallInstructions,
                  onManageNotifications: _manageNotificationSetup,
                  onCheckNotifications: _refreshNotificationReadiness,
                  onAddProject: _addProject,
                );
              }
              final compact = constraints.maxWidth < 760;
              final showSidebar =
                  presentation.sidebarVisible && _focusedAgentLocalId == null;
              final showInspector =
                  presentation.inspectorVisible &&
                  !compact &&
                  _focusedAgentLocalId == null;
              final projectSidebarWidth = _projectSidebarWidth.clamp(
                _minimumProjectSidebarWidth,
                _maximumProjectSidebarWidth(
                  constraints.maxWidth,
                  showInspector,
                ),
              );
              final inspectorWidth = _inspectorWidth.clamp(
                _minimumInspectorWidth,
                _maximumInspectorWidth(constraints.maxWidth, showSidebar),
              );
              final agentsSurface = AgentsSurface(
                sessions: _visibleSessions,
                expandedAgentLocalId: _expandedAgentLocalId,
                focusedAgentLocalId: _focusedAgentLocalId,
                chatViewport: _chatViewport,
                agentListController: _agentListController,
                composerKey: _idleComposerKey,
                composerKeyForAgent: _composerKeyForAgent,
                headerKeyForAgent: _agentHeaderKey,
                initialPrompt: _defaultStartPrompt,
                effectiveCodexHome: _effectiveRuntimeCodexHome,
                onStartCodex: _startCodex,
                onStartPrompt: _startCodexRuntime,
                onSubmitPrompt: _submitComposer,
                onLoadMessages: _loadAgentMessages,
                onStopCodex: _stopCodex,
                onDeleteAgent: _deleteAgent,
                onRenameAgent: _renameAgent,
                hasUnreadResult: (session) => unreadResultAttentionIdsForAgent(
                  attention: _attention,
                  agentId: session.localId,
                  readAttentionIds: _readAttentionIds,
                ).isNotEmpty,
                onFocusAgent: (session) {
                  if (session != null) _markAgentResultsRead(session);
                  setState(() {
                    _focusedAgentLocalId = session?.localId;
                    if (session != null) {
                      _expandedAgentLocalId = session.localId;
                    }
                  });
                  final agentId = session?.localId ?? _expandedAgentLocalId;
                  if (agentId != null) {
                    _focusAgentComposerAfterLayout(agentId);
                  }
                },
                onToggleExpanded: _toggleExpandedAgent,
              );
              final selectedTerminal = _selectedProject.id == null
                  ? null
                  : _projectTerminals[_selectedProject.id];
              final selectedFiles = _selectedProject.id == null
                  ? null
                  : _projectFiles.putIfAbsent(
                      _selectedProject.id!,
                      ProjectFilesState.new,
                    );
              ProjectToolsPanel projectToolsPanel({required bool showFiles}) {
                final editorOwnsHorizontalPresentation =
                    _presentedTool == WorkspaceToolKind.editor &&
                    _terminalPresentation == TerminalPresentation.horizontal;
                return ProjectToolsPanel(
                  width: inspectorWidth,
                  projectId: _selectedProject.id,
                  terminal: selectedTerminal,
                  files: selectedFiles,
                  showFiles: showFiles,
                  onEnsureTerminal: _ensureSelectedProjectTerminal,
                  onEnsureFiles: () {
                    final projectId = _selectedProject.id;
                    if (projectId != null) {
                      return _loadProjectDirectory(projectId, '');
                    }
                    return Future.value();
                  },
                  presentation: editorOwnsHorizontalPresentation
                      ? TerminalPresentation.docked
                      : _terminalPresentation,
                  presentedTool: editorOwnsHorizontalPresentation
                      ? WorkspaceToolKind.terminal
                      : _presentedTool,
                  dockedTerminalExpanded: _dockedTerminalExpanded,
                  dockedFilesExpanded: _dockedFilesExpanded,
                  onToggleDocked: () => setState(
                    () => _dockedTerminalExpanded = !_dockedTerminalExpanded,
                  ),
                  onToggleFiles: () => setState(
                    () => _dockedFilesExpanded = !_dockedFilesExpanded,
                  ),
                  onPresentationChanged: _setTerminalPresentation,
                  onToolPresentationChanged: _setToolPresentation,
                  onToggleDirectory: (entry) {
                    final projectId = _selectedProject.id;
                    if (projectId != null) {
                      unawaited(_toggleProjectDirectory(projectId, entry));
                    }
                  },
                  onOpenFile: (entry) {
                    final projectId = _selectedProject.id;
                    if (projectId != null) {
                      unawaited(_openProjectFile(projectId, entry));
                    }
                  },
                  onRevealFile: (entry) =>
                      unawaited(_revealProjectEntry(entry)),
                  onBackToFiles: selectedFiles == null
                      ? null
                      : () => unawaited(_closeProjectFile(selectedFiles)),
                  onSaveFile: selectedFiles == null
                      ? null
                      : () => unawaited(_saveProjectFile(selectedFiles)),
                  onReloadFile:
                      selectedFiles == null || _selectedProject.id == null
                      ? null
                      : () => unawaited(
                          _reloadProjectFile(
                            _selectedProject.id!,
                            selectedFiles,
                          ),
                        ),
                  onOverwriteFile: selectedFiles == null
                      ? null
                      : () => unawaited(_overwriteProjectFile(selectedFiles)),
                  onRefreshFiles: () {
                    final projectId = _selectedProject.id;
                    if (projectId != null) {
                      unawaited(
                        _loadProjectDirectory(projectId, '', refresh: true),
                      );
                    }
                  },
                );
              }

              Widget expandedToolSurface({VoidCallback? onClose}) {
                if (_presentedTool == WorkspaceToolKind.editor &&
                    selectedFiles?.document != null) {
                  return ProjectFileEditorSurface(
                    files: selectedFiles!,
                    presentation: _terminalPresentation,
                    onBack: () => unawaited(_closeProjectFile(selectedFiles)),
                    onSave: () => unawaited(_saveProjectFile(selectedFiles)),
                    onReload: () => unawaited(
                      _reloadProjectFile(_selectedProject.id!, selectedFiles),
                    ),
                    onOverwrite: () =>
                        unawaited(_overwriteProjectFile(selectedFiles)),
                    onPresentationChanged: (value) =>
                        _setToolPresentation(WorkspaceToolKind.editor, value),
                    onClose: onClose,
                  );
                }
                return ProjectTerminalSurface(
                  terminal: selectedTerminal,
                  presentation: _terminalPresentation,
                  onTitleTap: () =>
                      _setTerminalPresentation(TerminalPresentation.docked),
                  onPresentationChanged: _setTerminalPresentation,
                  onClose: onClose,
                );
              }

              Widget standardWorkspace({
                required bool includeInspector,
                bool hideFileSurface = false,
              }) {
                return Row(
                  children: [
                    Expanded(child: agentsSurface),
                    if (showInspector && includeInspector) ...[
                      WorkspaceResizeHandle(
                        key: const Key('inspector-resize-handle'),
                        onDragUpdate: (delta) {
                          setState(() {
                            _inspectorWidth = (inspectorWidth - delta).clamp(
                              _minimumInspectorWidth,
                              _maximumInspectorWidth(
                                constraints.maxWidth,
                                showSidebar,
                              ),
                            );
                          });
                        },
                        onDragEnd: _persistPaneWidths,
                        onReset: () {
                          setState(
                            () => _inspectorWidth = _defaultInspectorWidth,
                          );
                          unawaited(_persistPaneWidths());
                        },
                      ),
                      projectToolsPanel(showFiles: !hideFileSurface),
                    ],
                  ],
                );
              }

              Widget terminalWorkspace() {
                final body = switch (_terminalPresentation) {
                  TerminalPresentation.horizontal
                      when _presentedTool == WorkspaceToolKind.editor =>
                    Column(
                      children: [
                        Expanded(
                          child: standardWorkspace(
                            includeInspector: true,
                            hideFileSurface: true,
                          ),
                        ),
                        const Divider(height: 1),
                        SizedBox(
                          height: (constraints.maxHeight * 0.34).clamp(
                            190.0,
                            340.0,
                          ),
                          child: expandedToolSurface(),
                        ),
                      ],
                    ),
                  TerminalPresentation.horizontal => Column(
                    children: [
                      SizedBox(
                        height: (constraints.maxHeight * 0.34).clamp(
                          190.0,
                          340.0,
                        ),
                        child: expandedToolSurface(),
                      ),
                      const Divider(height: 1),
                      Expanded(
                        child: standardWorkspace(includeInspector: false),
                      ),
                    ],
                  ),
                  TerminalPresentation.maximized => CallbackShortcuts(
                    bindings: {
                      const SingleActivator(LogicalKeyboardKey.escape):
                          _restoreMaximizedTerminal,
                    },
                    child: Focus(
                      autofocus: true,
                      child: expandedToolSurface(
                        onClose: _restoreMaximizedTerminal,
                      ),
                    ),
                  ),
                  _ => standardWorkspace(includeInspector: true),
                };
                return Row(
                  children: [
                    if (showSidebar) ...[
                      ProjectSidebar(
                        width: projectSidebarWidth,
                        projects: _projects,
                        selectedIndex: _selectedProjectIndex,
                        onAddProject: _addProject,
                        onSelectProject: _selectProject,
                        onRevealProject: (project) =>
                            unawaited(_revealProjectInFinder(project)),
                        onCopyProjectPath: (project) =>
                            unawaited(_copyProjectPath(project)),
                        onDeleteProject: (project) =>
                            unawaited(_deleteProject(project)),
                        summaryForProject: (project) => summarizeProjectAgents(
                          sessions: _agentSessions,
                          attention: _attention,
                          projectId: project.id,
                          readAttentionIds: _readAttentionIds,
                        ),
                      ),
                      WorkspaceResizeHandle(
                        key: const Key('projects-resize-handle'),
                        onDragUpdate: (delta) {
                          setState(() {
                            _projectSidebarWidth = (projectSidebarWidth + delta)
                                .clamp(
                                  _minimumProjectSidebarWidth,
                                  _maximumProjectSidebarWidth(
                                    constraints.maxWidth,
                                    showInspector,
                                  ),
                                );
                          });
                        },
                        onDragEnd: _persistPaneWidths,
                        onReset: () {
                          setState(
                            () => _projectSidebarWidth =
                                _defaultProjectSidebarWidth,
                          );
                          unawaited(_persistPaneWidths());
                        },
                      ),
                    ],
                    Expanded(child: body),
                  ],
                );
              }

              return ColoredBox(
                color: context.ditch.workspace,
                child: Column(
                  children: [
                    DitchToolbar(
                      projectName: _selectedProject.name,
                      connection: presentation.connection,
                      notifications: _attention,
                      unreadNotificationCount: _unreadNotificationCount,
                      sidebarVisible: showSidebar,
                      inspectorVisible: showInspector,
                      onNotificationsViewed: _markNotificationsRead,
                      onOpenNotification: _openAttentionSession,
                      onDismissNotification: _dismissAttention,
                      onDismissAllNotifications: _dismissAllNotifications,
                      onOpenCodexSettings: _openAppSettings,
                      codexAvailable: _codexBinary != null,
                      onToggleSidebar: _presentation.toggleSidebar,
                      onToggleInspector: _presentation.toggleInspector,
                    ),
                    if (_codexReadiness != null &&
                        _codexReadiness!.ready == false)
                      CodexSetupBanner(
                        report: _codexReadiness!,
                        onRepair: _repairCodexSetup,
                        onCheckAgain: _refreshCodexReadiness,
                      ),
                    if ((_notificationReadiness != null &&
                            _notificationReadiness!.ready == false) ||
                        _notificationSetupError != null)
                      NotificationSetupBanner(
                        readiness:
                            _notificationReadiness ??
                            const NotificationReadiness(
                              authorization:
                                  NotificationAuthorizationState.unknown,
                              alertsEnabled: false,
                              notificationCenterEnabled: false,
                              soundsEnabled: false,
                            ),
                        checking: _notificationSetupChecking,
                        error: _notificationSetupError,
                        onManage: _manageNotificationSetup,
                        onCheckAgain: _refreshNotificationReadiness,
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
                          : terminalWorkspace(),
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

class CodexOnboardingView extends StatelessWidget {
  const CodexOnboardingView({
    required this.introduced,
    required this.checking,
    required this.updating,
    required this.readiness,
    required this.error,
    required this.notificationReadiness,
    required this.notificationChecking,
    required this.notificationError,
    required this.onContinue,
    required this.onCheckAgain,
    required this.onChooseInstallation,
    required this.onUpdate,
    required this.onSignIn,
    required this.onOpenInstallInstructions,
    required this.onManageNotifications,
    required this.onCheckNotifications,
    required this.onAddProject,
    super.key,
  });

  final bool introduced;
  final bool checking;
  final bool updating;
  final CodexReadinessReport? readiness;
  final String? error;
  final NotificationReadiness? notificationReadiness;
  final bool notificationChecking;
  final String? notificationError;
  final VoidCallback onContinue;
  final VoidCallback onCheckAgain;
  final VoidCallback onChooseInstallation;
  final VoidCallback onUpdate;
  final VoidCallback onSignIn;
  final VoidCallback onOpenInstallInstructions;
  final VoidCallback onManageNotifications;
  final VoidCallback onCheckNotifications;
  final VoidCallback onAddProject;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ColoredBox(
      color: context.ditch.workspace,
      child: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(32),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 620),
            child: Card(
              child: Padding(
                padding: const EdgeInsets.all(32),
                child: !introduced
                    ? Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Icon(
                            Icons.hub_outlined,
                            size: 46,
                            color: theme.colorScheme.primary,
                          ),
                          const SizedBox(height: 20),
                          Text(
                            'Welcome to Ditch',
                            style: theme.textTheme.headlineSmall,
                          ),
                          const SizedBox(height: 10),
                          const Text(
                            'Ditch runs your existing Codex CLI. First, it will verify the installation, required capabilities, authentication, and diagnostics. It will not create a private Codex installation or change your PATH.',
                            textAlign: TextAlign.center,
                          ),
                          const SizedBox(height: 24),
                          FilledButton(
                            key: const Key('onboarding-continue'),
                            onPressed: onContinue,
                            child: const Text('Check Codex Setup'),
                          ),
                        ],
                      )
                    : _buildSetup(context),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildSetup(BuildContext context) {
    final report = readiness;
    final busy = checking || updating || notificationChecking;
    if (report == null && error == null) {
      return const Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          CircularProgressIndicator(),
          SizedBox(height: 18),
          Text('Checking your Codex setup…'),
        ],
      );
    }
    final theme = Theme.of(context);
    final ready = report?.ready == true;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          ready ? 'Codex is ready' : 'Finish Codex setup',
          style: theme.textTheme.headlineSmall,
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: 22),
        _SetupCheckRow(
          label: 'Installation found',
          complete: report?.path != null,
          detail: report?.path ?? 'No Codex CLI found',
        ),
        _SetupCheckRow(
          label: 'Compatible CLI',
          complete: report?.compatible == true,
          detail: report?.version ?? 'Version unavailable',
        ),
        _SetupCheckRow(
          label: 'Signed in',
          complete: report?.authenticated == true,
          detail: report?.authenticated == true
              ? 'Codex authentication is active'
              : 'Sign in is required',
        ),
        if (report?.doctorSupported == true)
          const _SetupCheckRow(
            label: 'Diagnostics',
            complete: true,
            detail: 'Codex diagnostic command is available',
          ),
        if (ready) ...[
          _SetupCheckRow(
            label: 'Notifications',
            complete: notificationReadiness?.ready == true,
            detail: _notificationDetail,
          ),
          if (notificationError != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: SelectableText(
                notificationError!,
                style: TextStyle(color: theme.colorScheme.error),
              ),
            ),
        ],
        if (report != null && report.issues.isNotEmpty) ...[
          const SizedBox(height: 12),
          ...report.issues.map(
            (issue) => Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Text(
                issue,
                style: TextStyle(color: theme.colorScheme.error),
              ),
            ),
          ),
        ],
        if (error != null) ...[
          const SizedBox(height: 10),
          SelectableText(
            error!,
            style: TextStyle(color: theme.colorScheme.error),
          ),
        ],
        if (busy) ...[
          const SizedBox(height: 18),
          const LinearProgressIndicator(),
          const SizedBox(height: 8),
          Text(updating ? 'Updating Codex…' : 'Checking Codex…'),
        ],
        const SizedBox(height: 24),
        Wrap(
          alignment: WrapAlignment.end,
          spacing: 10,
          runSpacing: 10,
          children: [
            if (!ready)
              OutlinedButton(
                onPressed: busy ? null : onCheckAgain,
                child: const Text('Check Again'),
              ),
            if (report?.path == null)
              OutlinedButton(
                onPressed: busy ? null : onOpenInstallInstructions,
                child: const Text('Codex Install Instructions'),
              ),
            if (report != null &&
                report.path != null &&
                !report.compatible &&
                !report.updateSupported)
              OutlinedButton(
                onPressed: busy ? null : onOpenInstallInstructions,
                child: const Text('Update Instructions'),
              ),
            if (report?.path != null && !ready)
              OutlinedButton(
                onPressed: busy ? null : onChooseInstallation,
                child: const Text('Choose Installation'),
              ),
            if (report != null && !report.compatible && report.updateSupported)
              FilledButton.tonal(
                onPressed: busy ? null : onUpdate,
                child: const Text('Update Codex'),
              ),
            if (report != null && report.compatible && !report.authenticated)
              FilledButton.tonal(
                onPressed: busy ? null : onSignIn,
                child: const Text('Sign In to Codex'),
              ),
            if (ready && notificationReadiness?.ready != true)
              FilledButton.tonalIcon(
                key: const Key('enable-notifications'),
                onPressed: busy ? null : onManageNotifications,
                icon: const Icon(Icons.notifications_active_outlined),
                label: Text(
                  notificationReadiness?.authorization ==
                          NotificationAuthorizationState.denied
                      ? 'Open Notification Settings'
                      : 'Enable Notifications',
                ),
              ),
            if (ready &&
                notificationReadiness != null &&
                notificationReadiness?.ready != true)
              OutlinedButton(
                onPressed: busy ? null : onCheckNotifications,
                child: const Text('Check Notifications Again'),
              ),
            if (ready && notificationReadiness?.ready == true)
              FilledButton.icon(
                key: const Key('first-project-add'),
                onPressed: onAddProject,
                icon: const Icon(Icons.add),
                label: const Text('Add Your First Project'),
              ),
          ],
        ),
      ],
    );
  }

  String get _notificationDetail {
    final value = notificationReadiness;
    if (value == null) {
      return notificationChecking
          ? 'Checking Ditch Runtime…'
          : 'Notification status has not been checked';
    }
    if (value.ready) {
      return value.soundsEnabled
          ? 'Alerts and sounds are enabled'
          : 'Alerts are enabled; sounds are disabled';
    }
    return switch (value.authorization) {
      NotificationAuthorizationState.denied =>
        'Permission was denied. Enable Ditch in System Settings.',
      NotificationAuthorizationState.authorized ||
      NotificationAuthorizationState.provisional ||
      NotificationAuthorizationState.ephemeral =>
        'Permission exists, but alerts are disabled in System Settings.',
      NotificationAuthorizationState.notDetermined =>
        'Allow the runtime helper to notify you when agents finish.',
      NotificationAuthorizationState.unknown =>
        'The runtime helper returned an unknown notification status.',
    };
  }
}

class _SetupCheckRow extends StatelessWidget {
  const _SetupCheckRow({
    required this.label,
    required this.complete,
    required this.detail,
  });

  final String label;
  final bool complete;
  final String detail;

  @override
  Widget build(BuildContext context) {
    final color = complete
        ? context.ditch.success
        : Theme.of(context).colorScheme.error;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(
            complete ? Icons.check_circle : Icons.cancel_outlined,
            color: color,
            size: 20,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  label,
                  style: const TextStyle(fontWeight: FontWeight.w600),
                ),
                Text(detail, style: Theme.of(context).textTheme.bodySmall),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class CodexSetupBanner extends StatelessWidget {
  const CodexSetupBanner({
    required this.report,
    required this.onRepair,
    required this.onCheckAgain,
    super.key,
  });

  final CodexReadinessReport report;
  final VoidCallback onRepair;
  final VoidCallback onCheckAgain;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Theme.of(context).colorScheme.errorContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        child: Row(
          children: [
            Icon(
              Icons.warning_amber_rounded,
              color: Theme.of(context).colorScheme.onErrorContainer,
            ),
            const SizedBox(width: 10),
            const Expanded(
              child: Text(
                'Codex setup needs attention. New agents are disabled until it is repaired.',
              ),
            ),
            TextButton(onPressed: onCheckAgain, child: const Text('Recheck')),
            FilledButton.tonal(
              onPressed: onRepair,
              child: const Text('Repair'),
            ),
          ],
        ),
      ),
    );
  }
}

class NotificationSetupBanner extends StatelessWidget {
  const NotificationSetupBanner({
    required this.readiness,
    required this.checking,
    required this.error,
    required this.onManage,
    required this.onCheckAgain,
    super.key,
  });

  final NotificationReadiness readiness;
  final bool checking;
  final String? error;
  final VoidCallback onManage;
  final VoidCallback onCheckAgain;

  @override
  Widget build(BuildContext context) {
    final denied =
        readiness.authorization == NotificationAuthorizationState.denied;
    return Material(
      color: Theme.of(context).colorScheme.secondaryContainer,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
        child: Row(
          children: [
            Icon(
              Icons.notifications_off_outlined,
              color: Theme.of(context).colorScheme.onSecondaryContainer,
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                error ??
                    'Enable macOS notifications to know when an agent completes, fails, or needs attention.',
              ),
            ),
            TextButton(
              onPressed: checking ? null : onCheckAgain,
              child: const Text('Recheck'),
            ),
            FilledButton.tonal(
              key: const Key('notification-setup-action'),
              onPressed: checking ? null : onManage,
              child: Text(denied ? 'Open Settings' : 'Enable Notifications'),
            ),
          ],
        ),
      ),
    );
  }
}

class RemoteSettingsDialog extends StatefulWidget {
  const RemoteSettingsDialog({required this.client, super.key});

  final DitchRuntimeClient client;

  @override
  State<RemoteSettingsDialog> createState() => _RemoteSettingsDialogState();
}

class _RemoteSettingsDialogState extends State<RemoteSettingsDialog> {
  Map<String, dynamic>? _status;
  Map<String, dynamic>? _pairing;
  Timer? _timer;
  bool _busy = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_refresh());
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      setState(() {});
      if (_pairing?['state'] == 'pending' || _pairing?['state'] == 'claimed') {
        unawaited(_pollPairing());
      }
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _refresh() async {
    if (mounted) {
      setState(() {
        _status = null;
        _error = null;
      });
    }
    try {
      final response = await widget.client.remoteControlStatus();
      if (!mounted) return;
      setState(() {
        _status = (response['RemoteControlStatus'] as Map?)
            ?.cast<String, dynamic>();
        _error = null;
      });
    } on Object catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<void> _connect() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final response = await widget.client.createRemotePairing();
      if (!mounted) return;
      setState(() {
        _pairing = (response['RemotePairing'] as Map?)?.cast<String, dynamic>();
        _busy = false;
      });
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = '$error';
        });
      }
    }
  }

  Future<void> _pollPairing() async {
    final pairingId = _pairing?['pairing_id']?.toString();
    if (pairingId == null || _busy) return;
    try {
      final response = await widget.client.getRemotePairing(pairingId);
      if (!mounted) return;
      final next = (response['RemotePairing'] as Map?)?.cast<String, dynamic>();
      if (next != null) {
        // The one-time QR payload exists only in memory on initial creation.
        next['qr_payload'] ??= _pairing?['qr_payload'];
        setState(() => _pairing = next);
      }
    } on Object catch (error) {
      if (mounted) setState(() => _error = '$error');
    }
  }

  Future<void> _confirm() async {
    final pairingId = _pairing?['pairing_id']?.toString();
    if (pairingId == null) return;
    setState(() => _busy = true);
    try {
      final response = await widget.client.confirmRemotePairing(pairingId);
      if (!mounted) return;
      setState(() {
        _pairing = (response['RemotePairing'] as Map?)?.cast<String, dynamic>();
        _busy = false;
      });
      await _refresh();
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = '$error';
        });
      }
    }
  }

  Future<void> _cancelPairing() async {
    final pairingId = _pairing?['pairing_id']?.toString();
    if (pairingId != null) await widget.client.cancelRemotePairing(pairingId);
    if (mounted) setState(() => _pairing = null);
  }

  Future<void> _revoke(String id, String name) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Revoke this iPhone?'),
        content: Text('$name will immediately lose remote access.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Revoke'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    await widget.client.revokeRemoteDevice(id);
    await _refresh();
  }

  Future<void> _disable() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Disable Remote Control?'),
        content: const Text(
          'Connected phones will lose access. Local projects and sessions will keep working.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Disable'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;
    await widget.client.disableRemoteControl();
    await _refresh();
  }

  @override
  Widget build(BuildContext context) {
    final status = _status;
    final devices =
        (status?['devices'] as List?)?.whereType<Map>().toList() ?? const [];
    return AlertDialog(
      icon: const Icon(Icons.phone_iphone),
      title: const Text('Remote Control'),
      content: SizedBox(
        width: 580,
        child: status == null
            ? _error == null
                  ? const Center(child: CircularProgressIndicator())
                  : Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          'iPhones with access could not be refreshed.',
                          style: Theme.of(context).textTheme.titleSmall,
                        ),
                        const SizedBox(height: 8),
                        SelectableText(
                          _error!,
                          style: TextStyle(
                            color: Theme.of(context).colorScheme.error,
                          ),
                        ),
                        const SizedBox(height: 12),
                        OutlinedButton(
                          onPressed: _refresh,
                          child: const Text('Try again'),
                        ),
                      ],
                    )
            : SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    SwitchListTile(
                      contentPadding: EdgeInsets.zero,
                      title: const Text('Mobile Control'),
                      subtitle: Text(
                        status?['enabled'] == true
                            ? 'This Mac accepts encrypted Ditch commands.'
                            : 'Local Ditch works without Remote Control.',
                      ),
                      value: status?['enabled'] == true,
                      onChanged: null,
                    ),
                    ListTile(
                      contentPadding: EdgeInsets.zero,
                      leading: Icon(
                        status?['online'] == true
                            ? Icons.cloud_done_outlined
                            : Icons.cloud_off_outlined,
                      ),
                      title: Text(
                        status?['machine_name']?.toString() ?? 'This Mac',
                      ),
                      subtitle: Text(
                        status?['online'] == true ? 'Online' : 'Offline',
                      ),
                    ),
                    const Divider(),
                    Text(
                      'iPhones with access to this Mac',
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                    if (devices.isEmpty)
                      const Padding(
                        padding: EdgeInsets.symmetric(vertical: 12),
                        child: Text('No iPhones have access to this Mac.'),
                      ),
                    ...devices.map((raw) {
                      final device = raw.cast<Object?, Object?>();
                      final name = device['name']?.toString() ?? 'iPhone';
                      final state = device['state']?.toString() ?? 'unknown';
                      final lastSeen = device['last_seen_at']?.toString();
                      return ListTile(
                        contentPadding: EdgeInsets.zero,
                        leading: const Icon(Icons.phone_iphone),
                        title: Text(name),
                        subtitle: Text(
                          state == 'active'
                              ? lastSeen == null
                                    ? 'Paired'
                                    : 'Paired · Last seen $lastSeen'
                              : 'Device revoked',
                        ),
                        trailing: state == 'active'
                            ? TextButton(
                                onPressed: _busy
                                    ? null
                                    : () => _revoke(
                                        device['device_id'].toString(),
                                        name,
                                      ),
                                child: const Text('Revoke'),
                              )
                            : null,
                      );
                    }),
                    if (_pairing != null) ...[
                      const Divider(),
                      _buildPairing(context),
                    ],
                    if (_error != null) ...[
                      const SizedBox(height: 10),
                      SelectableText(
                        _error!,
                        style: TextStyle(
                          color: Theme.of(context).colorScheme.error,
                        ),
                      ),
                    ],
                    const SizedBox(height: 12),
                    const Text(
                      'Source code, terminal output, Git worktrees, secrets, and full transcripts stay on this Mac. Anonymous identity and sanitized status metadata are stored by the remote relay; sensitive command and transcript payloads are relayed encrypted.',
                    ),
                  ],
                ),
              ),
      ),
      actions: [
        if (status?['enabled'] == true)
          TextButton(
            onPressed: _busy ? null : _disable,
            child: const Text('Disable Remote Control'),
          ),
        if (_pairing == null)
          FilledButton.icon(
            key: const Key('connect-iphone'),
            onPressed: _busy || status?['configured'] != true ? null : _connect,
            icon: const Icon(Icons.qr_code),
            label: const Text('Connect iPhone'),
          ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Close'),
        ),
      ],
    );
  }

  Widget _buildPairing(BuildContext context) {
    final state = _pairing?['state']?.toString() ?? 'pending';
    final expiry = DateTime.tryParse(_pairing?['expires_at']?.toString() ?? '');
    final remaining =
        expiry?.difference(DateTime.now().toUtc()) ?? Duration.zero;
    if (state == 'consumed') {
      return const ListTile(
        leading: Icon(Icons.check_circle_outline),
        title: Text('iPhone connected'),
      );
    }
    if (state == 'claimed') {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            '${_pairing?['pending_device_name'] ?? 'An iPhone'} wants to connect',
            style: Theme.of(context).textTheme.titleMedium,
          ),
          const SizedBox(height: 8),
          const Text(
            'Confirm only if this is the iPhone you just scanned with.',
          ),
          const SizedBox(height: 12),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              TextButton(
                onPressed: _busy ? null : _cancelPairing,
                child: const Text('Cancel'),
              ),
              const SizedBox(width: 8),
              FilledButton(
                onPressed: _busy ? null : _confirm,
                child: const Text('Connect'),
              ),
            ],
          ),
        ],
      );
    }
    final qr = _pairing?['qr_payload']?.toString();
    return Column(
      children: [
        Text(
          'Scan this with The Ditch on iPhone',
          style: Theme.of(context).textTheme.titleMedium,
        ),
        const SizedBox(height: 12),
        if (qr != null)
          ColoredBox(
            color: Colors.white,
            child: Padding(
              padding: const EdgeInsets.all(12),
              child: QrImageView(
                data: qr,
                size: 220,
                semanticsLabel: 'One-time Ditch iPhone pairing code',
              ),
            ),
          ),
        const SizedBox(height: 10),
        Text(
          'Expires in ${remaining.isNegative ? '0:00' : '${remaining.inMinutes}:${(remaining.inSeconds % 60).toString().padLeft(2, '0')}'}',
        ),
        TextButton(
          onPressed: _busy ? null : _cancelPairing,
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

class DitchToolbar extends StatelessWidget {
  const DitchToolbar({
    required this.projectName,
    required this.connection,
    required this.notifications,
    required this.unreadNotificationCount,
    required this.sidebarVisible,
    required this.inspectorVisible,
    required this.onToggleSidebar,
    required this.onToggleInspector,
    required this.onNotificationsViewed,
    required this.onOpenNotification,
    required this.onDismissNotification,
    required this.onDismissAllNotifications,
    required this.onOpenCodexSettings,
    required this.codexAvailable,
    super.key,
  });

  final String projectName;
  final RuntimeConnectionPhase connection;
  final List<AttentionEvent> notifications;
  final int unreadNotificationCount;
  final bool sidebarVisible;
  final bool inspectorVisible;
  final VoidCallback onToggleSidebar;
  final VoidCallback onToggleInspector;
  final VoidCallback onNotificationsViewed;
  final ValueChanged<AttentionEvent> onOpenNotification;
  final ValueChanged<AttentionEvent> onDismissNotification;
  final VoidCallback onDismissAllNotifications;
  final VoidCallback onOpenCodexSettings;
  final bool codexAvailable;

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;
    final (label, color) = switch (connection) {
      RuntimeConnectionPhase.connected => ('Connected', tokens.success),
      RuntimeConnectionPhase.connecting => ('Connecting', tokens.waiting),
      RuntimeConnectionPhase.reconnecting => ('Reconnecting', tokens.waiting),
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
            const SizedBox(width: 6),
            PopupMenuButton<ThemeMode>(
              key: const Key('theme-mode-menu'),
              tooltip: 'Appearance',
              initialValue: ditchThemeMode.value,
              onSelected: (mode) => unawaited(setDitchThemeMode(mode)),
              icon: Icon(
                Theme.of(context).brightness == Brightness.dark
                    ? Icons.dark_mode_outlined
                    : Icons.light_mode_outlined,
              ),
              itemBuilder: (context) => const [
                PopupMenuItem(
                  value: ThemeMode.system,
                  child: Text('System appearance'),
                ),
                PopupMenuItem(value: ThemeMode.light, child: Text('Light')),
                PopupMenuItem(value: ThemeMode.dark, child: Text('Dark')),
              ],
            ),
            IconButton(
              tooltip: codexAvailable
                  ? 'Codex installation'
                  : 'Codex not found — choose an installation',
              onPressed: onOpenCodexSettings,
              icon: Icon(
                codexAvailable
                    ? Icons.settings_outlined
                    : Icons.warning_amber_rounded,
                color: codexAvailable
                    ? null
                    : Theme.of(context).colorScheme.error,
              ),
            ),
            NotificationCenterButton(
              notifications: notifications,
              unreadCount: unreadNotificationCount,
              onViewed: onNotificationsViewed,
              onOpen: onOpenNotification,
              onDismiss: onDismissNotification,
              onDismissAll: onDismissAllNotifications,
            ),
            IconButton(
              tooltip: inspectorVisible ? 'Hide terminal' : 'Show terminal',
              onPressed: onToggleInspector,
              icon: Icon(
                inspectorVisible
                    ? Icons.vertical_split
                    : Icons.vertical_split_outlined,
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
                    ? 'Ditch Runtime is not responding. Agents may still be running.'
                    : 'Ditch Runtime is not responding. Agents may still be running. $message',
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
                  'Ditch Runtime is not responding',
                  style: Theme.of(context).textTheme.headlineSmall,
                ),
                const SizedBox(height: 8),
                const Text(
                  'Agent processes may still be running. Ditch could not reconnect to its local runtime service.',
                ),
                const SizedBox(height: 20),
                Text(
                  'Expected process',
                  style: Theme.of(context).textTheme.labelLarge,
                ),
                const SizedBox(height: 4),
                const SelectableText('Ditch Runtime'),
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
                  'Search for “Ditch Runtime” in Activity Monitor. Force Quit Applications does not list macOS background services.',
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

class WorkspaceResizeHandle extends StatefulWidget {
  const WorkspaceResizeHandle({
    required this.onDragUpdate,
    required this.onDragEnd,
    required this.onReset,
    super.key,
  });

  final ValueChanged<double> onDragUpdate;
  final Future<void> Function() onDragEnd;
  final VoidCallback onReset;

  @override
  State<WorkspaceResizeHandle> createState() => _WorkspaceResizeHandleState();
}

class _WorkspaceResizeHandleState extends State<WorkspaceResizeHandle> {
  bool _active = false;

  void _setActive(bool value) {
    if (_active == value) return;
    setState(() => _active = value);
  }

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.resizeLeftRight,
      onEnter: (_) => _setActive(true),
      onExit: (_) => _setActive(false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onDoubleTap: widget.onReset,
        onHorizontalDragStart: (_) => _setActive(true),
        onHorizontalDragUpdate: (details) =>
            widget.onDragUpdate(details.delta.dx),
        onHorizontalDragEnd: (_) {
          _setActive(false);
          unawaited(widget.onDragEnd());
        },
        child: Semantics(
          label: 'Resize pane',
          child: SizedBox(
            width: _CommandCenterScreenState._splitterExtent,
            child: Center(
              child: AnimatedContainer(
                duration: MediaQuery.disableAnimationsOf(context)
                    ? const Duration(milliseconds: 1)
                    : const Duration(milliseconds: 170),
                width: _active ? 2 : 1,
                color: _active ? context.ditch.accent : context.ditch.separator,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class NotificationCenterButton extends StatefulWidget {
  const NotificationCenterButton({
    required this.notifications,
    required this.unreadCount,
    required this.onViewed,
    required this.onOpen,
    required this.onDismiss,
    required this.onDismissAll,
    super.key,
  });

  final List<AttentionEvent> notifications;
  final int unreadCount;
  final VoidCallback onViewed;
  final ValueChanged<AttentionEvent> onOpen;
  final ValueChanged<AttentionEvent> onDismiss;
  final VoidCallback onDismissAll;

  @override
  State<NotificationCenterButton> createState() =>
      _NotificationCenterButtonState();
}

class _NotificationCenterButtonState extends State<NotificationCenterButton> {
  final _controller = MenuController();

  void _toggle() {
    if (_controller.isOpen) {
      _controller.close();
    } else {
      widget.onViewed();
      _controller.open();
    }
  }

  @override
  Widget build(BuildContext context) {
    final notifications = List<AttentionEvent>.from(widget.notifications)
      ..sort((a, b) => b.createdAt.compareTo(a.createdAt));
    return MenuAnchor(
      controller: _controller,
      alignmentOffset: const Offset(-340, 4),
      style: MenuStyle(
        padding: const WidgetStatePropertyAll(EdgeInsets.zero),
        backgroundColor: WidgetStatePropertyAll(context.ditch.surface),
      ),
      menuChildren: [
        SizedBox(
          key: const Key('notification-center'),
          width: 380,
          height: notifications.isEmpty ? 150 : 430,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 12, 8, 8),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        'Notifications',
                        style: Theme.of(context).textTheme.titleMedium,
                      ),
                    ),
                    if (notifications.isNotEmpty)
                      TextButton(
                        key: const Key('clear-notifications'),
                        onPressed: widget.onDismissAll,
                        child: const Text('Clear all'),
                      ),
                  ],
                ),
              ),
              const Divider(height: 1),
              Expanded(
                child: notifications.isEmpty
                    ? const Center(child: Text('No notifications'))
                    : ListView.separated(
                        primary: false,
                        padding: const EdgeInsets.all(10),
                        itemCount: notifications.length,
                        separatorBuilder: (_, _) => const SizedBox(height: 8),
                        itemBuilder: (context, index) {
                          final event = notifications[index];
                          return _NotificationCenterItem(
                            event: event,
                            onOpen: event.canOpenSession
                                ? () {
                                    _controller.close();
                                    widget.onOpen(event);
                                  }
                                : null,
                            onDismiss: () => widget.onDismiss(event),
                          );
                        },
                      ),
              ),
            ],
          ),
        ),
      ],
      builder: (context, controller, _) => Badge(
        isLabelVisible: widget.unreadCount > 0,
        label: Text('${widget.unreadCount}'),
        child: IconButton(
          key: const Key('notification-bell'),
          tooltip: 'Notifications',
          onPressed: _toggle,
          icon: Icon(
            controller.isOpen
                ? Icons.notifications
                : Icons.notifications_outlined,
          ),
        ),
      ),
    );
  }
}

class _NotificationCenterItem extends StatelessWidget {
  const _NotificationCenterItem({
    required this.event,
    required this.onDismiss,
    this.onOpen,
  });

  final AttentionEvent event;
  final VoidCallback? onOpen;
  final VoidCallback onDismiss;

  @override
  Widget build(BuildContext context) {
    final project = event.projectName?.trim();
    final agent = event.agentName?.trim();
    final source = [
      if (project != null && project.isNotEmpty) project,
      if (agent != null && agent.isNotEmpty) agent,
    ].join(' · ');
    return DitchSurface(
      padding: const EdgeInsets.all(12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(event.icon, size: 19),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  event.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.titleSmall,
                ),
                if (source.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(
                    source,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: Theme.of(context).textTheme.labelSmall,
                  ),
                ],
                const SizedBox(height: 5),
                Text(event.body, maxLines: 3, overflow: TextOverflow.ellipsis),
                const SizedBox(height: 8),
                Row(
                  children: [
                    if (onOpen != null)
                      TextButton.icon(
                        onPressed: onOpen,
                        icon: const Icon(Icons.open_in_full, size: 15),
                        label: const Text('Open'),
                      ),
                    const Spacer(),
                    IconButton(
                      tooltip: 'Dismiss notification',
                      visualDensity: VisualDensity.compact,
                      onPressed: onDismiss,
                      icon: const Icon(Icons.close, size: 16),
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

class ProjectSidebar extends StatelessWidget {
  const ProjectSidebar({
    required this.width,
    required this.projects,
    required this.selectedIndex,
    required this.onAddProject,
    required this.onSelectProject,
    required this.onRevealProject,
    required this.onCopyProjectPath,
    required this.onDeleteProject,
    required this.summaryForProject,
    super.key,
  });

  final double width;
  final List<DitchProject> projects;
  final int selectedIndex;
  final VoidCallback onAddProject;
  final ValueChanged<int> onSelectProject;
  final ValueChanged<DitchProject> onRevealProject;
  final ValueChanged<DitchProject> onCopyProjectPath;
  final ValueChanged<DitchProject> onDeleteProject;
  final ProjectAgentSummary Function(DitchProject) summaryForProject;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ColoredBox(
      color: context.ditch.sidebar,
      child: SizedBox(
        width: width,
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
                      final summary = summaryForProject(project);
                      return ProjectTile(
                        name: project.name,
                        path: project.path,
                        selected: index == selectedIndex,
                        onTap: () => onSelectProject(index),
                        onReveal: () => onRevealProject(project),
                        onCopyPath: () => onCopyProjectPath(project),
                        onDelete: () => onDeleteProject(project),
                        runningCount: summary.runningCount,
                        stoppedCount: summary.stoppedCount,
                        hasUnreadResult: summary.hasUnreadResult,
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
    required this.onReveal,
    required this.onCopyPath,
    required this.onDelete,
    this.runningCount = 0,
    this.stoppedCount = 0,
    this.hasUnreadResult = false,
    super.key,
  });

  final String name;
  final String path;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onReveal;
  final VoidCallback onCopyPath;
  final VoidCallback onDelete;
  final int runningCount;
  final int stoppedCount;
  final bool hasUnreadResult;

  Future<void> _showContextMenu(
    BuildContext context,
    TapDownDetails details,
  ) async {
    final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
    final action = await showMenu<_ProjectMenuAction>(
      context: context,
      position: RelativeRect.fromRect(
        Rect.fromLTWH(
          details.globalPosition.dx,
          details.globalPosition.dy,
          0,
          0,
        ),
        Offset.zero & overlay.size,
      ),
      items: const [
        PopupMenuItem(
          value: _ProjectMenuAction.copyPath,
          child: Text('Copy Project Path'),
        ),
        PopupMenuDivider(),
        PopupMenuItem(value: _ProjectMenuAction.delete, child: Text('Delete')),
      ],
    );
    switch (action) {
      case _ProjectMenuAction.copyPath:
        onCopyPath();
      case _ProjectMenuAction.delete:
        onDelete();
      case null:
        break;
    }
  }

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;

    return InkWell(
      key: ValueKey('project-tile-$path'),
      borderRadius: BorderRadius.circular(tokens.radiusSmall),
      onTap: onTap,
      onSecondaryTapDown: (details) => _showContextMenu(context, details),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: selected ? tokens.selection : Colors.transparent,
          borderRadius: BorderRadius.circular(tokens.radiusSmall),
        ),
        child: Padding(
          padding: const EdgeInsets.all(10),
          child: Row(
            children: [
              const Icon(Icons.folder_outlined, size: 18),
              const SizedBox(width: 8),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                        if (hasUnreadResult)
                          Tooltip(
                            message: 'New agent result',
                            child: Container(
                              key: ValueKey('project-unread-$path'),
                              width: 8,
                              height: 8,
                              decoration: BoxDecoration(
                                color: context.ditch.accent,
                                shape: BoxShape.circle,
                              ),
                            ),
                          ),
                      ],
                    ),
                    Text(
                      path,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                    const SizedBox(height: 3),
                    Text(
                      '$runningCount running · $stoppedCount stopped',
                      key: ValueKey('project-counts-$path'),
                      style: Theme.of(context).textTheme.labelSmall?.copyWith(
                        color: context.ditch.mutedText,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 4),
              IconButton(
                key: ValueKey('reveal-project-$path'),
                onPressed: onReveal,
                tooltip: 'Show in Finder',
                visualDensity: VisualDensity.compact,
                constraints: const BoxConstraints.tightFor(
                  width: 28,
                  height: 28,
                ),
                padding: EdgeInsets.zero,
                icon: const Icon(Icons.folder_open_outlined, size: 16),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

enum _ProjectMenuAction { copyPath, delete }

class AgentsSurface extends StatelessWidget {
  const AgentsSurface({
    required this.sessions,
    required this.expandedAgentLocalId,
    required this.focusedAgentLocalId,
    required this.chatViewport,
    required this.agentListController,
    required this.composerKey,
    this.composerKeyForAgent,
    required this.headerKeyForAgent,
    required this.initialPrompt,
    this.effectiveCodexHome,
    required this.onStartCodex,
    this.onStartPrompt,
    required this.onSubmitPrompt,
    this.onLoadMessages = _ignoreAgentSession,
    required this.onStopCodex,
    required this.onDeleteAgent,
    required this.onRenameAgent,
    this.hasUnreadResult = _neverUnreadAgentResult,
    required this.onFocusAgent,
    required this.onToggleExpanded,
    super.key,
  });

  final List<AgentSession> sessions;
  final String? expandedAgentLocalId;
  final String? focusedAgentLocalId;
  final ConversationViewportController chatViewport;
  final ScrollController agentListController;
  final GlobalKey<AgentComposerState> composerKey;
  final GlobalKey<AgentComposerState> Function(String agentId)?
  composerKeyForAgent;
  final Key Function(String agentId) headerKeyForAgent;
  final String initialPrompt;
  final String? effectiveCodexHome;
  final VoidCallback onStartCodex;
  final ValueChanged<String>? onStartPrompt;
  final void Function(AgentSession session, String prompt) onSubmitPrompt;
  final ValueChanged<AgentSession> onLoadMessages;
  final ValueChanged<AgentSession> onStopCodex;
  final ValueChanged<AgentSession> onDeleteAgent;
  final void Function(AgentSession session, String? title) onRenameAgent;
  final bool Function(AgentSession session) hasUnreadResult;
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
              Row(
                children: [
                  Expanded(
                    child: Text(
                      'Agents',
                      style: Theme.of(context).textTheme.headlineSmall,
                    ),
                  ),
                  FilledButton.icon(
                    key: const Key('agents-new-agent-button'),
                    onPressed: onStartCodex,
                    icon: const Icon(Icons.add, size: 16),
                    label: const Text('New Agent'),
                  ),
                ],
              ),
              const SizedBox(height: 12),
            ],
            Expanded(
              child: AgentSessionList(
                sessions: sessions,
                expandedAgentLocalId: expandedAgentLocalId,
                focusedAgentLocalId: focusedAgentLocalId,
                chatViewport: chatViewport,
                agentListController: agentListController,
                composerKey: composerKey,
                composerKeyForAgent: composerKeyForAgent,
                headerKeyForAgent: headerKeyForAgent,
                initialPrompt: initialPrompt,
                effectiveCodexHome: effectiveCodexHome,
                onStartPrompt: onStartPrompt ?? (_) {},
                onToggleExpanded: onToggleExpanded,
                onSubmitPrompt: onSubmitPrompt,
                onLoadMessages: onLoadMessages,
                onStopCodex: onStopCodex,
                onDeleteAgent: onDeleteAgent,
                onRenameAgent: onRenameAgent,
                hasUnreadResult: hasUnreadResult,
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
    required this.chatViewport,
    required this.agentListController,
    required this.composerKey,
    this.composerKeyForAgent,
    required this.headerKeyForAgent,
    required this.initialPrompt,
    this.effectiveCodexHome,
    required this.onStartPrompt,
    required this.onToggleExpanded,
    required this.onSubmitPrompt,
    this.onLoadMessages = _ignoreAgentSession,
    required this.onStopCodex,
    required this.onDeleteAgent,
    required this.onRenameAgent,
    this.hasUnreadResult = _neverUnreadAgentResult,
    required this.onFocusAgent,
    super.key,
  });

  final List<AgentSession> sessions;
  final String? expandedAgentLocalId;
  final String? focusedAgentLocalId;
  final ConversationViewportController chatViewport;
  final ScrollController agentListController;
  final GlobalKey<AgentComposerState> composerKey;
  final GlobalKey<AgentComposerState> Function(String agentId)?
  composerKeyForAgent;
  final Key Function(String agentId) headerKeyForAgent;
  final String initialPrompt;
  final String? effectiveCodexHome;
  final ValueChanged<String> onStartPrompt;
  final ValueChanged<AgentSession> onToggleExpanded;
  final void Function(AgentSession session, String prompt) onSubmitPrompt;
  final ValueChanged<AgentSession> onLoadMessages;
  final ValueChanged<AgentSession> onStopCodex;
  final ValueChanged<AgentSession> onDeleteAgent;
  final void Function(AgentSession session, String? title) onRenameAgent;
  final bool Function(AgentSession session) hasUnreadResult;
  final ValueChanged<AgentSession?> onFocusAgent;

  @override
  Widget build(BuildContext context) {
    GlobalKey<AgentComposerState> keyForAgent(AgentSession session) =>
        composerKeyForAgent?.call(session.localId) ?? composerKey;

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
                conversationId: 'new-agent',
                messages: const [],
                viewport: chatViewport,
                enlarged: false,
                composerKey: composerKey,
                initialPrompt: initialPrompt,
                hasSession: false,
                isWorking: false,
                autofocusAfterInitialPositioning: true,
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
              headerKey: headerKeyForAgent(focusedSession.localId),
              session: focusedSession,
              hasUnreadResult: hasUnreadResult(focusedSession),
              expanded: true,
              enlarged: true,
              chatViewport: chatViewport,
              composerKey: keyForAgent(focusedSession),
              initialPrompt: initialPrompt,
              effectiveCodexHome: effectiveCodexHome,
              onTap: () {},
              onEnlarge: () => onFocusAgent(null),
              onDelete: () => onDeleteAgent(focusedSession),
              onRename: (title) => onRenameAgent(focusedSession, title),
              onSubmitPrompt: (prompt) =>
                  onSubmitPrompt(focusedSession, prompt),
              onLoadMessages: () => onLoadMessages(focusedSession),
              onStopCodex: () => onStopCodex(focusedSession),
            ),
          ),
        ),
      );
    }
    final expandedIndex = expandedAgentLocalId == null
        ? -1
        : sessions.indexWhere(
            (session) => session.localId == expandedAgentLocalId,
          );
    if (expandedIndex >= 0) {
      final session = sessions[expandedIndex];
      return ExpandableAgentPanel(
        key: ValueKey(session.localId),
        headerKey: headerKeyForAgent(session.localId),
        session: session,
        hasUnreadResult: hasUnreadResult(session),
        expanded: true,
        enlarged: false,
        chatViewport: chatViewport,
        composerKey: keyForAgent(session),
        initialPrompt: initialPrompt,
        effectiveCodexHome: effectiveCodexHome,
        onTap: () => onToggleExpanded(session),
        onEnlarge: () => onFocusAgent(session),
        onDelete: () => onDeleteAgent(session),
        onRename: (title) => onRenameAgent(session, title),
        onSubmitPrompt: (prompt) => onSubmitPrompt(session, prompt),
        onLoadMessages: () => onLoadMessages(session),
        onStopCodex: () => onStopCodex(session),
      );
    }
    return LayoutBuilder(
      builder: (context, _) => Scrollbar(
        controller: agentListController,
        interactive: true,
        child: ListView.separated(
          key: const PageStorageKey<String>('agent-session-list'),
          controller: agentListController,
          physics: null,
          itemCount: sessions.length,
          separatorBuilder: (_, _) => const SizedBox(height: 10),
          itemBuilder: (context, index) {
            final session = sessions[index];
            return ExpandableAgentPanel(
              key: ValueKey(session.localId),
              headerKey: headerKeyForAgent(session.localId),
              session: session,
              hasUnreadResult: hasUnreadResult(session),
              expanded: false,
              enlarged: false,
              chatViewport: null,
              composerKey: null,
              initialPrompt: initialPrompt,
              effectiveCodexHome: effectiveCodexHome,
              onTap: () => onToggleExpanded(session),
              onEnlarge: () => onFocusAgent(session),
              onDelete: () => onDeleteAgent(session),
              onRename: (title) => onRenameAgent(session, title),
              onSubmitPrompt: (prompt) => onSubmitPrompt(session, prompt),
              onLoadMessages: () => onLoadMessages(session),
              onStopCodex: () => onStopCodex(session),
            );
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

bool _canResumeSession(AgentSession session, String? effectiveCodexHome) {
  final homeMismatch =
      session.hasCodexThread &&
      session.originCodexHome != null &&
      effectiveCodexHome != null &&
      session.originCodexHome != effectiveCodexHome;
  final noThread =
      session.cannotResumeWithoutThread ||
      session.resumeBlockReason == 'NoCodexThread';
  return !(noThread || homeMismatch);
}

String? _resumeBlockedMessage(
  AgentSession session,
  String? effectiveCodexHome,
) {
  final noThread =
      session.cannotResumeWithoutThread ||
      session.resumeBlockReason == 'NoCodexThread';
  if (noThread) {
    return 'This session cannot be resumed because Codex never created a thread. Start a new agent to continue.';
  }
  if (session.hasCodexThread &&
      session.originCodexHome != null &&
      effectiveCodexHome != null &&
      session.originCodexHome != effectiveCodexHome) {
    return 'This session used ${session.originCodexHome}. Switch the runtime to that CODEX_HOME to resume it, or start a new agent.';
  }
  return null;
}

class ExpandableAgentPanel extends StatelessWidget {
  const ExpandableAgentPanel({
    required this.headerKey,
    required this.session,
    this.hasUnreadResult = false,
    required this.expanded,
    required this.enlarged,
    required this.chatViewport,
    required this.composerKey,
    required this.initialPrompt,
    this.effectiveCodexHome,
    required this.onTap,
    required this.onEnlarge,
    required this.onDelete,
    required this.onRename,
    required this.onSubmitPrompt,
    this.onLoadMessages = _ignoreCallback,
    required this.onStopCodex,
    super.key,
  });

  final AgentSession session;
  final bool hasUnreadResult;
  final Key headerKey;
  final bool expanded;
  final bool enlarged;
  final ConversationViewportController? chatViewport;
  final GlobalKey<AgentComposerState>? composerKey;
  final String initialPrompt;
  final String? effectiveCodexHome;
  final VoidCallback onTap;
  final VoidCallback onEnlarge;
  final VoidCallback onDelete;
  final ValueChanged<String?> onRename;
  final ValueChanged<String> onSubmitPrompt;
  final VoidCallback onLoadMessages;
  final VoidCallback onStopCodex;

  @override
  Widget build(BuildContext context) {
    final resumeBlocked = !_canResumeSession(session, effectiveCodexHome);
    final stopping = session.status == AgentStatus.stopping;
    final resumeBlockedMessage = _resumeBlockedMessage(
      session,
      effectiveCodexHome,
    );

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
                  Row(
                    children: [
                      Flexible(
                        child: EditableAgentTitle(
                          title: session.displayName,
                          hasOverride:
                              session.userTitle?.trim().isNotEmpty ?? false,
                          onRename: onRename,
                        ),
                      ),
                      if (hasUnreadResult) ...[
                        const SizedBox(width: 8),
                        Tooltip(
                          message: 'New agent result',
                          child: Container(
                            key: ValueKey('agent-unread-${session.localId}'),
                            width: 8,
                            height: 8,
                            decoration: BoxDecoration(
                              color: context.ditch.accent,
                              shape: BoxShape.circle,
                            ),
                          ),
                        ),
                      ],
                    ],
                  ),
                  const SizedBox(height: 2),
                  AgentStatusChip(status: session.status),
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
              onPressed: session.canStop ? onStopCodex : null,
              icon: const Icon(Icons.stop_circle_outlined),
              label: const Text('Stop'),
            ),
            IconButton(
              onPressed: session.isActive ? null : onDelete,
              tooltip: 'Delete agent permanently',
              icon: const Icon(Icons.delete_outline),
            ),
          ],
        );
        Widget buildChatPanel() => AgentChatPanel(
          conversationId: session.localId,
          messages: session.messages,
          viewport: chatViewport!,
          enlarged: enlarged,
          composerKey: composerKey!,
          initialPrompt: session.hasCodexThread ? '' : initialPrompt,
          hasSession: session.hasCodexThread,
          isWorking: session.isWorking,
          canStop: session.canStop,
          enabled: !resumeBlocked && !stopping,
          disabledMessage: stopping
              ? 'Stopping Codex and waiting for the thread to be released…'
              : resumeBlockedMessage,
          onSubmitPrompt: onSubmitPrompt,
          messagesReady: session.messagesLoaded,
          messagesLoading: session.messagesLoading,
          hasOlderMessages: session.hasOlderMessages,
          historyError: session.historyError,
          onLoadOlder: onLoadMessages,
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
            child: DitchSurface(
              padding: const EdgeInsets.all(12),
              bordered: false,
              child: Column(
                children: [
                  InkWell(
                    key: headerKey,
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
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.only(top: 12),
                        child: buildChatPanel(),
                      ),
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
}

class AgentStatusChip extends StatelessWidget {
  const AgentStatusChip({required this.status, super.key});

  final AgentStatus status;

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final (label, color) = switch (status) {
      AgentStatus.starting || AgentStatus.working => (
        'Running',
        dark ? Colors.blue.shade300 : Colors.blue.shade700,
      ),
      AgentStatus.stopping => (
        'Stopping',
        dark ? Colors.orange.shade200 : Colors.orange.shade800,
      ),
      AgentStatus.completed => (
        'Completed',
        dark ? Colors.green.shade300 : Colors.green.shade700,
      ),
      AgentStatus.failed => (
        'Failed',
        dark ? Colors.red.shade300 : Colors.red.shade700,
      ),
      AgentStatus.stopped => (
        'Stopped',
        dark ? Colors.blueGrey.shade200 : Colors.blueGrey.shade700,
      ),
      AgentStatus.idle => (
        'Ready',
        dark ? Colors.grey.shade300 : Colors.grey.shade700,
      ),
    };
    return Align(
      alignment: Alignment.centerLeft,
      child: Container(
        key: ValueKey('agent-status-${status.name}'),
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        decoration: BoxDecoration(
          color: color.withValues(alpha: dark ? 0.20 : 0.12),
          borderRadius: BorderRadius.circular(6),
          border: Border.all(color: color.withValues(alpha: 0.28)),
        ),
        child: Text(
          label,
          style: Theme.of(context).textTheme.labelSmall?.copyWith(
            color: color,
            fontWeight: FontWeight.w700,
          ),
        ),
      ),
    );
  }
}

class AgentChatPanel extends StatelessWidget {
  const AgentChatPanel({
    required this.conversationId,
    required this.messages,
    required this.viewport,
    required this.enlarged,
    required this.composerKey,
    required this.initialPrompt,
    required this.hasSession,
    required this.isWorking,
    this.canStop = false,
    this.messagesReady = true,
    this.messagesLoading = false,
    this.hasOlderMessages = false,
    this.historyError,
    this.onLoadOlder = _ignoreCallback,
    this.enabled = true,
    this.disabledMessage,
    required this.onSubmitPrompt,
    required this.onStopCodex,
    this.onEnlarge,
    this.autofocusAfterInitialPositioning = false,
    super.key,
  });

  final String conversationId;
  final List<AgentChatMessage> messages;
  final ConversationViewportController viewport;
  final bool enlarged;
  final GlobalKey<AgentComposerState> composerKey;
  final String initialPrompt;
  final bool hasSession;
  final bool isWorking;
  final bool canStop;
  final bool messagesReady;
  final bool messagesLoading;
  final bool hasOlderMessages;
  final String? historyError;
  final VoidCallback onLoadOlder;
  final bool enabled;
  final String? disabledMessage;
  final ValueChanged<String> onSubmitPrompt;
  final VoidCallback onStopCodex;
  final VoidCallback? onEnlarge;
  final bool autofocusAfterInitialPositioning;

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
        canStop: canStop,
        enabled: enabled,
        onSubmit: onSubmitPrompt,
        onStop: onStopCodex,
        onEnlarge: onEnlarge,
        onEscape: enlarged ? onEnlarge : null,
      ),
    ];

    return ColoredBox(
      color: context.ditch.workspace,
      child: Column(
        children: [
          toolbar,
          const Divider(height: 1),
          Expanded(
            child: ConversationTranscript(
              key: ValueKey('conversation-$conversationId'),
              messages: messages,
              isWorking: isWorking,
              viewport: viewport,
              ready: messagesReady,
              loadingOlder: messagesLoading,
              hasOlderMessages: hasOlderMessages,
              historyError: historyError,
              onLoadOlder: onLoadOlder,
              onInitialPositioned: autofocusAfterInitialPositioning
                  ? () => composerKey.currentState?.focus()
                  : null,
            ),
          ),
          ...footer,
        ],
      ),
    );
  }
}

class ConversationTranscript extends StatefulWidget {
  const ConversationTranscript({
    required this.messages,
    required this.viewport,
    this.isWorking = false,
    this.ready = true,
    this.loadingOlder = false,
    this.hasOlderMessages = false,
    this.historyError,
    this.onLoadOlder = _ignoreCallback,
    this.onInitialPositioned,
    super.key,
  });

  final List<AgentChatMessage> messages;
  final ConversationViewportController viewport;
  final bool isWorking;
  final bool ready;
  final bool loadingOlder;
  final bool hasOlderMessages;
  final String? historyError;
  final VoidCallback onLoadOlder;
  final VoidCallback? onInitialPositioned;

  @override
  State<ConversationTranscript> createState() => _ConversationTranscriptState();
}

class _ConversationTranscriptState extends State<ConversationTranscript> {
  @override
  void initState() {
    super.initState();
    if (widget.ready) {
      widget.viewport.synchronizeItems(_itemIds);
      widget.viewport.attach(onInitialPositioned: widget.onInitialPositioned);
    } else {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) widget.onLoadOlder();
      });
    }
  }

  @override
  void didUpdateWidget(ConversationTranscript oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.viewport != widget.viewport) {
      if (widget.ready) {
        widget.viewport.attach(onInitialPositioned: widget.onInitialPositioned);
      }
    }
    if (widget.ready) {
      widget.viewport.synchronizeItems(_itemIds);
      if (!oldWidget.ready) {
        widget.viewport.attach(onInitialPositioned: widget.onInitialPositioned);
      }
    }
  }

  List<ConversationItem> get _items =>
      buildConversationItems(widget.messages, isWorking: widget.isWorking);

  List<String> get _itemIds =>
      _items.map((item) => item.identity).toList(growable: false);

  @override
  Widget build(BuildContext context) {
    final controller = widget.viewport.scrollController;
    if (!widget.ready) {
      return Center(
        child: widget.historyError == null
            ? const CircularProgressIndicator(
                key: Key('conversation-initial-loading'),
              )
            : _HistoryLoadFailure(
                message: widget.historyError!,
                onRetry: widget.onLoadOlder,
              ),
      );
    }
    final showHistoryControl =
        widget.hasOlderMessages ||
        widget.loadingOlder ||
        widget.historyError != null;
    final showEmptyState = widget.messages.isEmpty;
    final items = _items;
    return Stack(
      fit: StackFit.expand,
      children: [
        NotificationListener<ScrollNotification>(
          onNotification: (notification) {
            widget.viewport.handleScrollNotification(notification);
            if ((notification is ScrollUpdateNotification ||
                    notification is ScrollEndNotification) &&
                widget.viewport.isNearOldest &&
                widget.hasOlderMessages &&
                !widget.loadingOlder) {
              widget.onLoadOlder();
            }
            return false;
          },
          child: Scrollbar(
            controller: controller,
            interactive: true,
            child: ListView.separated(
              key: PageStorageKey<String>('transcript-${widget.key}'),
              controller: controller,
              reverse: true,
              padding: const EdgeInsets.fromLTRB(12, 12, 12, 20),
              itemCount: showEmptyState
                  ? 1
                  : items.length + (showHistoryControl ? 1 : 0),
              separatorBuilder: (_, _) => const SizedBox(height: 10),
              itemBuilder: (context, index) {
                if (showEmptyState) {
                  return const Center(child: Text('No messages yet.'));
                }
                if (index == items.length) {
                  return _HistoryLoadControl(
                    loading: widget.loadingOlder,
                    error: widget.historyError,
                    onRetry: widget.onLoadOlder,
                  );
                }
                final item = items[items.length - 1 - index];
                return KeyedSubtree(
                  key: ValueKey(item.identity),
                  child: item.isToolGroup
                      ? ToolActivityGroup(
                          messages: item.toolMessages,
                          active: item.isActiveToolGroup,
                        )
                      : AgentChatBubble(message: item.message!),
                );
              },
            ),
          ),
        ),
        ListenableBuilder(
          listenable: widget.viewport,
          builder: (context, _) {
            final count = widget.viewport.unseenCount;
            return AnimatedSwitcher(
              duration: const Duration(milliseconds: 140),
              child: count == 0
                  ? const SizedBox.shrink(
                      key: ValueKey('no-new-conversation-items'),
                    )
                  : Align(
                      key: const ValueKey('new-conversation-items'),
                      alignment: Alignment.bottomCenter,
                      child: Padding(
                        padding: const EdgeInsets.only(bottom: 16),
                        child: Semantics(
                          label:
                              '$count new ${count == 1 ? "message" : "messages"}',
                          button: true,
                          child: FilledButton.icon(
                            key: const Key('conversation-new-messages'),
                            onPressed: widget.viewport.showLatest,
                            icon: const Icon(Icons.arrow_downward, size: 16),
                            label: Text(
                              '$count new ${count == 1 ? "message" : "messages"}',
                            ),
                          ),
                        ),
                      ),
                    ),
            );
          },
        ),
      ],
    );
  }
}

class _HistoryLoadControl extends StatelessWidget {
  const _HistoryLoadControl({
    required this.loading,
    required this.error,
    required this.onRetry,
  });

  final bool loading;
  final String? error;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    if (error != null) {
      return _HistoryLoadFailure(message: error!, onRetry: onRetry);
    }
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 12),
      child: Center(
        child: loading
            ? const SizedBox.square(
                dimension: 20,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            : TextButton.icon(
                key: const Key('conversation-load-older'),
                onPressed: onRetry,
                icon: const Icon(Icons.history, size: 16),
                label: const Text('Load earlier messages'),
              ),
      ),
    );
  }
}

class _HistoryLoadFailure extends StatelessWidget {
  const _HistoryLoadFailure({required this.message, required this.onRetry});

  final String message;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(message, textAlign: TextAlign.center),
          const SizedBox(height: 8),
          OutlinedButton.icon(
            key: const Key('conversation-history-retry'),
            onPressed: onRetry,
            icon: const Icon(Icons.refresh, size: 16),
            label: const Text('Retry'),
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
    final tokens = context.ditch;
    final reduceMotion = MediaQuery.disableAnimationsOf(context);
    if (reduceMotion && _controller.isAnimating) {
      _controller.stop();
    } else if (!reduceMotion && widget.visible && !_controller.isAnimating) {
      _controller.repeat();
    }

    return AnimatedSwitcher(
      duration: reduceMotion
          ? const Duration(milliseconds: 1)
          : const Duration(milliseconds: 170),
      child: widget.visible
          ? Container(
              key: const ValueKey('thinking-status'),
              width: double.infinity,
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
              child: Align(
                alignment: Alignment.centerLeft,
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: tokens.waitingSoft,
                    borderRadius: BorderRadius.circular(
                      context.ditch.radiusCompact,
                    ),
                  ),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 12,
                      vertical: 7,
                    ),
                    child: AnimatedBuilder(
                      animation: _controller,
                      builder: (context, _) {
                        final dots = reduceMotion
                            ? 1
                            : 1 + (_controller.value * 3).floor();
                        return Text(
                          'Thinking${'.' * dots}',
                          style: Theme.of(context).textTheme.labelLarge
                              ?.copyWith(color: tokens.waiting),
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
    this.canStop = false,
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
  final bool canStop;
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
  static const _minimumEditorHeight = 48.0;
  static const _maximumEditorHeight = 160.0;
  late String _draftText;
  double _editorHeight = _minimumEditorHeight;
  bool _composerHasFocus = false;

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
    if (!widget.enabled || widget.isWorking || widget.canStop) {
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

  void _handleFocusChanged(bool focused) {
    if (_composerHasFocus == focused) return;
    setState(() => _composerHasFocus = focused);
  }

  void _handleContentHeightChanged(double height) {
    final nextHeight = height
        .clamp(_minimumEditorHeight, _maximumEditorHeight)
        .toDouble();
    if ((_editorHeight - nextHeight).abs() < 1) return;
    setState(() => _editorHeight = nextHeight);
  }

  Future<void> _selectApproval(AgentApprovalPreset value) async {
    if (value == AgentApprovalPreset.fullAccess) {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          icon: const Icon(Icons.warning_amber_rounded),
          title: const Text('Enable Full Access?'),
          content: const Text(
            'Codex will run without approval prompts or sandbox restrictions and can access the internet and files on this Mac.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('Enable Full Access'),
            ),
          ],
        ),
      );
      if (confirmed != true) return;
    }
    agentExecutionSettings.setApproval(value);
  }

  @override
  Widget build(BuildContext context) {
    final hasText = _draftText.trim().isNotEmpty;
    final actionLabel = widget.hasSession ? 'Send' : 'Start';
    final actionIcon = widget.hasSession ? Icons.send : Icons.play_arrow;
    final editable = widget.enabled;
    final canSubmit = widget.enabled && !widget.isWorking && !widget.canStop;

    return Padding(
      padding: const EdgeInsets.all(12),
      child: GestureDetector(
        key: const Key('composer-focus-surface'),
        behavior: HitTestBehavior.opaque,
        excludeFromSemantics: true,
        onTap: editable ? focus : null,
        child: Container(
          decoration: BoxDecoration(
            color: context.ditch.surfaceHover,
            borderRadius: BorderRadius.circular(context.ditch.radiusMedium),
            border: Border.all(
              color: _composerHasFocus
                  ? context.ditch.accent.withValues(alpha: 0.72)
                  : context.ditch.separator.withValues(alpha: 0.72),
            ),
          ),
          padding: const EdgeInsets.all(10),
          child: ListenableBuilder(
            listenable: agentExecutionSettings,
            builder: (context, _) => Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                LayoutBuilder(
                  builder: (context, constraints) => Row(
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: [
                      Expanded(
                        child: AnimatedContainer(
                          key: const Key('composer-editor-shell'),
                          duration: MediaQuery.disableAnimationsOf(context)
                              ? const Duration(milliseconds: 1)
                              : const Duration(milliseconds: 120),
                          curve: Curves.easeOut,
                          height: _editorHeight,
                          child: NativeComposerTextView(
                            key: _nativeComposerKey,
                            initialText: widget.initialText,
                            enabled: editable,
                            placeholder: widget.hasSession
                                ? 'Send a follow-up to Codex'
                                : 'Tell Codex what to do',
                            onChanged: _handleChanged,
                            onFocusChanged: _handleFocusChanged,
                            onContentHeightChanged: _handleContentHeightChanged,
                            onSubmitRequested: submit,
                            onEnlarge: widget.onEnlarge,
                            onEscape: widget.onEscape,
                          ),
                        ),
                      ),
                      const SizedBox(width: 6),
                      if (widget.canStop)
                        IconButton.filledTonal(
                          onPressed: widget.onStop,
                          tooltip: 'Stop Codex',
                          icon: const Icon(Icons.stop_circle_outlined),
                        )
                      else if (constraints.maxWidth < 190)
                        FilledButton.icon(
                          onPressed: hasText && canSubmit ? submit : null,
                          icon: Icon(actionIcon),
                          label: Text(actionLabel),
                          style: FilledButton.styleFrom(
                            padding: const EdgeInsets.symmetric(horizontal: 10),
                          ),
                        )
                      else
                        FilledButton.icon(
                          onPressed: hasText && canSubmit ? submit : null,
                          icon: Icon(actionIcon),
                          label: Text(actionLabel),
                        ),
                    ],
                  ),
                ),
                const SizedBox(height: 8),
                LayoutBuilder(
                  builder: (context, constraints) {
                    final controlWidth = constraints.maxWidth < 220
                        ? constraints.maxWidth
                        : 220.0;
                    return Wrap(
                      spacing: 16,
                      runSpacing: 8,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        SizedBox(
                          width: controlWidth,
                          child: DropdownButtonHideUnderline(
                            child: DropdownButton<AgentApprovalPreset>(
                              value: agentExecutionSettings.approval,
                              isDense: true,
                              isExpanded: true,
                              onChanged: (value) {
                                if (value != null) {
                                  unawaited(_selectApproval(value));
                                }
                              },
                              items: const [
                                DropdownMenuItem(
                                  value: AgentApprovalPreset.ask,
                                  enabled: false,
                                  child: Text('Ask for approval — unavailable'),
                                ),
                                DropdownMenuItem(
                                  value: AgentApprovalPreset.approveForMe,
                                  child: Text('Approve for me'),
                                ),
                                DropdownMenuItem(
                                  value: AgentApprovalPreset.fullAccess,
                                  child: Text('Full Access'),
                                ),
                              ],
                            ),
                          ),
                        ),
                        SizedBox(
                          width: controlWidth,
                          child: Row(
                            children: [
                              Expanded(
                                child: DropdownButtonHideUnderline(
                                  child: DropdownButton<String?>(
                                    value: agentExecutionSettings.model,
                                    hint: const Text('Default model'),
                                    isDense: true,
                                    isExpanded: true,
                                    onChanged: agentExecutionSettings.setModel,
                                    items: agentExecutionSettings.models
                                        .map(
                                          (model) => DropdownMenuItem<String?>(
                                            value: model.id,
                                            child: Text(model.displayName),
                                          ),
                                        )
                                        .toList(),
                                  ),
                                ),
                              ),
                              const SizedBox(width: 6),
                              ContextWindowIndicator(
                                model: agentExecutionSettings.selectedModel,
                              ),
                            ],
                          ),
                        ),
                        if (widget.isWorking || widget.canStop)
                          const Text('Applies next turn'),
                      ],
                    );
                  },
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class ContextWindowIndicator extends StatelessWidget {
  const ContextWindowIndicator({required this.model, super.key});

  final AgentModelOption? model;

  @override
  Widget build(BuildContext context) {
    final limit = model?.contextWindowTokens;
    final label = limit == null
        ? 'Context size unavailable'
        : '${_formatTokens(limit)} context window; live usage is unavailable';
    return Tooltip(
      message: label,
      child: Semantics(
        label: label,
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: context.ditch.surface,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 4),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  Icons.data_usage_outlined,
                  size: 14,
                  color: limit == null ? Theme.of(context).disabledColor : null,
                ),
                if (limit != null) ...[
                  const SizedBox(width: 3),
                  Text(
                    _formatTokens(limit),
                    style: Theme.of(context).textTheme.labelSmall,
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }

  static String _formatTokens(int tokens) =>
      tokens >= 1000 ? '${(tokens / 1000).round()}k' : '$tokens';
}

class NativeComposerTextView extends StatefulWidget {
  const NativeComposerTextView({
    required this.initialText,
    required this.enabled,
    required this.placeholder,
    required this.onChanged,
    required this.onFocusChanged,
    required this.onContentHeightChanged,
    required this.onSubmitRequested,
    this.onEnlarge,
    this.onEscape,
    super.key,
  });

  final String initialText;
  final bool enabled;
  final String placeholder;
  final ValueChanged<String> onChanged;
  final ValueChanged<bool> onFocusChanged;
  final ValueChanged<double> onContentHeightChanged;
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
  int? _lastAppearanceHash;
  late final TextEditingController _fallbackController;
  late final FocusNode _fallbackFocusNode;

  @override
  void initState() {
    super.initState();
    _fallbackController = TextEditingController(text: widget.initialText);
    _fallbackFocusNode = FocusNode();
    _fallbackFocusNode.addListener(_handleFallbackFocusChanged);
  }

  @override
  void didUpdateWidget(NativeComposerTextView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.initialText != widget.initialText &&
        _fallbackController.text.trim().isEmpty) {
      _fallbackController.text = widget.initialText;
      final channel = _channel;
      if (channel != null &&
          !kIsWeb &&
          defaultTargetPlatform == TargetPlatform.macOS) {
        unawaited(channel.invokeMethod<void>('setText', widget.initialText));
      }
    }
    _syncNativeEnabled();
  }

  @override
  void dispose() {
    _fallbackFocusNode.removeListener(_handleFallbackFocusChanged);
    _channel?.setMethodCallHandler(null);
    _channel = null;
    _fallbackController.dispose();
    _fallbackFocusNode.dispose();
    super.dispose();
  }

  void _handleFallbackFocusChanged() {
    widget.onFocusChanged(_fallbackFocusNode.hasFocus);
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
      final focused = await channel.invokeMethod<bool>('focus') ?? false;
      if (!focused && mounted && _focusRequested) {
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (mounted && _focusRequested && identical(_channel, channel)) {
            unawaited(channel.invokeMethod<bool>('focus'));
          }
        });
      }
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

  Map<String, Object> _appearance(BuildContext context) {
    final tokens = context.ditch;
    return {
      'textColor': tokens.ink.toARGB32(),
      'placeholderColor': tokens.muted.toARGB32(),
      'caretColor': tokens.accent.toARGB32(),
      'fontName': 'Avenir Next',
    };
  }

  Future<void> _syncNativeAppearance(BuildContext context) async {
    final channel = _channel;
    if (channel == null || !mounted) return;
    final appearance = _appearance(context);
    final hash = Object.hashAll(appearance.values);
    if (_lastAppearanceHash == hash) return;
    _lastAppearanceHash = hash;
    await channel.invokeMethod<void>('setAppearance', appearance);
  }

  @override
  Widget build(BuildContext context) {
    if (_channel != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) unawaited(_syncNativeAppearance(context));
      });
    }
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
        'text': _fallbackController.text,
        'enabled': widget.enabled,
        'placeholder': widget.placeholder,
        'fontSize': 14.0,
        'escapeEnabled': widget.onEscape != null,
        ..._appearance(context),
      },
      creationParamsCodec: const StandardMessageCodec(),
      onPlatformViewCreated: (id) {
        final channel = MethodChannel('the_ditch/composer_text_view/$id');
        _channel = channel;
        _lastSentEnabled = widget.enabled;
        unawaited(_syncNativeAppearance(context));
        channel.setMethodCallHandler((call) async {
          if (call.method == 'textChanged') {
            final text = call.arguments as String? ?? '';
            if (_fallbackController.text != text) {
              _fallbackController.value = TextEditingValue(
                text: text,
                selection: TextSelection.collapsed(offset: text.length),
              );
            }
            widget.onChanged(text);
          } else if (call.method == 'focusChanged') {
            final focused = call.arguments as bool? ?? false;
            _focusRequested = focused;
            widget.onFocusChanged(focused);
          } else if (call.method == 'contentHeightChanged') {
            final height = (call.arguments as num?)?.toDouble();
            if (height != null) widget.onContentHeightChanged(height);
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

class ToolActivityGroup extends StatefulWidget {
  const ToolActivityGroup({
    required this.messages,
    required this.active,
    super.key,
  });

  final List<AgentChatMessage> messages;
  final bool active;

  @override
  State<ToolActivityGroup> createState() => _ToolActivityGroupState();
}

class _ToolActivityGroupState extends State<ToolActivityGroup> {
  late bool _expanded;

  @override
  void initState() {
    super.initState();
    _expanded = widget.active;
  }

  @override
  void didUpdateWidget(ToolActivityGroup oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.active && !widget.active) {
      _expanded = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final colors = Theme.of(context).colorScheme;
    final foreground = colors.onTertiaryContainer;
    final count = widget.messages.length;
    final summary =
        'Tool activity · $count ${count == 1 ? "action" : "actions"}';
    return LayoutBuilder(
      builder: (context, constraints) {
        final maxWidth = constraints.maxWidth < 520
            ? constraints.maxWidth * 0.9
            : (constraints.maxWidth * 0.72).clamp(360.0, 760.0);
        return Align(
          alignment: Alignment.centerLeft,
          child: ConstrainedBox(
            constraints: BoxConstraints(maxWidth: maxWidth),
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: colors.tertiaryContainer,
                borderRadius: BorderRadius.circular(context.ditch.radiusMedium),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Semantics(
                    button: true,
                    expanded: _expanded,
                    label: summary,
                    child: InkWell(
                      key: const Key('tool-activity-toggle'),
                      borderRadius: BorderRadius.circular(
                        context.ditch.radiusMedium,
                      ),
                      onTap: () => setState(() => _expanded = !_expanded),
                      child: Padding(
                        padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
                        child: Row(
                          children: [
                            Icon(Icons.terminal, size: 16, color: foreground),
                            const SizedBox(width: 7),
                            Expanded(
                              child: Text(
                                summary,
                                style: Theme.of(context).textTheme.labelMedium
                                    ?.copyWith(
                                      color: foreground,
                                      fontWeight: FontWeight.w700,
                                    ),
                              ),
                            ),
                            if (widget.active)
                              Padding(
                                padding: const EdgeInsets.only(right: 6),
                                child: Text(
                                  'Running',
                                  style: Theme.of(context).textTheme.labelSmall
                                      ?.copyWith(color: foreground),
                                ),
                              ),
                            Icon(
                              _expanded
                                  ? Icons.keyboard_arrow_up
                                  : Icons.keyboard_arrow_down,
                              color: foreground,
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                  if (_expanded) ...[
                    Divider(
                      height: 1,
                      color: foreground.withValues(alpha: .18),
                    ),
                    Padding(
                      padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          for (var index = 0; index < count; index++) ...[
                            Row(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Expanded(
                                  child: SelectionArea(
                                    child: Text(
                                      widget.messages[index].text,
                                      style: Theme.of(context)
                                          .textTheme
                                          .bodySmall
                                          ?.copyWith(
                                            color: foreground,
                                            height: 1.4,
                                            fontFamily: 'SF Mono',
                                            fontSize: 12.5,
                                          ),
                                    ),
                                  ),
                                ),
                                IconButton(
                                  visualDensity: VisualDensity.compact,
                                  tooltip: 'Copy tool action',
                                  icon: const Icon(
                                    Icons.copy_outlined,
                                    size: 15,
                                  ),
                                  onPressed: () => Clipboard.setData(
                                    ClipboardData(
                                      text: widget.messages[index].text,
                                    ),
                                  ),
                                ),
                              ],
                            ),
                            if (index < count - 1)
                              Divider(
                                height: 16,
                                color: foreground.withValues(alpha: .14),
                              ),
                          ],
                        ],
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ),
        );
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
        'Ditch',
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

class ProjectToolsPanel extends StatefulWidget {
  const ProjectToolsPanel({
    required this.width,
    required this.projectId,
    required this.terminal,
    required this.files,
    required this.showFiles,
    required this.onEnsureTerminal,
    required this.onEnsureFiles,
    required this.presentation,
    required this.presentedTool,
    required this.dockedTerminalExpanded,
    required this.dockedFilesExpanded,
    required this.onToggleDocked,
    required this.onToggleFiles,
    required this.onPresentationChanged,
    required this.onToolPresentationChanged,
    required this.onToggleDirectory,
    required this.onOpenFile,
    required this.onRevealFile,
    required this.onBackToFiles,
    required this.onSaveFile,
    required this.onReloadFile,
    required this.onOverwriteFile,
    required this.onRefreshFiles,
    super.key,
  });

  final double width;
  final String? projectId;
  final ProjectTerminalSession? terminal;
  final ProjectFilesState? files;
  final bool showFiles;
  final Future<void> Function() onEnsureTerminal;
  final Future<void> Function() onEnsureFiles;
  final TerminalPresentation presentation;
  final WorkspaceToolKind presentedTool;
  final bool dockedTerminalExpanded;
  final bool dockedFilesExpanded;
  final VoidCallback onToggleDocked;
  final VoidCallback onToggleFiles;
  final ValueChanged<TerminalPresentation> onPresentationChanged;
  final void Function(WorkspaceToolKind, TerminalPresentation)
  onToolPresentationChanged;
  final ValueChanged<ProjectFileEntry> onToggleDirectory;
  final ValueChanged<ProjectFileEntry> onOpenFile;
  final ValueChanged<ProjectFileEntry> onRevealFile;
  final VoidCallback? onBackToFiles;
  final VoidCallback? onSaveFile;
  final VoidCallback? onReloadFile;
  final VoidCallback? onOverwriteFile;
  final VoidCallback onRefreshFiles;

  @override
  State<ProjectToolsPanel> createState() => _ProjectToolsPanelState();
}

class _ProjectToolsPanelState extends State<ProjectToolsPanel> {
  @override
  void initState() {
    super.initState();
    if (widget.terminal == null) {
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => widget.onEnsureTerminal(),
      );
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => widget.onEnsureFiles());
  }

  @override
  void didUpdateWidget(ProjectToolsPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.terminal == null &&
        oldWidget.terminal?.projectId != widget.terminal?.projectId) {
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => widget.onEnsureTerminal(),
      );
    }
    if (oldWidget.projectId != widget.projectId) {
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => widget.onEnsureFiles(),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final terminal = widget.terminal;
    if (widget.presentation == TerminalPresentation.vertical) {
      return SizedBox(
        width: widget.width,
        child:
            widget.presentedTool == WorkspaceToolKind.editor &&
                widget.files?.document != null
            ? ProjectFileEditorSurface(
                files: widget.files!,
                presentation: widget.presentation,
                onBack: widget.onBackToFiles!,
                onSave: widget.onSaveFile!,
                onReload: widget.onReloadFile!,
                onOverwrite: widget.onOverwriteFile!,
                onPresentationChanged: (value) => widget
                    .onToolPresentationChanged(WorkspaceToolKind.editor, value),
              )
            : ProjectTerminalSurface(
                terminal: terminal,
                presentation: widget.presentation,
                onTitleTap: () =>
                    widget.onPresentationChanged(TerminalPresentation.docked),
                onPresentationChanged: widget.onPresentationChanged,
              ),
      );
    }
    return SizedBox(
      width: widget.width,
      child: Column(
        children: [
          ProjectTerminalHeader(
            terminalAvailable: terminal != null,
            expanded: widget.dockedTerminalExpanded,
            presentation: widget.presentation,
            onTitleTap: widget.onToggleDocked,
            onPresentationChanged: widget.onPresentationChanged,
          ),
          if (widget.dockedTerminalExpanded)
            Expanded(child: ProjectTerminalBody(terminal: terminal)),
          if (widget.showFiles) ...[
            ProjectFilesHeader(
              expanded: widget.dockedFilesExpanded,
              hasDocument: widget.files?.document != null,
              onTitleTap: widget.onToggleFiles,
              onRefresh: widget.onRefreshFiles,
            ),
            if (widget.dockedFilesExpanded)
              Expanded(
                child: ProjectFilesBody(
                  files: widget.files,
                  presentation: widget.presentation,
                  onToggleDirectory: widget.onToggleDirectory,
                  onOpenFile: widget.onOpenFile,
                  onRevealFile: widget.onRevealFile,
                  onBack: widget.onBackToFiles,
                  onSave: widget.onSaveFile,
                  onReload: widget.onReloadFile,
                  onOverwrite: widget.onOverwriteFile,
                  onPresentationChanged: (value) =>
                      widget.onToolPresentationChanged(
                        WorkspaceToolKind.editor,
                        value,
                      ),
                ),
              ),
          ],
          if (!widget.showFiles)
            const Expanded(child: SizedBox())
          else if (!widget.dockedTerminalExpanded &&
              !widget.dockedFilesExpanded)
            const Spacer(),
        ],
      ),
    );
  }
}

class ProjectFilesHeader extends StatelessWidget {
  const ProjectFilesHeader({
    required this.expanded,
    required this.hasDocument,
    required this.onTitleTap,
    required this.onRefresh,
    super.key,
  });

  final bool expanded;
  final bool hasDocument;
  final VoidCallback onTitleTap;
  final VoidCallback onRefresh;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: context.ditch.inspector,
      child: SizedBox(
        height: 44,
        child: Row(
          children: [
            Expanded(
              child: InkWell(
                onTap: onTitleTap,
                child: Padding(
                  padding: const EdgeInsets.only(left: 14),
                  child: Row(
                    children: [
                      Icon(
                        expanded ? Icons.expand_more : Icons.chevron_right,
                        size: 18,
                      ),
                      const SizedBox(width: 6),
                      const Icon(Icons.folder_outlined, size: 17),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          hasDocument ? 'Editor' : 'File Explorer',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
            IconButton(
              key: const Key('files-refresh'),
              tooltip: 'Refresh files',
              onPressed: onRefresh,
              icon: const Icon(Icons.refresh, size: 18),
            ),
            const SizedBox(width: 4),
          ],
        ),
      ),
    );
  }
}

class ProjectFilesBody extends StatelessWidget {
  const ProjectFilesBody({
    required this.files,
    required this.presentation,
    required this.onToggleDirectory,
    required this.onOpenFile,
    required this.onRevealFile,
    required this.onBack,
    required this.onSave,
    required this.onReload,
    required this.onOverwrite,
    required this.onPresentationChanged,
    super.key,
  });

  final ProjectFilesState? files;
  final TerminalPresentation presentation;
  final ValueChanged<ProjectFileEntry> onToggleDirectory;
  final ValueChanged<ProjectFileEntry> onOpenFile;
  final ValueChanged<ProjectFileEntry> onRevealFile;
  final VoidCallback? onBack;
  final VoidCallback? onSave;
  final VoidCallback? onReload;
  final VoidCallback? onOverwrite;
  final ValueChanged<TerminalPresentation> onPresentationChanged;

  List<(ProjectFileEntry, int)> _visibleEntries(ProjectFilesState state) {
    final visible = <(ProjectFileEntry, int)>[];
    void append(String directory, int depth) {
      for (final entry in state.directories[directory] ?? const []) {
        visible.add((entry, depth));
        if (entry.isDirectory &&
            state.expandedDirectories.contains(entry.relativePath)) {
          append(entry.relativePath, depth + 1);
        }
      }
    }

    append('', 0);
    return visible;
  }

  @override
  Widget build(BuildContext context) {
    final state = files;
    if (state == null) return const Center(child: Text('Select a project.'));
    final document = state.document;
    if (document != null) {
      return ProjectFileEditorBody(
        document: document,
        presentation: presentation,
        onBack: onBack!,
        onSave: onSave!,
        onReload: onReload!,
        onOverwrite: onOverwrite!,
        onPresentationChanged: onPresentationChanged,
      );
    }
    if (!state.directories.containsKey('') &&
        state.loadingDirectories.contains('')) {
      return const Center(child: CircularProgressIndicator());
    }
    final entries = _visibleEntries(state);
    return Column(
      children: [
        if (state.error != null)
          Padding(
            padding: const EdgeInsets.all(10),
            child: Text(
              state.error!,
              style: TextStyle(color: context.ditch.error),
            ),
          ),
        Expanded(
          child: entries.isEmpty
              ? const Center(child: Text('No files to show.'))
              : ListView.builder(
                  key: const Key('project-file-tree'),
                  primary: false,
                  itemCount: entries.length,
                  itemBuilder: (context, index) {
                    final (entry, depth) = entries[index];
                    final expanded = state.expandedDirectories.contains(
                      entry.relativePath,
                    );
                    final loading = state.loadingDirectories.contains(
                      entry.relativePath,
                    );
                    return ListTile(
                      dense: true,
                      contentPadding: EdgeInsets.only(
                        left: 10 + depth * 16,
                        right: 8,
                      ),
                      leading: loading
                          ? const SizedBox.square(
                              dimension: 16,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : Icon(
                              entry.isDirectory
                                  ? expanded
                                        ? Icons.folder_open_outlined
                                        : Icons.folder_outlined
                                  : entry.isFile
                                  ? Icons.description_outlined
                                  : Icons.link,
                              size: 17,
                            ),
                      title: Text(
                        entry.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(fontSize: 13),
                      ),
                      trailing: IconButton(
                        tooltip: 'Reveal in Finder',
                        visualDensity: VisualDensity.compact,
                        onPressed: () => onRevealFile(entry),
                        icon: const Icon(Icons.open_in_new, size: 15),
                      ),
                      onTap: entry.isDirectory
                          ? () => onToggleDirectory(entry)
                          : entry.isFile
                          ? () => onOpenFile(entry)
                          : null,
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class ProjectFileEditorSurface extends StatelessWidget {
  const ProjectFileEditorSurface({
    required this.files,
    required this.presentation,
    required this.onBack,
    required this.onSave,
    required this.onReload,
    required this.onOverwrite,
    required this.onPresentationChanged,
    this.onClose,
    super.key,
  });

  final ProjectFilesState files;
  final TerminalPresentation presentation;
  final VoidCallback onBack;
  final VoidCallback onSave;
  final VoidCallback onReload;
  final VoidCallback onOverwrite;
  final ValueChanged<TerminalPresentation> onPresentationChanged;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final document = files.document!;
    return ColoredBox(
      color: context.ditch.workspace,
      child: ProjectFileEditorBody(
        document: document,
        presentation: presentation,
        onBack: onBack,
        onSave: onSave,
        onReload: onReload,
        onOverwrite: onOverwrite,
        onPresentationChanged: onPresentationChanged,
        onClose: onClose,
      ),
    );
  }
}

class ProjectFileEditorBody extends StatelessWidget {
  const ProjectFileEditorBody({
    required this.document,
    required this.presentation,
    required this.onBack,
    required this.onSave,
    required this.onReload,
    required this.onOverwrite,
    required this.onPresentationChanged,
    this.onClose,
    super.key,
  });

  final ProjectEditorDocument document;
  final TerminalPresentation presentation;
  final VoidCallback onBack;
  final VoidCallback onSave;
  final VoidCallback onReload;
  final VoidCallback onOverwrite;
  final ValueChanged<TerminalPresentation> onPresentationChanged;
  final VoidCallback? onClose;

  void _toggle(TerminalPresentation target) {
    onPresentationChanged(
      presentation == target ? TerminalPresentation.docked : target,
    );
  }

  @override
  Widget build(BuildContext context) {
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyS, meta: true): onSave,
      },
      child: Column(
        children: [
          Material(
            color: context.ditch.inspector,
            child: SizedBox(
              height: 44,
              child: Row(
                children: [
                  IconButton(
                    key: const Key('editor-back'),
                    tooltip: 'Back to file tree',
                    onPressed: onBack,
                    icon: const Icon(Icons.arrow_back, size: 18),
                  ),
                  Expanded(
                    child: Text(
                      '${document.dirty ? '● ' : ''}${document.relativePath}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: 13),
                    ),
                  ),
                  IconButton(
                    key: const Key('editor-save'),
                    tooltip: 'Save file (⌘S)',
                    onPressed: document.dirty && !document.saving
                        ? onSave
                        : null,
                    icon: document.saving
                        ? const SizedBox.square(
                            dimension: 16,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.save_outlined, size: 18),
                  ),
                  IconButton(
                    key: const Key('editor-expand-horizontal'),
                    tooltip: 'Expand editor horizontally',
                    onPressed: () => _toggle(TerminalPresentation.horizontal),
                    icon: const Icon(Icons.swap_horiz, size: 18),
                  ),
                  IconButton(
                    key: const Key('editor-expand-vertical'),
                    tooltip: 'Expand editor vertically',
                    onPressed: () => _toggle(TerminalPresentation.vertical),
                    icon: const Icon(Icons.swap_vert, size: 18),
                  ),
                  IconButton(
                    key: const Key('editor-maximize'),
                    tooltip: 'Maximize editor',
                    onPressed: () => _toggle(TerminalPresentation.maximized),
                    icon: const Icon(Icons.fullscreen, size: 19),
                  ),
                  if (onClose != null)
                    IconButton(
                      key: const Key('editor-maximize-close'),
                      tooltip: 'Restore editor',
                      onPressed: onClose,
                      icon: const Icon(Icons.close, size: 18),
                    ),
                ],
              ),
            ),
          ),
          if (document.error != null)
            Material(
              color: context.ditch.error.withValues(alpha: .10),
              child: Padding(
                padding: const EdgeInsets.symmetric(
                  horizontal: 12,
                  vertical: 8,
                ),
                child: Row(
                  children: [
                    Expanded(child: Text(document.error!)),
                    TextButton(
                      onPressed: onReload,
                      child: const Text('Reload'),
                    ),
                    if (document.conflict)
                      TextButton(
                        onPressed: onOverwrite,
                        child: const Text('Overwrite'),
                      ),
                  ],
                ),
              ),
            ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.all(10),
              child: TextField(
                key: const Key('project-file-editor'),
                controller: document.controller,
                expands: true,
                maxLines: null,
                minLines: null,
                keyboardType: TextInputType.multiline,
                textAlignVertical: TextAlignVertical.top,
                style: const TextStyle(
                  fontFamily: 'SF Mono',
                  fontSize: 13,
                  height: 1.45,
                ),
                decoration: const InputDecoration(
                  border: OutlineInputBorder(),
                  contentPadding: EdgeInsets.all(12),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class ProjectTerminalSurface extends StatelessWidget {
  const ProjectTerminalSurface({
    required this.terminal,
    required this.presentation,
    required this.onTitleTap,
    required this.onPresentationChanged,
    this.onClose,
    super.key,
  });

  final ProjectTerminalSession? terminal;
  final TerminalPresentation presentation;
  final VoidCallback onTitleTap;
  final ValueChanged<TerminalPresentation> onPresentationChanged;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final terminalTheme = ditchTerminalTheme(context);
    return ColoredBox(
      color: terminalTheme.background,
      child: Column(
        children: [
          ProjectTerminalHeader(
            terminalAvailable: terminal != null,
            expanded: true,
            presentation: presentation,
            onTitleTap: onTitleTap,
            onPresentationChanged: onPresentationChanged,
            onClose: onClose,
          ),
          Expanded(
            child: ProjectTerminalBody(
              terminal: terminal,
              onEscape: presentation == TerminalPresentation.maximized
                  ? onClose
                  : null,
            ),
          ),
        ],
      ),
    );
  }
}

class ProjectTerminalBody extends StatelessWidget {
  const ProjectTerminalBody({required this.terminal, this.onEscape, super.key});

  final ProjectTerminalSession? terminal;
  final VoidCallback? onEscape;

  @override
  Widget build(BuildContext context) {
    return terminal == null
        ? const Center(child: Text('Starting project shell…'))
        : TerminalView(
            terminal!.terminal,
            autofocus: false,
            theme: ditchTerminalTheme(context),
            padding: const EdgeInsets.all(8),
            onKeyEvent: onEscape == null
                ? null
                : (_, event) {
                    if (event is KeyDownEvent &&
                        event.logicalKey == LogicalKeyboardKey.escape) {
                      onEscape!();
                      return KeyEventResult.handled;
                    }
                    return KeyEventResult.ignored;
                  },
          );
  }
}

TerminalTheme ditchTerminalTheme(BuildContext context) {
  final tokens = context.ditch;
  final dark = Theme.of(context).brightness == Brightness.dark;
  return TerminalTheme(
    cursor: tokens.accent,
    selection: tokens.accent.withValues(alpha: 0.30),
    foreground: dark ? const Color(0xffeef0f4) : const Color(0xff272d39),
    background: dark ? const Color(0xff090c12) : const Color(0xfff7f6f2),
    black: dark ? const Color(0xff111620) : const Color(0xff272d39),
    red: tokens.error,
    green: tokens.success,
    yellow: tokens.waiting,
    blue: tokens.running,
    magenta: dark ? const Color(0xffd7a7f2) : const Color(0xff7f3ca3),
    cyan: dark ? const Color(0xff7fd8df) : const Color(0xff167078),
    white: dark ? const Color(0xffeef0f4) : const Color(0xfff7f6f2),
    brightBlack: tokens.muted,
    brightRed: dark ? const Color(0xffffaaa0) : const Color(0xffd55245),
    brightGreen: dark ? const Color(0xff91e2b9) : const Color(0xff249361),
    brightYellow: dark ? const Color(0xffffca79) : const Color(0xffba6e08),
    brightBlue: dark ? const Color(0xffc7d5ff) : const Color(0xff4469bb),
    brightMagenta: dark ? const Color(0xffe6bef7) : const Color(0xff9852b8),
    brightCyan: dark ? const Color(0xffa0eaf0) : const Color(0xff238891),
    brightWhite: tokens.ink,
    searchHitBackground: tokens.accentSoft,
    searchHitBackgroundCurrent: tokens.accent,
    searchHitForeground: const Color(0xff17120d),
  );
}

class ProjectTerminalHeader extends StatelessWidget {
  const ProjectTerminalHeader({
    required this.terminalAvailable,
    required this.expanded,
    required this.presentation,
    required this.onTitleTap,
    required this.onPresentationChanged,
    this.onClose,
    super.key,
  });

  final bool terminalAvailable;
  final bool expanded;
  final TerminalPresentation presentation;
  final VoidCallback onTitleTap;
  final ValueChanged<TerminalPresentation> onPresentationChanged;
  final VoidCallback? onClose;

  void _toggle(TerminalPresentation target) {
    onPresentationChanged(
      presentation == target ? TerminalPresentation.docked : target,
    );
  }

  @override
  Widget build(BuildContext context) {
    return Material(
      color: context.ditch.inspector,
      child: SizedBox(
        height: 44,
        child: Row(
          children: [
            if (onClose != null)
              IconButton(
                key: const Key('terminal-maximize-close'),
                tooltip: 'Restore terminal',
                onPressed: onClose,
                icon: const Icon(Icons.close, size: 18),
              ),
            Expanded(
              child: InkWell(
                onTap: onTitleTap,
                child: LayoutBuilder(
                  builder: (context, constraints) => Padding(
                    padding: EdgeInsets.only(left: onClose == null ? 14 : 4),
                    child: Row(
                      children: [
                        Icon(
                          expanded ? Icons.expand_more : Icons.chevron_right,
                          size: 18,
                        ),
                        const SizedBox(width: 6),
                        const Icon(Icons.terminal, size: 17),
                        const SizedBox(width: 8),
                        const Expanded(
                          child: Text(
                            'Terminal',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                        if (!terminalAvailable &&
                            constraints.maxWidth >= 96) ...[
                          const SizedBox(width: 6),
                          const Icon(Icons.more_horiz, size: 18),
                        ],
                      ],
                    ),
                  ),
                ),
              ),
            ),
            IconButton(
              key: const Key('terminal-expand-horizontal'),
              tooltip: 'Expand terminal horizontally',
              onPressed: () => _toggle(TerminalPresentation.horizontal),
              icon: Icon(
                Icons.swap_horiz,
                size: 18,
                color: presentation == TerminalPresentation.horizontal
                    ? context.ditch.accent
                    : null,
              ),
            ),
            IconButton(
              key: const Key('terminal-expand-vertical'),
              tooltip: 'Expand terminal vertically',
              onPressed: () => _toggle(TerminalPresentation.vertical),
              icon: Icon(
                Icons.swap_vert,
                size: 18,
                color: presentation == TerminalPresentation.vertical
                    ? context.ditch.accent
                    : null,
              ),
            ),
            IconButton(
              key: const Key('terminal-maximize'),
              tooltip: 'Maximize terminal',
              onPressed: () => _toggle(TerminalPresentation.maximized),
              icon: Icon(
                Icons.fullscreen,
                size: 19,
                color: presentation == TerminalPresentation.maximized
                    ? context.ditch.accent
                    : null,
              ),
            ),
            const SizedBox(width: 4),
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
                'Ditch will create and verify:\n.ditch/agents  •  .ditch/hooks  •  .ditch/mcp',
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
