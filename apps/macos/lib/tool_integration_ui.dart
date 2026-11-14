part of 'main.dart';

class DesktopLicensesDialog extends StatefulWidget {
  const DesktopLicensesDialog({super.key});
  @override
  State<DesktopLicensesDialog> createState() => _DesktopLicensesDialogState();
}

class _DesktopLicensesDialogState extends State<DesktopLicensesDialog> {
  late final Future<String> _text = File(
    '${File(Platform.resolvedExecutable).parent.parent.path}/Resources/DitchComputerUseLicenses.txt',
  ).readAsString();

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Computer Use Licenses'),
    content: SizedBox(
      width: 640,
      height: 440,
      child: FutureBuilder<String>(
        future: _text,
        builder: (context, snapshot) => snapshot.hasError
            ? const Text(
                'Notices are available in the built Ditch app bundle. Source builds also include the original notices in community/vendor/peekaboo and the resolved dependencies.',
              )
            : snapshot.hasData
            ? SingleChildScrollView(child: SelectableText(snapshot.data!))
            : const Center(child: CircularProgressIndicator()),
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Done'),
      ),
    ],
  );
}

/// Readiness is separate from authorization: Full Access remains the only
/// Ditch permission. Setup buttons request macOS permissions from StatusHost.
class DesktopIntegrationStatus extends StatefulWidget {
  const DesktopIntegrationStatus({
    required this.client,
    required this.fullAccess,
    required this.isRemote,
    this.agentId,
    super.key,
    this.toolsDeclared,
    this.visualizer = false,
    this.onVisualizerChanged,
    this.appliesNextTurn = false,
    this.compact = false,
  });
  final DitchRuntimeClient client;
  final String? agentId;
  final bool fullAccess;
  final bool isRemote;
  final bool? toolsDeclared;
  final bool visualizer;
  final ValueChanged<bool>? onVisualizerChanged;
  final bool appliesNextTurn;
  final bool compact;

  @override
  State<DesktopIntegrationStatus> createState() =>
      _DesktopIntegrationStatusState();
}

class _DesktopIntegrationStatusState extends State<DesktopIntegrationStatus> {
  Map<String, dynamic>? _status;
  String? _error;
  bool _loading = false;
  bool _visualizerSupported = false;
  Timer? _timer;
  StreamSubscription<Map<String, dynamic>>? _events;
  String? _activity;

  @override
  void initState() {
    super.initState();
    unawaited(_refresh());
    _timer = Timer.periodic(
      const Duration(seconds: 4),
      (_) => unawaited(_refresh()),
    );
  }

  @override
  void didUpdateWidget(DesktopIntegrationStatus oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.agentId != widget.agentId ||
        oldWidget.fullAccess != widget.fullAccess ||
        oldWidget.isRemote != widget.isRemote) {
      _activity = null;
      unawaited(_refresh());
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    _events?.cancel();
    super.dispose();
  }

  Future<void> _refresh() async {
    if (_loading || widget.isRemote) return;
    _loading = true;
    final agentId = widget.agentId;
    try {
      final runtime = await widget.client.runtimeStatus();
      _visualizerSupported = runtime.capabilities.contains(
        'desktop_visualizer_v1',
      );
      if (!runtime.capabilities.contains('tool_integrations_v1')) {
        if (mounted) {
          setState(() {
            _status = null;
            _error = 'Update the Ditch runtime';
          });
        }
        return;
      }
      _events ??= widget.client.subscribeToolEvents().listen(
        (event) {
          final value = event['ToolIntegration'];
          if (mounted && value is Map && value['agent_id'] == widget.agentId) {
            setState(
              () => _activity = value['state'] == 'running'
                  ? (value['tool'] == 'desktop_act'
                        ? 'Acting on desktop'
                        : 'Observing desktop')
                  : null,
            );
          }
        },
        onError: (_) {
          _events?.cancel();
          _events = null;
        },
        onDone: () {
          _events = null;
        },
      );
      final response = await widget.client.request({
        'ToolIntegration': {
          'Status': {'agent_id': agentId},
        },
      });
      final value = response['ToolIntegration']?['Status'];
      if (mounted && agentId == widget.agentId) {
        setState(() {
          _status = value is Map ? Map<String, dynamic>.from(value) : null;
          _error = null;
        });
      }
    } catch (_) {
      if (mounted) {
        setState(() {
          _status = null;
          _error = 'Runtime unavailable';
        });
      }
    } finally {
      _loading = false;
    }
  }

  String get _label {
    if (widget.isRemote) return 'Computer use · local Mac only';
    if (!widget.fullAccess) return 'Computer use · requires Full Access';
    if (widget.toolsDeclared == false && _status?['eligible'] != true) {
      return 'Computer use · model support unverified';
    }
    if (_activity != null) return 'Computer use · $_activity';
    if (_error != null) return 'Computer use · $_error';
    final status = _status;
    if (status == null) return 'Computer use · checking…';
    if (status['native_available'] != true) return 'Computer use · unavailable';
    if (status['state'] == 'busy') return 'Computer use · busy';
    if (status['state'] == 'desktopInactive') {
      return 'Computer use · desktop inactive';
    }
    if (status['accessibility'] != true || status['screen_recording'] != true) {
      return 'Computer use · macOS permissions required';
    }
    if (status['eligible'] == true) {
      return status['tools_registered'] == true &&
              status['adapter_connected'] == true
          ? 'Computer use · Peekaboo connected'
          : 'Computer use · connecting Peekaboo tools';
    }
    return 'Computer use · available for next turn';
  }

