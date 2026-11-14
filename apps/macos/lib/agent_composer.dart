part of 'main.dart';

class AgentComposer extends StatefulWidget {
  const AgentComposer({
    required this.initialText,
    this.draft,
    this.agentId,
    this.isRemote = false,
    required this.hasSession,
    required this.isWorking,
    this.canStop = false,
    this.enabled = true,
    this.expanded = false,
    required this.onSubmit,
    required this.onStop,
    this.onEnlarge,
    this.onEscape,
    this.executionSettings,
    this.runtimeClient,
    this.projectId,
    this.loadModels,
    super.key,
  });

  final String initialText;
  final AgentPromptDraft? draft;
  final String? agentId;
  final bool isRemote;
  final AgentExecutionSettings? executionSettings;
  final DitchRuntimeClient? runtimeClient;
  final String? projectId;
  final Future<List<AgentModelOption>> Function()? loadModels;
  final bool hasSession;
  final bool isWorking;
  final bool canStop;
  final bool enabled;
  final bool expanded;

  /// True only after runtime acceptance; false retains the editable draft.
  final Future<bool> Function(String) onSubmit;
  final VoidCallback onStop;
  final VoidCallback? onEnlarge;
  final VoidCallback? onEscape;

  @override
  State<AgentComposer> createState() => AgentComposerState();
}

