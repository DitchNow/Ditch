import 'dart:async';
import 'package:flutter/material.dart';
import '../data/runtime_transport.dart';

enum ApprovalChoice { session, cancel, once }

extension ApprovalChoiceLabel on ApprovalChoice {
  String get label => switch (this) {
    ApprovalChoice.session => 'Approve in this session',
    ApprovalChoice.cancel => 'Cancel',
    ApprovalChoice.once => 'Approve once',
  };
  String get result => switch (this) {
    ApprovalChoice.session => 'Approved in this session',
    ApprovalChoice.cancel => 'Cancelled',
    ApprovalChoice.once => 'Approved once',
  };
}

/// UI state keyed by the runtime request ID, never part of the agent prompt.
class InlineApproval extends ChangeNotifier {
  InlineApproval({
    required this.request,
    required this.respond,
    required this.refresh,
    this.isAvailable,
  });
  final Map<String, dynamic> request;
  final Future<void> Function(ApprovalChoice) respond;
  final Future<void> Function() refresh;
  final bool Function()? isAvailable;
  bool get available => isAvailable?.call() ?? true;
  String get id => request['id'].toString();
  DateTime get createdAt =>
      DateTime.tryParse(request['created_at']?.toString() ?? '') ??
      DateTime.fromMillisecondsSinceEpoch(0);
  DateTime? get expiresAt =>
      DateTime.tryParse(request['expires_at']?.toString() ?? '');
  bool get expired => expiresAt?.isBefore(DateTime.now()) ?? false;
  bool submitting = false;
  bool uncertain = false;
  String? resolution;
  String? error;
  bool get pending => resolution == null && !expired;
  bool get actionable => pending && available && !submitting && !uncertain;

  void resolve([String label = 'Resolved']) {
    resolution ??= label;
    error = null;
    notifyListeners();
  }

  Future<void> choose(ApprovalChoice choice) async {
    if (!actionable) return;
    submitting = true;
    error = null;
    notifyListeners();
    try {
      await respond(choice);
      // A resolution event may arrive before the command acknowledgement.
      resolution = choice.result;
    } on DitchRuntimeException catch (caught) {
      if (caught.code == 'permission_not_found') {
        resolution = 'No longer awaiting approval';
      } else {
        error = caught.message;
        uncertain = true;
      }
    } on Object {
      error = 'Connection interrupted. The result is not known yet.';
      uncertain = true;
    } finally {
      submitting = false;
      notifyListeners();
    }
  }

  Future<void> checkStatus() async {
    if (submitting) return;
    submitting = true;
    notifyListeners();
    try {
      await refresh();
    } on Object {
      error = 'Could not check the request. Reconnect and check again.';
    } finally {
      submitting = false;
      notifyListeners();
    }
  }
}

class InlineApprovalCard extends StatefulWidget {
  const InlineApprovalCard({super.key, required this.approval});
  final InlineApproval approval;
  @override
  State<InlineApprovalCard> createState() => _InlineApprovalCardState();
}

class _InlineApprovalCardState extends State<InlineApprovalCard> {
  Timer? _expiry;
  @override
  void initState() {
    super.initState();
    final expires = widget.approval.expiresAt;
    if (expires != null && expires.isAfter(DateTime.now())) {
      _expiry = Timer(expires.difference(DateTime.now()), () {
        if (mounted) setState(() {});
      });
    }
  }

  @override
  void dispose() {
    _expiry?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: widget.approval,
    builder: (context, _) {
      final approval = widget.approval;
      return Card(
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'Approval required',
                style: Theme.of(context).textTheme.titleSmall,
              ),
              const SizedBox(height: 8),
              Text(
                approval.request['summary']?.toString() ??
                    'The agent is requesting permission.',
              ),
              if (approval.request['command'] case final String command
                  when command.isNotEmpty) ...[
                const SizedBox(height: 8),
                SelectableText(
                  command,
                  style: const TextStyle(fontFamily: 'monospace'),
                ),
              ],
              if (approval.request['target'] case final String target
                  when target.isNotEmpty) ...[
                const SizedBox(height: 8),
                Text('Target: $target'),
              ],
              const SizedBox(height: 12),
              if (approval.resolution != null || approval.expired)
                Text(approval.resolution ?? 'Expired')
              else ...[
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final choice in ApprovalChoice.values)
                      OutlinedButton(
                        onPressed: approval.actionable
                            ? () => approval.choose(choice)
                            : null,
                        child: Text(choice.label),
                      ),
                  ],
                ),
                if (!approval.available)
                  const Text('Reconnect to respond to this request.'),
                if (approval.submitting) const Text('Waiting for the runtime…'),
                if (approval.error != null) Text(approval.error!),
                if (approval.uncertain) ...[
                  const Text(
                    'Awaiting confirmation. This request will not be sent again.',
                  ),
                  TextButton(
                    onPressed: approval.submitting
                        ? null
                        : approval.checkStatus,
                    child: const Text('Check status'),
                  ),
                ],
              ],
            ],
          ),
        ),
      );
    },
  );
}
