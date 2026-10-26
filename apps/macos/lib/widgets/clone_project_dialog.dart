import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import '../application/project_clone_controller.dart';
import '../data/project_clone_models.dart';

class CloneProjectDialog extends StatefulWidget {
  const CloneProjectDialog({required this.client, super.key});
  final ProjectCloneClient client;

  @override
  State<CloneProjectDialog> createState() => _CloneProjectDialogState();
}

class _CloneProjectDialogState extends State<CloneProjectDialog> {
  static const _picker = MethodChannel('the_ditch/project_picker');
  final _url = TextEditingController();
  final _parent = TextEditingController();
  final _folder = TextEditingController();
  final _name = TextEditingController();
  late final ProjectCloneController _clone;
  bool _folderEdited = false;
  bool _nameEdited = false;
  bool _returned = false;
  bool _picking = false;
  String? _pickerError;

  @override
  void initState() {
    super.initState();
    _clone = ProjectCloneController(widget.client)..addListener(_changed);
  }

  void _changed() {
    if (!mounted || _returned) return;
    final project = _clone.status?.project;
    if (_clone.status?.phase == 'completed' && project != null) {
      _returned = true;
      // Rebuild PopScope before popping the finished operation.
      setState(() {});
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) Navigator.pop(context, project);
      });
    } else {
      setState(() {});
    }
  }

  Future<void> _browse() async {
    setState(() {
      _picking = true;
      _pickerError = null;
    });
    try {
      final path = await _picker.invokeMethod<String>('chooseDirectory');
      if (mounted && path != null && path.isNotEmpty) {
        setState(() => _parent.text = path);
      }
    } on Object {
      if (mounted) {
        setState(
          () => _pickerError =
              'Could not open the folder picker. Enter a parent folder path below.',
        );
      }
    } finally {
      if (mounted) setState(() => _picking = false);
    }
  }

  @override
  void dispose() {
    _clone.removeListener(_changed);
    _clone.dispose();
    for (final field in [_url, _parent, _folder, _name]) {
      field.dispose();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final status = _clone.status;
    final retained = status?.checkoutReady == true;
    final editable = !_clone.active && !retained && !_returned;
    final enteredDestination =
        '${_parent.text.replaceFirst(RegExp(r'/+$'), '')}/${_folder.text}';
    final destination = editable
        ? enteredDestination
        : status?.destination ?? enteredDestination;
    return PopScope(
      canPop: !_clone.active || _returned,
      child: AlertDialog(
        title: const Text('Clone from GitHub'),
        content: SizedBox(
          width: 540,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Clone onto this Mac using your existing Git credentials or SSH agent.',
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: _url,
                  enabled: editable,
                  decoration: const InputDecoration(
                    labelText: 'Git URL',
                    hintText: 'https://github.com/owner/repository.git',
                  ),
                  onChanged: (url) => setState(() {
                    final inferred = repositoryFolderName(url);
                    if (!_folderEdited) _folder.text = inferred;
                    if (!_nameEdited) _name.text = inferred;
                  }),
                ),
                const SizedBox(height: 12),
                Row(
                  children: [
                    Expanded(
                      child: TextField(
                        controller: _parent,
                        enabled: editable && !_picking,
                        decoration: const InputDecoration(
                          labelText: 'Destination parent folder',
                        ),
                        onChanged: (_) => setState(() {}),
                      ),
                    ),
                    const SizedBox(width: 8),
                    OutlinedButton(
                      onPressed: editable && !_picking ? _browse : null,
                      child: const Text('Choose…'),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: _folder,
                  enabled: editable,
                  decoration: const InputDecoration(
                    labelText: 'Repository folder name',
                  ),
                  onChanged: (_) => setState(() => _folderEdited = true),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: _name,
                  enabled: editable,
                  decoration: const InputDecoration(labelText: 'Project name'),
                  onChanged: (_) => _nameEdited = true,
                ),
                const SizedBox(height: 12),
                SelectableText('Destination: $destination'),
                if (_clone.active) ...[
                  const SizedBox(height: 16),
                  LinearProgressIndicator(
                    value: _clone.cancelling || _clone.connectionLost
                        ? null
                        : status?.percentage,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    _clone.cancelling
                        ? 'Cancelling and cleaning up…'
                        : status?.phaseLabel ?? 'Starting clone',
                  ),
                  if (status?.progressLabel != null && !_clone.cancelling)
                    Text(
                      '${status!.progressLabel}${status.percentage == null ? '' : ' — ${(status.percentage! * 100).round()}%'}',
                    ),
                ],
                if (_clone.error != null || _pickerError != null) ...[
                  const SizedBox(height: 12),
                  Text(
                    _clone.error ?? _pickerError!,
                    style: TextStyle(
                      color: Theme.of(context).colorScheme.error,
                    ),
                  ),
                ],
                if (retained && status?.phase != 'completed') ...[
                  const SizedBox(height: 8),
                  const Text(
                    'Your checkout is retained at the destination above. You can also add it later with Local Project.',
                  ),
                ],
              ],
            ),
          ),
        ),
        actions: [
          if (_clone.connectionLost)
            TextButton(
              onPressed: () {
                setState(() => _returned = true);
                WidgetsBinding.instance.addPostFrameCallback((_) {
                  if (mounted) Navigator.pop(context);
                });
              },
              child: const Text('Close and check later'),
            ),
          TextButton(
            onPressed: _returned
                ? null
                : _clone.active
                ? (_clone.cancelling ? null : _clone.cancel)
                : () => Navigator.pop(context),
            child: Text(_clone.active ? 'Cancel clone' : 'Close'),
          ),
          FilledButton(
            onPressed: _clone.active || _picking || _returned
                ? null
                : retained
                ? _clone.retryRegistration
                : () => _clone.start(
                    url: _url.text.trim(),
                    parent: _parent.text.trim(),
                    folder: _folder.text.trim(),
                    name: _name.text.trim(),
                  ),
            child: Text(retained ? 'Retry registration' : 'Clone and add'),
          ),
        ],
      ),
    );
  }
}