class AgentComposerState extends State<AgentComposer> {
  var _nativeComposerKey = GlobalKey<NativeComposerTextViewState>();
  double get _minimumEditorHeight => widget.expanded ? 160.0 : 72.0;
  double get _maximumEditorHeight => widget.expanded ? 320.0 : 200.0;
  late String _draftText;
  final _localAttachments = <PromptAttachment>[];
  List<PromptAttachment> get _attachments =>
      widget.draft?.attachments ?? _localAttachments;
  String? _attachmentError;
  int _generation = 0;
  int _attachmentRevision = 0;
  int _textRevision = 0;
  int? _submittingGeneration;
  Future<void> _dropQueue = Future.value();
  double _editorHeight = 72.0;
  bool _composerHasFocus = false;
  AgentSettingsScope? _scope;
  AgentExecutionSettings get _settings =>
      widget.executionSettings ?? _scope?.settings ?? agentExecutionSettings;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _scope = AgentSettingsScope.of(context);
  }

  @override
  void initState() {
    super.initState();
    _editorHeight = _minimumEditorHeight;
    _draftText = widget.draft?.text ?? widget.initialText;
    widget.draft?.addListener(_draftSubmissionChanged);
  }

  void _draftSubmissionChanged() {
    if (!mounted) return;
    final draft = widget.draft!;
    final restoreFocus = _composerHasFocus && draft.text != _draftText;
    setState(() {
      final text = draft.text;
      if (text != _draftText) {
        _draftText = text;
        _nativeComposerKey = GlobalKey<NativeComposerTextViewState>();
        _editorHeight = _minimumEditorHeight;
      }
    });
    if (restoreFocus) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && identical(draft, widget.draft)) focus();
      });
    }
  }

  @override
  void dispose() {
    widget.draft?.removeListener(_draftSubmissionChanged);
    super.dispose();
  }

  @override
  void didUpdateWidget(AgentComposer oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.expanded != widget.expanded) {
      _editorHeight = _minimumEditorHeight;
    }
    if (!identical(oldWidget.draft, widget.draft)) {
      oldWidget.draft?.removeListener(_draftSubmissionChanged);
      widget.draft?.addListener(_draftSubmissionChanged);
      _generation++;
      _dropQueue = Future.value();
      _attachmentRevision++;
      _localAttachments.clear();
      _attachmentError = null;
      _draftText = widget.draft?.text ?? widget.initialText;
      _nativeComposerKey = GlobalKey<NativeComposerTextViewState>();
      _editorHeight = _minimumEditorHeight;
    } else if (widget.draft == null &&
        oldWidget.initialText != widget.initialText &&
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

  Future<void> chooseFiles() async {
    final generation = _generation;
    try {
      final paths = await const MethodChannel(
        'the_ditch/project_picker',
      ).invokeListMethod<String>('chooseFiles');
      if (mounted && generation == _generation && paths != null) {
        await _includeFiles(paths);
      }
    } on Object catch (error) {
      if (mounted && generation == _generation) {
        setState(() => _attachmentError = 'Could not choose files: $error');
      }
    }
  }

  Future<void> _includeFiles(List<String> paths) {
    final generation = _generation;
    // Serial validation preserves the order of consecutive multi-file drops.
    _dropQueue = _dropQueue.then((_) async {
      if (!mounted || generation != _generation || !widget.enabled) return;
      if (widget.isRemote) {
        setState(
          () => _attachmentError =
              'Local file inclusion is unavailable for SSH projects. Use a path on the remote machine.',
        );
        return;
      }
      final valid = <PromptAttachment>[];
      final invalid = <String>[];
      for (final path in paths) {
        final attachment = PromptAttachment(path);
        if (path.startsWith('/') && await attachment.isReadable) {
          valid.add(attachment);
        } else {
          invalid.add(attachment.filename);
        }
      }
      if (!mounted ||
          generation != _generation ||
          !widget.enabled ||
          widget.isRemote) {
        return;
      }
      setState(() {
        final existing = _attachments.map((file) => file.path).toSet();
        for (final file in valid) {
          if (existing.add(file.path)) _attachments.add(file);
        }
        _attachmentRevision++;
        widget.draft?.attachmentsChanged();
        _attachmentError = invalid.isEmpty
            ? null
            : 'Cannot include ${invalid.join(', ')}. Choose readable regular files.';
      });
    });
    return _dropQueue;
  }

  Future<void> submit() async {
    if (!widget.enabled ||
        widget.isWorking ||
        widget.canStop ||
        _submittingGeneration == _generation ||
        widget.draft?.submitting == true) {
      return;
    }
    final generation = _generation;
    setState(() => _submittingGeneration = generation);
    final editor = _nativeComposerKey.currentState;
    final draft = widget.draft;
    draft?.beginSubmission();
    var accepted = false;
    var submittedRevision = draft?.revision ?? 0;
    var submittedAttachments = <PromptAttachment>[];
    bool current() =>
        mounted &&
        generation == _generation &&
        identical(editor, _nativeComposerKey.currentState) &&
        identical(draft, widget.draft) &&
        widget.enabled &&
        !widget.isWorking &&
        !widget.canStop;
    try {
      await _dropQueue;
      if (!current()) return;
      final revision = _attachmentRevision;
      final nativeText = await editor?.currentText() ?? _draftText;
      if (!current()) return;
      final attachments = List<PromptAttachment>.of(_attachments);
      if (attachments.isNotEmpty && widget.isRemote) {
        setState(
          () => _attachmentError =
              'Local file inclusion is unavailable for SSH projects. Remove these files to send.',
        );
        return;
      }
      for (final attachment in attachments) {
        if (!await attachment.isReadable) {
          if (current()) {
            setState(
              () => _attachmentError =
                  'Cannot read ${attachment.filename}. Restore the file or remove it to send.',
            );
          }
          return;
        }
      }
      if (!current() || revision != _attachmentRevision) return;
      // Do not clear edits made while file availability was being checked.
      if ((await editor?.currentText() ?? _draftText) != nativeText ||
          !current() ||
          revision != _attachmentRevision) {
        return;
      }
      final prompt = serializeAttachedPrompt(nativeText, attachments);
      if (prompt.isEmpty) return;
      _handleChanged(nativeText);
      final textRevision = _textRevision;
      submittedRevision = draft?.revision ?? 0;
      submittedAttachments = attachments;
      accepted = await widget.onSubmit(prompt);
      bool sameComposer() =>
          mounted &&
          generation == _generation &&
          identical(draft, widget.draft) &&
          identical(editor, _nativeComposerKey.currentState);
      if (accepted && sameComposer()) {
        final beforeRead = _textRevision;
        final latestText = await editor?.currentText() ?? _draftText;
        if (sameComposer() && beforeRead == _textRevision) {
          // Native text can be ahead of its textChanged notification.
          _handleChanged(latestText);
          if (draft == null &&
              textRevision == _textRevision &&
              revision == _attachmentRevision &&
              latestText == nativeText) {
            _handleChanged('');
            setState(() {
              _attachments.clear();
              _attachmentRevision++;
              _attachmentError = null;
            });
            await editor?.clearText();
          }
        }
      }
    } on Object catch (error) {
      if (mounted && generation == _generation) {
        if (accepted) draft?.revision++;
        setState(
          () => _attachmentError = accepted
              ? 'Prompt accepted, but the draft could not be cleared: $error'
              : 'Prompt was not accepted: $error',
        );
      }
    } finally {
      draft?.finishSubmission(
        accepted: accepted,
        submittedRevision: submittedRevision,
        submittedAttachments: submittedAttachments,
      );
      if (_submittingGeneration == generation) {
        if (mounted) {
          setState(() => _submittingGeneration = null);
        } else {
          _submittingGeneration = null;
        }
      }
    }
  }

  void _handleChanged(String text) {
    if (_draftText != text) {
      _textRevision++;
      setState(() => _draftText = text);
    }
    // Update this editor before broadcasting, so it does not remount itself.
    widget.draft?.text = text;
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

  @override
  Widget build(BuildContext context) {
    final hasText = _draftText.trim().isNotEmpty || _attachments.isNotEmpty;
    final actionLabel = widget.hasSession ? 'Send' : 'Start';
    final editable = widget.enabled;
    final canSubmit =
        widget.enabled &&
        !widget.isWorking &&
        !widget.canStop &&
        _submittingGeneration != _generation &&
        widget.draft?.submitting != true;

    return NativeFileDropRegion(
      key: ValueKey(_generation),
      enabled: widget.enabled && (ModalRoute.of(context)?.isCurrent ?? true),
      remote: widget.isRemote,
      onDrop: _includeFiles,
      child: Padding(
        padding: const EdgeInsets.all(8),
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
              listenable: Listenable.merge([_settings, widget.draft]),
              builder: (context, _) => Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (_attachments.isNotEmpty) ...[
                    PromptAttachments(
                      attachments: _attachments,
                      onRemove: widget.enabled
                          ? (file) => setState(() {
                              _attachments.remove(file);
                              _attachmentRevision++;
                              widget.draft?.attachmentsChanged();
                              _attachmentError = null;
                            })
                          : null,
                    ),
                    const SizedBox(height: 8),
                  ],
                  if (_attachmentError != null)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 8),
                      child: Semantics(
                        liveRegion: true,
                        child: Text(
                          _attachmentError!,
                          style: TextStyle(
                            color: Theme.of(context).colorScheme.error,
                          ),
                        ),
                      ),
                    ),
                  AnimatedContainer(
                    key: const Key('composer-editor-shell'),
                    duration: MediaQuery.disableAnimationsOf(context)
                        ? Duration.zero
                        : const Duration(milliseconds: 120),
                    curve: Curves.easeOut,
                    height: _editorHeight,
                    child: NativeComposerTextView(
                      key: _nativeComposerKey,
                      initialText: _draftText,
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
                  const SizedBox(height: 6),
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.end,
                    children: [
                      IconButton(
                        key: const Key('composer-attach'),
                        style: IconButton.styleFrom(
                          minimumSize: const Size(30, 30),
                          maximumSize: const Size(30, 30),
                          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                        ),
                        tooltip: widget.isRemote
                            ? 'Local attachments unavailable for SSH projects'
                            : 'Attach files',
                        onPressed: editable && !widget.isRemote
                            ? chooseFiles
                            : null,
                        icon: const Icon(Icons.attach_file, size: 16),
                        constraints: const BoxConstraints.tightFor(
                          width: 30,
                          height: 30,
                        ),
                        padding: EdgeInsets.zero,
                        visualDensity: VisualDensity.compact,
                      ),
                      const SizedBox(width: 4),
                      Expanded(
                        child: AgentExecutionControls(
                          runtimeClient:
                              widget.runtimeClient ?? _scope?.runtimeClient,
                          projectId: widget.projectId ?? _scope?.projectId,
                          settings: _settings,
                          agentId: widget.agentId,
                          isRemote: widget.isRemote,
                          loadModels: widget.loadModels ?? _scope?.loadModels,
                          appliesNextTurn:
                              widget.isWorking ||
                              widget.canStop ||
                              widget.draft?.submitting == true,
                          visualizer: widget.draft?.visualizer ?? false,
                          onVisualizerChanged: widget.draft == null
                              ? null
                              : (value) {
                                  widget.draft!.visualizer = value;
                                },
                        ),
                      ),
                      const SizedBox(width: 6),
                      IconButton.filled(
                        key: const Key('composer-submit'),
                        style: IconButton.styleFrom(
                          minimumSize: const Size(32, 32),
                          maximumSize: const Size(32, 32),
                          tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                        ),
                        tooltip: widget.canStop ? 'Stop Codex' : actionLabel,
                        onPressed: widget.canStop
                            ? widget.onStop
                            : hasText && canSubmit
                            ? submit
                            : null,
                        constraints: const BoxConstraints.tightFor(
                          width: 32,
                          height: 32,
                        ),
                        padding: EdgeInsets.zero,
                        visualDensity: VisualDensity.compact,
                        icon: Icon(
                          widget.canStop ? Icons.stop : Icons.arrow_upward,
                          size: 18,
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class AgentExecutionControls extends StatefulWidget {
  const AgentExecutionControls({
    required this.settings,
    this.loadModels,
    this.runtimeClient,
    this.projectId,
    this.agentId,
    this.isRemote = false,
    this.appliesNextTurn = false,
    this.visualizer = false,
    this.onVisualizerChanged,
    super.key,
  });

  final AgentExecutionSettings settings;
  final Future<List<AgentModelOption>> Function()? loadModels;
  final DitchRuntimeClient? runtimeClient;
  final String? projectId;
  final String? agentId;
  final bool isRemote;
  final bool appliesNextTurn;
  final bool visualizer;
  final ValueChanged<bool>? onVisualizerChanged;

  @override
  State<AgentExecutionControls> createState() => _AgentExecutionControlsState();
}

class _AgentExecutionControlsState extends State<AgentExecutionControls> {
  AgentExecutionSettings? _loadedSettings;
  bool _changing = false;

  Future<bool> _revokeDesktop() async {
    if (widget.agentId == null ||
        widget.runtimeClient == null ||
        widget.settings.inferenceEndpointId == null ||
        widget.isRemote) {
      return true;
    }
    try {
      final status = await widget.runtimeClient!.runtimeStatus();
      if (!status.capabilities.contains('tool_integrations_v1')) return true;
      await widget.runtimeClient!.request({
        'ToolIntegration': {
          'Revoke': {'agent_id': widget.agentId},
        },
      });
      return true;
    } catch (error) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Could not finish stopping computer use: $error'),
          ),
        );
      }
      return false;
    }
  }

  Future<void> _selectModel(AgentModelOption? option) async {
    if (_changing || option?.selectionKey == widget.settings.selectionKey) {
      return;
    }
    setState(() => _changing = true);
    try {
      if (await _revokeDesktop() && mounted) {
        widget.settings.selectOption(option);
      }
    } finally {
      if (mounted) setState(() => _changing = false);
    }
  }

  @override
  void initState() {
    super.initState();
    _loadModels();
  }

  @override
  void didUpdateWidget(AgentExecutionControls oldWidget) {
    super.didUpdateWidget(oldWidget);
    _loadModels();
  }

  void _loadModels() {
    if (widget.loadModels == null &&
        (widget.runtimeClient == null || widget.projectId == null)) {
      _loadedSettings = null;
      return;
    }
    if (identical(_loadedSettings, widget.settings)) return;
    final settings = _loadedSettings = widget.settings;
    final load = widget.loadModels;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && load != null) unawaited(settings.loadModels(load));
      if (mounted && widget.runtimeClient != null && widget.projectId != null) {
        unawaited(
          settings.loadOssModels(widget.runtimeClient!, widget.projectId!),
        );
      }
    });
  }

  Future<void> _selectApproval(AgentApprovalPreset value) async {
    if (_changing || value == widget.settings.approval) return;
    setState(() => _changing = true);
    try {
      if (value == AgentApprovalPreset.fullAccess) {
        final confirmed = await showDialog<bool>(
          context: context,
          builder: (context) => AlertDialog(
            icon: const Icon(Icons.warning_amber_rounded),
            title: const Text('Enable Full Access?'),
            content: Text(
              'Codex will run without approval prompts or sandbox restrictions and can access the internet and files on the computer running this project.'
              '${widget.settings.inferenceEndpointId != null && !widget.isRemote ? ' Computer use can also observe and control applications on this Mac when macOS permissions are granted to Ditch. Observations are sent to your selected model.' : ''}',
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
      if (!mounted) return;
      if (value != AgentApprovalPreset.fullAccess && !await _revokeDesktop()) {
        return;
      }
      if (mounted) widget.settings.setApproval(value);
    } finally {
      if (mounted) setState(() => _changing = false);
    }
  }

  Future<void> _addOssModel() async {
    if (widget.runtimeClient == null ||
        widget.projectId == null ||
        widget.settings.providerBound) {
      return;
    }
    final option = await showDialog<AgentModelOption>(
      context: context,
      builder: (_) => AddOssModelDialog(
        client: widget.runtimeClient!,
        projectId: widget.projectId!,
      ),
    );
    if (!mounted || option == null) return;
    await widget.settings.loadOssModels(
      widget.runtimeClient!,
      widget.projectId!,
    );
    if (!mounted) return;
    await _selectModel(option);
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.settings,
    builder: (context, _) {
      return LayoutBuilder(
        builder: (context, constraints) {
          final compact = constraints.maxWidth < 480;
          return Wrap(
            spacing: 6,
            runSpacing: 4,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              SizedBox(
                width: compact ? 132 : 152,
                child: AgentModelPicker(
                  settings: widget.settings,
                  onSelect: _selectModel,
                  onAdd:
                      widget.runtimeClient != null && widget.projectId != null
                      ? () => unawaited(_addOssModel())
                      : null,
                  onRetryOpenAI: widget.loadModels == null
                      ? null
                      : () => unawaited(
                          widget.settings.loadModels(widget.loadModels!),
                        ),
                  onRetryOss:
                      widget.runtimeClient == null || widget.projectId == null
                      ? null
                      : () => unawaited(
                          widget.settings.loadOssModels(
                            widget.runtimeClient!,
                            widget.projectId!,
                          ),
                        ),
                ),
              ),
              SizedBox(
                width: compact ? 104 : 132,
                child: DropdownButtonHideUnderline(
                  child: DropdownButton<AgentApprovalPreset>(
                    value: widget.settings.approval,
                    isDense: true,
                    style: Theme.of(context).textTheme.labelSmall?.copyWith(
                      color: context.ditch.ink,
                      fontSize: 12,
                      fontWeight: FontWeight.w400,
                    ),
                    iconSize: 14,
                    isExpanded: true,
                    onChanged: _changing
                        ? null
                        : (value) {
                            if (value != null) {
                              unawaited(_selectApproval(value));
                            }
                          },
                    selectedItemBuilder: compact
                        ? (context) => const [
                            Tooltip(
                              message: 'Ask for approval',
                              child: Text('Ask'),
                            ),
                            Tooltip(
                              message: 'Approve for me',
                              child: Text('Auto'),
                            ),
                            Tooltip(
                              message: 'Full Access',
                              child: Text('Full access'),
                            ),
                          ]
                        : null,
                    items: const [
                      DropdownMenuItem(
                        value: AgentApprovalPreset.ask,
                        child: Text('Ask for approval'),
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
              if (widget.settings.inferenceEndpointId != null &&
                  widget.runtimeClient != null)
                DesktopIntegrationStatus(
                  compact: compact,
                  client: widget.runtimeClient!,
                  agentId: widget.agentId,
                  fullAccess:
                      widget.settings.approval ==
                      AgentApprovalPreset.fullAccess,
                  isRemote: widget.isRemote,
                  toolsDeclared: widget.settings.selectedModel?.toolsDeclared,
                  visualizer: widget.visualizer,
                  onVisualizerChanged: widget.onVisualizerChanged,
                  appliesNextTurn: widget.appliesNextTurn,
                ),
              if (_changing)
                const Tooltip(
                  message: 'Stopping computer use…',
                  child: Icon(Icons.hourglass_top, size: 14),
                )
              else if (widget.appliesNextTurn)
                const Tooltip(
                  message: 'Changes apply next turn',
                  child: Icon(Icons.schedule, size: 14),
                ),
            ],
          );
        },
      );
    },
  );
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
          maxLines: 8,
          style: Theme.of(context).textTheme.bodyMedium?.copyWith(
            fontSize: 14,
            fontWeight: FontWeight.w400,
          ),
          onChanged: widget.onChanged,
          decoration: InputDecoration(
            hintText: widget.placeholder,
            border: InputBorder.none,
            enabledBorder: InputBorder.none,
            focusedBorder: InputBorder.none,
            disabledBorder: InputBorder.none,
            filled: false,
            contentPadding: EdgeInsets.zero,
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
