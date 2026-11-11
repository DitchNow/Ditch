part of 'main.dart';

class AddOssModelDialog extends StatefulWidget {
  const AddOssModelDialog({
    required this.client,
    required this.projectId,
    super.key,
  });
  final DitchRuntimeClient client;
  final String projectId;
  @override
  State<AddOssModelDialog> createState() => _AddOssModelDialogState();
}

class _AddOssModelDialogState extends State<AddOssModelDialog> {
  final _name = TextEditingController();
  final _url = TextEditingController();
  final _token = TextEditingController();
  final _manualModel = TextEditingController();
  List<String> _models = [];
  String? _model;
  bool _manual = true;
  bool _privateHttp = false;
  bool _busy = false;
  String? _status;
  String? _error;

  @override
  void dispose() {
    for (final controller in [_name, _url, _token, _manualModel]) {
      controller.dispose();
    }
    super.dispose();
  }

  String get _exactModel => _manual ? _manualModel.text.trim() : _model ?? '';
  Map<String, dynamic> get _connection => {
    'api_url': _url.text.trim(),
    'auth': _token.text.isEmpty ? null : _token.text,
    'allow_private_http': _privateHttp,
  };
  void _invalidate(String _) => setState(() {
    _status = null;
    _models = [];
    _model = null;
    _manual = true;
  });

  Future<void> _test() async {
    if (_url.text.trim().isEmpty) {
      setState(() => _error = 'Enter the Ollama API URL.');
      return;
    }
    final exactModel = _exactModel;
    setState(() {
      _busy = true;
      _error = null;
      _status = null;
    });
    try {
      final response = await widget.client.ollamaRequest(widget.projectId, {
        'Test': {
          ..._connection,
          'model': exactModel.isEmpty ? null : exactModel,
        },
      });
      if (!mounted) return;
      final probe = Map<String, dynamic>.from(response['Probe'] as Map);
      setState(() {
        _models = ((probe['models'] as List?) ?? [])
            .map((v) => v.toString())
            .toSet()
            .toList();
        if (_models.isNotEmpty && exactModel.isEmpty) {
          _manual = false;
          _model = _models.first;
        }
        _status = probe['responses_tool_check'] == true
            ? 'Responses streaming and tool check passed. Full coding compatibility still requires verification.'
            : probe['reachable'] == true
            ? 'Server reachable. Select a model and test again to check Responses and tools.'
            : null;
        _error = probe['error']?.toString();
      });
    } on Object catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _add() async {
    final model = _exactModel;
    if (_name.text.trim().isEmpty ||
        _url.text.trim().isEmpty ||
        model.isEmpty) {
      setState(() => _error = 'Enter a name, API URL and exact model ID.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final response = await widget.client.ollamaRequest(widget.projectId, {
        'Add': {
          ..._connection,
          'display_name': _name.text.trim(),
          'model': model,
        },
      });
      if (!mounted) return;
      final endpoint = Map<String, dynamic>.from(response['Endpoint'] as Map);
      _token.clear();
      Navigator.of(context).pop(
        AgentModelOption(
          id: model,
          displayName: model,
          endpointId: endpoint['id'].toString(),
          endpointName: endpoint['display_name'].toString(),
          availabilityError: endpoint['error']?.toString(),
        ),
      );
    } on Object catch (error) {
      if (mounted) setState(() => _error = '$error');
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: const Text('Add OSS model'),
    content: SizedBox(
      width: 420,
      child: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Text(
              'Connection checks run on the computer hosting this project.',
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _name,
              enabled: !_busy,
              decoration: const InputDecoration(
                labelText: 'Name',
                hintText: 'GPU Server A',
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _url,
              enabled: !_busy,
              onChanged: _invalidate,
              decoration: const InputDecoration(
                labelText: 'Ollama API URL',
                hintText: 'https://gpu.example.com',
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _token,
              enabled: !_busy,
              obscureText: true,
              enableSuggestions: false,
              autocorrect: false,
              onChanged: _invalidate,
              decoration: const InputDecoration(
                labelText: 'API token (optional)',
              ),
            ),
            CheckboxListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Allow HTTP on a private network'),
              value: _privateHttp,
              onChanged: _busy
                  ? null
                  : (value) => setState(() {
                      _privateHttp = value ?? false;
                      _status = null;
                    }),
            ),
            OutlinedButton(
              onPressed: _busy ? null : _test,
              child: const Text('Test connection'),
            ),
            if (_status != null)
              Padding(
                padding: const EdgeInsets.only(top: 8),
                child: Text(_status!),
              ),
            const SizedBox(height: 12),
            if (_models.isNotEmpty)
              DropdownButtonFormField<String>(
                initialValue: _manual ? '__manual__' : _model,
                key: ValueKey('${_models.join(',')}|$_manual|$_model'),
                isExpanded: true,
                decoration: const InputDecoration(labelText: 'Model'),
                items: [
                  ..._models.map(
                    (m) => DropdownMenuItem(
                      value: m,
                      child: Text(m, overflow: TextOverflow.ellipsis),
                    ),
                  ),
                  const DropdownMenuItem(
                    value: '__manual__',
                    child: Text('Enter exact model ID…'),
                  ),
                ],
                onChanged: _busy
                    ? null
                    : (value) => setState(() {
                        _manual = value == '__manual__';
                        _model = _manual ? null : value;
                        _status = null;
                      }),
              ),
            if (_manual)
              TextField(
                controller: _manualModel,
                enabled: !_busy,
                onChanged: (_) => setState(() => _status = null),
                decoration: const InputDecoration(
                  labelText: 'Exact model ID',
                  hintText: 'qwen3.5:9b',
                ),
              ),
            if (_error != null)
              Padding(
                padding: const EdgeInsets.only(top: 12),
                child: Text(
                  _error!,
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ),
            if (_busy)
              const Padding(
                padding: EdgeInsets.only(top: 12),
                child: LinearProgressIndicator(),
              ),
          ],
        ),
      ),
    ),
    actions: [
      TextButton(
        onPressed: _busy ? null : () => Navigator.of(context).pop(),
        child: const Text('Cancel'),
      ),
      FilledButton(onPressed: _busy ? null : _add, child: const Text('Add')),
    ],
  );
}