  Future<void> _setup() async {
    await showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Computer use on this Mac'),
        content: SizedBox(
          width: 400,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Grant macOS permissions to ${_status?['host_name'] ?? 'Ditch Runtime'}. '
                'Ditch cannot grant these permissions for you. The selected model receives observations of the windows it uses.',
              ),
              const SizedBox(height: 12),
              const Text(
                'Full Access authorizes computer use. The conversation’s Stop button also stops computer use. '
                'macOS may require restarting Ditch after changing permissions.',
              ),
              const SizedBox(height: 12),
              for (final permission in const {
                'Accessibility': 'Accessibility',
                'ScreenRecording': 'Screen Recording',
                'PostEvent': 'Input control',
              }.entries)
                TextButton(
                  onPressed: () async {
                    try {
                      await widget.client.request({
                        'ToolIntegration': {
                          'RequestSystemPermission': {
                            'permission': permission.key,
                          },
                        },
                      });
                    } catch (error) {
                      if (context.mounted) {
                        ScaffoldMessenger.of(context).showSnackBar(
                          SnackBar(
                            content: Text(
                              'Could not open permission setup: $error',
                            ),
                          ),
                        );
                      }
                    }
                  },
                  child: Text('Set up ${permission.value}'),
                ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Done'),
          ),
        ],
      ),
    );
    if (mounted) await _refresh();
  }

  String? get _visualizerUnavailable {
    if (widget.isRemote) return 'Available for local Mac sessions';
    if (!widget.fullAccess) return 'Requires Full Access';
    if (_error != null) return _error;
    if (!_visualizerSupported) {
      return 'Requires a runtime with visualizer support';
    }
    if (_status?['native_available'] != true) return 'Computer use unavailable';
    if (_status?['state'] == 'desktopInactive') return 'Desktop is inactive';
    if (widget.toolsDeclared == false && _status?['eligible'] != true) {
      return 'Model tool support is unverified';
    }
    if (_status?['accessibility'] != true ||
        _status?['screen_recording'] != true) {
      return 'Grant macOS computer-use permissions first';
    }
    return null;
  }

  String get _compactLabel {
    if (_label.contains('Peekaboo connected')) return 'Peekaboo';
    if (_label.contains('connecting')) return 'Connecting';
    if (_label.contains('available for next turn')) return 'Available';
    if (_activity != null) return 'Active';
    if (_label.contains('checking')) return 'Checking';
    if (_label.contains('busy')) return 'Busy';
    return 'Setup';
  }

  @override
  Widget build(BuildContext context) => Wrap(
    spacing: 4,
    runSpacing: 4,
    crossAxisAlignment: WrapCrossAlignment.center,
    children: [
      Tooltip(
        message: _label,
        child: TextButton.icon(
          onPressed: widget.isRemote ? null : _setup,
          icon: const Icon(Icons.desktop_mac_outlined, size: 14),
          label: widget.compact ? const SizedBox.shrink() : Text(_compactLabel),
          style: TextButton.styleFrom(
            foregroundColor: context.ditch.muted,
            minimumSize: const Size(0, 28),
            padding: const EdgeInsets.symmetric(horizontal: 6),
            tapTargetSize: MaterialTapTargetSize.shrinkWrap,
            textStyle: Theme.of(context).textTheme.labelSmall?.copyWith(
              fontSize: 12,
              fontWeight: FontWeight.w400,
            ),
          ),
        ),
      ),
      if (widget.onVisualizerChanged != null)
        Tooltip(
          message:
              _visualizerUnavailable ??
              (widget.appliesNextTurn
                  ? 'Show computer-use targets and actions on the next turn'
                  : 'Show computer-use targets and actions for this turn'),
          child: Semantics(
            label: 'Computer-use visualizer',
            toggled: widget.visualizer,
            child: TextButton.icon(
              key: const Key('computer-use-visualizer'),
              onPressed: _visualizerUnavailable == null
                  ? () => widget.onVisualizerChanged!(!widget.visualizer)
                  : null,
              icon: Icon(
                widget.visualizer
                    ? Icons.visibility
                    : Icons.visibility_off_outlined,
                size: 14,
              ),
              label: widget.compact
                  ? const SizedBox.shrink()
                  : const Text('Visualizer'),
              style: TextButton.styleFrom(
                foregroundColor: widget.visualizer
                    ? context.ditch.accent
                    : context.ditch.muted,
                backgroundColor: widget.visualizer
                    ? context.ditch.accent.withValues(alpha: 0.08)
                    : null,
                minimumSize: const Size(0, 28),
                padding: const EdgeInsets.symmetric(horizontal: 6),
                tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                textStyle: Theme.of(context).textTheme.labelSmall?.copyWith(
                  fontSize: 12,
                  fontWeight: FontWeight.w400,
                ),
              ),
            ),
          ),
        ),
    ],
  );
}
