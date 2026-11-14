part of 'main.dart';

/// Shared by the composer and new-agent dialog. Catalogs remain owned by settings.
class AgentModelPicker extends StatefulWidget {
  const AgentModelPicker({
    required this.settings,
    this.onAdd,
    this.onSelect,
    this.onRetryOpenAI,
    this.onRetryOss,
    super.key,
  });

  final AgentExecutionSettings settings;
  final VoidCallback? onAdd;
  final Future<void> Function(AgentModelOption?)? onSelect;
  final VoidCallback? onRetryOpenAI;
  final VoidCallback? onRetryOss;

  @override
  State<AgentModelPicker> createState() => _AgentModelPickerState();
}

class _AgentModelPickerState extends State<AgentModelPicker> {
  final _anchor = GlobalKey();
  final _focus = FocusNode(debugLabel: 'Model picker trigger');
  bool _open = false;

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  Future<void> _show() async {
    if (_open) return;
    final box = _anchor.currentContext!.findRenderObject()! as RenderBox;
    final anchor = box.localToGlobal(Offset.zero) & box.size;
    final settings = widget.settings;
    setState(() => _open = true);
    final choice = await showDialog<_ModelPickerChoice>(
      context: context,
      barrierColor: Colors.transparent,
      useSafeArea: false,
      builder: (context) => LayoutBuilder(
        builder: (context, constraints) {
          final padding = MediaQuery.paddingOf(context);
          final bottomInset = MediaQuery.viewInsetsOf(context).bottom;
          final top = padding.top + 12;
          final bottom =
              constraints.maxHeight - padding.bottom - bottomInset - 12;
          final maxHeight = (bottom - top).clamp(0.0, 440.0);
          final above = (anchor.top - top - 8).clamp(0.0, maxHeight);
          final below = (bottom - anchor.bottom - 8).clamp(0.0, maxHeight);
          final useAbove = above >= 440 || above >= below;
          final available = useAbove ? above : below;
          final fallback = available < 260;
          final height = fallback ? maxHeight : available;
          final width = (constraints.maxWidth - padding.horizontal - 24).clamp(
            0.0,
            380.0,
          );
          final left = anchor.left.clamp(
            padding.left + 12,
            constraints.maxWidth - padding.right - width - 12,
          );
          return Stack(
            children: [
              Positioned(
                left: left,
                top: fallback
                    ? top
                    : (useAbove ? anchor.top - height - 8 : anchor.bottom + 8)
                          .clamp(top, bottom - height),
                width: width,
                height: height,
                child: Align(
                  alignment: useAbove && !fallback
                      ? Alignment.bottomCenter
                      : Alignment.topCenter,
                  child: _AgentModelMenu(
                    settings: settings,
                    canAdd: widget.onAdd != null && !settings.providerBound,
                    onRetryOpenAI: widget.onRetryOpenAI,
                    onRetryOss: widget.onRetryOss,
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
    if (!mounted) return;
    setState(() => _open = false);
    _focus.requestFocus();
    // A project/scope switch while the menu was open must not edit its replacement.
    if (!identical(widget.settings, settings) || choice == null) return;
    if (choice.add) {
      widget.onAdd?.call();
    } else {
      if (widget.onSelect != null) {
        await widget.onSelect!(choice.option);
      } else {
        settings.selectOption(choice.option);
      }
    }
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.settings,
    builder: (context, _) {
      final settings = widget.settings;
      final model = settings.selectedModel;
      final label = model?.displayName ?? settings.model ?? 'Default model';
      final server = settings.inferenceEndpointId == null
          ? null
          : model?.endpointName ?? 'Saved server';
      return Tooltip(
        message: [label, ?server].join(' · '),
        child: Semantics(
          expanded: _open,
          child: TextButton(
            key: _anchor,
            focusNode: _focus,
            onPressed: _show,
            style: TextButton.styleFrom(
              foregroundColor: context.ditch.ink,
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
              textStyle: Theme.of(context).textTheme.bodyMedium,
            ),
            child: Row(
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(label, maxLines: 1, overflow: TextOverflow.ellipsis),
                      if (server != null)
                        Text(
                          server,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: Theme.of(context).textTheme.bodySmall,
                        ),
                    ],
                  ),
                ),
                const SizedBox(width: 6),
                Icon(_open ? Icons.expand_less : Icons.expand_more, size: 18),
              ],
            ),
          ),
        ),
      );
    },
  );
}

class _ModelPickerChoice {
  const _ModelPickerChoice(this.option) : add = false;
  const _ModelPickerChoice.add() : option = null, add = true;
  final AgentModelOption? option;
  final bool add;
}

class _AgentModelMenu extends StatefulWidget {
  const _AgentModelMenu({
    required this.settings,
    required this.canAdd,
    this.onRetryOpenAI,
    this.onRetryOss,
  });
  final AgentExecutionSettings settings;
  final bool canAdd;
  final VoidCallback? onRetryOpenAI;
  final VoidCallback? onRetryOss;

  @override
  State<_AgentModelMenu> createState() => _AgentModelMenuState();
}

class _AgentModelMenuState extends State<_AgentModelMenu> {
  final _search = TextEditingController();
  final _searchFocus = FocusNode();
  final _scroll = ScrollController();
  final _rowKeys = <String, GlobalKey>{};
  late bool _openAI = widget.settings.inferenceEndpointId == null;
  late bool _oss = !_openAI;
  String? _active;
  static const _defaultKey = 'default-model';

  @override
  void dispose() {
    _search.dispose();
    _searchFocus.dispose();
    _scroll.dispose();
    super.dispose();
  }

  String _key(AgentModelOption? model) => model?.selectionKey ?? _defaultKey;
  bool _matches(AgentModelOption? model) {
    final query = _search.text.toLowerCase().trim().replaceAll(
      RegExp(r'\s+'),
      ' ',
    );
    if (model == null) return 'default model openai'.contains(query);
    final server = model.endpointName ?? '';
    return [
      '${model.id} $server',
      '${model.displayName} $server',
      model.endpointId == null ? 'openai' : 'oss',
    ].any((text) => text.toLowerCase().contains(query));
  }

  void _revealActive() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final row = _rowKeys[_active]?.currentContext;
      if (row != null) Scrollable.ensureVisible(row, alignment: .5);
    });
  }

  Widget _row(AgentModelOption? model, {bool missing = false}) {
    final key = _key(model);
    final selected = widget.settings.selectionKey == model?.selectionKey;
    final error = model?.availabilityError;
    return Padding(
      key: _rowKeys.putIfAbsent(key, GlobalKey.new),
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      child: Semantics(
        selected: selected,
        child: TextButton(
          key: ValueKey('model-option:$key'),
          onPressed: missing
              ? null
              : () => Navigator.pop(context, _ModelPickerChoice(model)),
          style: TextButton.styleFrom(
            alignment: Alignment.centerLeft,
            foregroundColor: context.ditch.ink,
            disabledForegroundColor: context.ditch.muted,
            backgroundColor: selected ? context.ditch.accentSoft : null,
            side: _active == key
                ? BorderSide(color: context.ditch.accent)
                : BorderSide.none,
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(7),
            ),
            textStyle: Theme.of(context).textTheme.bodyMedium,
          ),
          child: Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(model?.displayName ?? 'Default model'),
                    if (model?.endpointId != null)
                      Text(
                        model?.endpointName ?? 'Saved server',
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                    if (missing || error != null)
                      Text(
                        missing
                            ? 'Saved model is not in the current catalog'
                            : error!,
                        style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color: context.ditch.error,
                        ),
                      ),
                  ],
                ),
              ),
              if (selected) ...[
                const SizedBox(width: 8),
                Icon(Icons.check, size: 18, color: context.ditch.accent),
              ],
            ],
          ),
        ),
      ),
    );
  }

  Widget _group({
    required bool oss,
    required List<AgentModelOption?> options,
    required bool expanded,
    required bool loading,
    String? error,
    VoidCallback? retry,
    required String? missingKey,
  }) {
    final title = oss ? 'OSS models' : 'OpenAI models';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Semantics(
          expanded: expanded,
          child: TextButton(
            onPressed: _search.text.trim().isNotEmpty
                ? null
                : () => setState(() {
                    if (oss) {
                      _oss = !_oss;
                    } else {
                      _openAI = !_openAI;
                    }
                    _active = null;
                  }),
            style: TextButton.styleFrom(
              foregroundColor: context.ditch.muted,
              disabledForegroundColor: context.ditch.muted,
              textStyle: Theme.of(
                context,
              ).textTheme.bodySmall?.copyWith(fontWeight: FontWeight.w500),
              padding: const EdgeInsets.symmetric(horizontal: 12),
            ),
            child: Row(
              children: [
                Icon(
                  expanded ? Icons.expand_more : Icons.chevron_right,
                  size: 16,
                ),
                const SizedBox(width: 6),
                Text(title),
                const Spacer(),
                Text('${options.length}'),
              ],
            ),
          ),
        ),
        if (loading)
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: 14),
            child: LinearProgressIndicator(minHeight: 2),
          ),
        if (error != null)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 4),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    error,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                ),
                if (retry != null)
                  TextButton(
                    onPressed: loading ? null : retry,
                    child: const Text('Retry'),
                  ),
              ],
            ),
          ),
        if (expanded) ...[
          for (final model in options)
            _row(model, missing: _key(model) == missingKey),
          if (options.isEmpty && !loading && error == null)
            Padding(
              padding: const EdgeInsets.all(14),
              child: Text(
                _search.text.trim().isNotEmpty
                    ? 'No matching models'
                    : oss
                    ? 'No OSS models added yet'
                    : 'No OpenAI models available',
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ),
        ],
      ],
    );
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.settings,
    builder: (context, _) {
      final settings = widget.settings;
      final searching = _search.text.trim().isNotEmpty;
      final showOpenAI =
          !settings.providerBound || settings.boundEndpointId == null;
      final showOss =
          !settings.providerBound || settings.boundEndpointId != null;
      final models = settings.allModels.where(settings.canSelect).toList();
      String? missingKey;
      if (settings.model != null && settings.selectedModel == null) {
        final saved = AgentModelOption(
          id: settings.model!,
          displayName: settings.model!,
          endpointId: settings.inferenceEndpointId,
          endpointName: settings.inferenceEndpointId == null
              ? null
              : 'Saved server',
        );
        models.insert(0, saved);
        missingKey = saved.selectionKey;
      }
      final openAI = <AgentModelOption?>[
        if (_matches(null)) null,
        ...models.where((m) => m.endpointId == null && _matches(m)),
      ];
      final oss = models
          .where((m) => m.endpointId != null && _matches(m))
          .toList();
      final visible = <AgentModelOption?>[
        if (showOpenAI && (_openAI || searching)) ...openAI,
        if (showOss && (_oss || searching)) ...oss,
      ].where((m) => _key(m) != missingKey).toList();
      return Focus(
        onKeyEvent: (_, event) {
          if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
            return KeyEventResult.ignored;
          }
          if (event.logicalKey == LogicalKeyboardKey.escape) {
            Navigator.pop(context);
            return KeyEventResult.handled;
          }
          if (!_searchFocus.hasFocus || visible.isEmpty) {
            return KeyEventResult.ignored;
          }
          final down = event.logicalKey == LogicalKeyboardKey.arrowDown;
          final up = event.logicalKey == LogicalKeyboardKey.arrowUp;
          if (down || up) {
            final index = visible.indexWhere((m) => _key(m) == _active);
            final next = index < 0
                ? (down ? 0 : visible.length - 1)
                : (index + (down ? 1 : -1)) % visible.length;
            setState(() => _active = _key(visible[next]));
            _revealActive();
            return KeyEventResult.handled;
          }
          if (event.logicalKey == LogicalKeyboardKey.enter) {
            final index = visible.indexWhere((m) => _key(m) == _active);
            if (index >= 0 || (searching && visible.length == 1)) {
              Navigator.pop(
                context,
                _ModelPickerChoice(visible[index < 0 ? 0 : index]),
              );
              return KeyEventResult.handled;
            }
          }
          return KeyEventResult.ignored;
        },
        child: Material(
          key: const ValueKey('model-picker-popover'),
          color: context.ditch.surface,
          elevation: 12,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(12),
            side: BorderSide(color: context.ditch.line),
          ),
          clipBehavior: Clip.antiAlias,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.all(12),
                child: TextField(
                  key: const ValueKey('model-picker-search'),
                  controller: _search,
                  focusNode: _searchFocus,
                  autofocus: true,
                  style: Theme.of(context).textTheme.bodyMedium,
                  decoration: InputDecoration(
                    hintText: 'Search models or servers…',
                    prefixIcon: const Icon(Icons.search, size: 18),
                    suffixIcon: searching
                        ? IconButton(
                            tooltip: 'Clear search',
                            icon: const Icon(Icons.close, size: 16),
                            onPressed: () => setState(() {
                              _search.clear();
                              _active = null;
                            }),
                          )
                        : null,
                  ),
                  onChanged: (_) => setState(() {
                    _active = null;
                    if (_scroll.hasClients) _scroll.jumpTo(0);
                  }),
                ),
              ),
              const Divider(height: 1),
              Flexible(
                child: Scrollbar(
                  controller: _scroll,
                  child: SingleChildScrollView(
                    controller: _scroll,
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        if (showOpenAI)
                          _group(
                            oss: false,
                            options: openAI,
                            expanded: _openAI || searching,
                            loading: settings.loadingModels,
                            error: settings.modelError,
                            retry: widget.onRetryOpenAI,
                            missingKey: missingKey,
                          ),
                        if (showOpenAI && showOss)
                          const Divider(height: 1, indent: 12, endIndent: 12),
                        if (showOss)
                          _group(
                            oss: true,
                            options: oss,
                            expanded: _oss || searching,
                            loading: settings.loadingOssModels,
                            error: settings.ossError,
                            retry: widget.onRetryOss,
                            missingKey: missingKey,
                          ),
                      ],
                    ),
                  ),
                ),
              ),
              if (settings.providerBound || widget.canAdd) ...[
                const Divider(height: 1),
                if (settings.providerBound)
                  Padding(
                    padding: const EdgeInsets.all(12),
                    child: Text(
                      'Start a new agent to change inference server.',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                  ),
                if (widget.canAdd)
                  Padding(
                    padding: const EdgeInsets.all(6),
                    child: TextButton.icon(
                      style: TextButton.styleFrom(
                        alignment: Alignment.centerLeft,
                        textStyle: Theme.of(context).textTheme.bodyMedium,
                      ),
                      onPressed: () => Navigator.pop(
                        context,
                        const _ModelPickerChoice.add(),
                      ),
                      icon: const Icon(Icons.add, size: 18),
                      label: const Text('Add OSS model…'),
                    ),
                  ),
              ],
            ],
          ),
        ),
      );
    },
  );
}
