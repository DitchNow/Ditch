import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:qr_flutter/qr_flutter.dart';
import 'package:the_ditch/main.dart';

class CommercialEditionSurface implements EditionSurface {
  const CommercialEditionSurface();

  @override
  List<EditionSettingsSection> settingsSections(DitchRuntimeClient client) => [
    EditionSettingsSection(
      id: 'remote-mobile',
      icon: Icons.phone_iphone,
      title: 'Remote Control',
      subtitle: 'iPhone pairing and device access',
      dialogBuilder: (client) => RemoteSettingsDialog(client: client),
    ),
  ];
}

class RemoteSettingsDialog extends StatefulWidget {
  const RemoteSettingsDialog({
    required this.client,
    this.sshAlias,
    this.deploymentEnvironment = ditchDeploymentEnvironment,
    this.relayOrigin = ditchRelayOrigin,
    super.key,
  });

  final DitchRuntimeClient client;
  final String? sshAlias;
  final String deploymentEnvironment;
  final String relayOrigin;

  @override
  State<RemoteSettingsDialog> createState() => _RemoteSettingsDialogState();
}

class _RemoteSettingsDialogState extends State<RemoteSettingsDialog> {
  static const _applicationChannel = MethodChannel('the_ditch/application');
  Map<String, dynamic>? _status;
  Map<String, dynamic>? _entitlement;
  Map<String, dynamic>? _pairing;
  Timer? _timer;
  Timer? _entitlementRetryTimer;
  bool _busy = false;
  bool _billingBusy = false;
  bool _refreshing = false;
  String? _error;
  String? _errorCode;
  String? _billingError;

  @override
  void initState() {
    super.initState();
    unawaited(_refresh());
    _timer = Timer.periodic(const Duration(seconds: 1), (_) {
      final state = _pairing?['state'];
      if (state == 'pending' || state == 'claimed') unawaited(_pollPairing());
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    _entitlementRetryTimer?.cancel();
    super.dispose();
  }

  void _scheduleEntitlementRetry() {
    _entitlementRetryTimer?.cancel();
    _entitlementRetryTimer = Timer(const Duration(seconds: 2), () {
      if (mounted) unawaited(_refresh());
    });
  }

  Future<Map<String, dynamic>> _requestLocalOrRemote(
    Object local,
    String remoteName,
    Map<String, dynamic> remoteBody,
  ) {
    final alias = widget.sshAlias;
    return widget.client.request(
      alias == null
          ? local
          : {
              remoteName: {'alias': alias, ...remoteBody},
            },
    );
  }

  Future<void> _refresh() async {
    if (_refreshing) return;
    if (mounted) setState(() => _refreshing = true);
    Map<String, dynamic>? entitlement;
    Object? entitlementError;
    try {
      entitlement = await widget.client.commercialEntitlement();
    } on Object catch (error) {
      entitlementError = error;
    }
    try {
      final response = await _requestLocalOrRemote(
        'RemoteControlStatus',
        'RemoteMachineControlStatus',
        const {},
      );
      if (!mounted) return;
      setState(() {
        _entitlement = entitlement;
        _status = entitlementError == null
            ? (response['RemoteControlStatus'] as Map?)?.cast<String, dynamic>()
            : null;
        _error = entitlementError == null
            ? null
            : _friendlyHostedServiceError(entitlementError);
        _errorCode = entitlementError is DitchRuntimeException
            ? entitlementError.code
            : entitlementError == null
            ? null
            : 'commercial_entitlement_unavailable';
        _refreshing = false;
        if (_errorCode == 'commercial_entitlement_loading') {
          _scheduleEntitlementRetry();
        }
      });
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _entitlement = entitlement;
          _status = null;
          final reportedError = entitlementError ?? error;
          _error = _friendlyHostedServiceError(reportedError);
          _errorCode = reportedError is DitchRuntimeException
              ? reportedError.code
              : 'commercial_entitlement_unavailable';
          _refreshing = false;
          if (_errorCode == 'commercial_entitlement_loading') {
            _scheduleEntitlementRetry();
          }
        });
      }
    }
  }

  Future<void> _connect() async {
    setState(() => _busy = true);
    try {
      final response = await _requestLocalOrRemote(
        'CreateRemotePairing',
        'CreateRemoteMachinePairing',
        const {},
      );
      if (!mounted) return;
      setState(() {
        _pairing = (response['RemotePairing'] as Map?)?.cast<String, dynamic>();
        _busy = false;
        _error = null;
        _errorCode = null;
      });
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = _friendlyHostedServiceError(error);
          _errorCode = error is DitchRuntimeException ? error.code : null;
        });
      }
    }
  }

  Future<void> _pollPairing() async {
    final id = _pairing?['pairing_id']?.toString();
    if (id == null || _busy) return;
    try {
      final response = await _requestLocalOrRemote(
        {
          'GetRemotePairing': {'pairing_id': id},
        },
        'GetRemoteMachinePairing',
        {'pairing_id': id},
      );
      if (!mounted) return;
      final next = (response['RemotePairing'] as Map?)?.cast<String, dynamic>();
      if (next != null) {
        next['qr_payload'] ??= _pairing?['qr_payload'];
        setState(() => _pairing = next);
      }
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _error = _friendlyHostedServiceError(error);
          _errorCode = error is DitchRuntimeException ? error.code : null;
        });
      }
    }
  }

  Future<void> _confirm() async {
    final id = _pairing?['pairing_id']?.toString();
    if (id == null) return;
    setState(() => _busy = true);
    try {
      await _requestLocalOrRemote(
        {
          'ConfirmRemotePairing': {'pairing_id': id},
        },
        'ConfirmRemoteMachinePairing',
        {'pairing_id': id},
      );
      if (mounted) {
        setState(() {
          _pairing = null;
          _busy = false;
        });
      }
      await _refresh();
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = _friendlyHostedServiceError(error);
          _errorCode = error is DitchRuntimeException ? error.code : null;
        });
      }
    }
  }

  Future<void> _cancel() async {
    final id = _pairing?['pairing_id']?.toString();
    if (id != null) {
      await _requestLocalOrRemote(
        {
          'CancelRemotePairing': {'pairing_id': id},
        },
        'CancelRemoteMachinePairing',
        {'pairing_id': id},
      );
    }
    if (mounted) setState(() => _pairing = null);
  }

  Future<void> _revoke(String id) async {
    await widget.client.request({
      'RevokeRemoteDevice': {'device_id': id},
    });
    await _refresh();
  }

  Future<void> _openBillingManagement() async {
    setState(() {
      _billingBusy = true;
      _billingError = null;
    });
    try {
      final response = await widget.client.request(
        'CommercialBillingManagement',
      );
      final session = (response['CommercialBillingManagement'] as Map?)
          ?.cast<String, dynamic>();
      final url = session?['billing_management_url']?.toString();
      if (url == null || url.isEmpty) {
        throw const FormatException(
          'Ditch did not provide a billing-management URL.',
        );
      }
      final opened = await _applicationChannel.invokeMethod<bool>(
        'openURL',
        url,
      );
      if (opened != true) {
        throw const FormatException(
          'The billing-management page could not be opened.',
        );
      }
      if (mounted) setState(() => _billingBusy = false);
    } on Object catch (error) {
      if (mounted) {
        setState(() {
          _billingBusy = false;
          _billingError = '$error';
        });
      }
    }
  }

  Future<void> _viewCommercialPlans() async {
    await showDialog<void>(
      context: context,
      builder: (context) => CommercialUpgradeDialog(
        client: widget.client,
        deploymentEnvironment: widget.deploymentEnvironment,
        relayOrigin: widget.relayOrigin,
      ),
    );
    if (mounted) unawaited(_refresh());
  }

  String _friendlyHostedServiceError(Object error) {
    if (error is DitchRuntimeException) {
      return switch (error.code) {
        'official_build_required' =>
          'This source build cannot use DitchNow hosted services. Install an official signed Community or Commercial build to use Remote Control.',
        'commercial_entitlement_loading' =>
          'Activating Remote Control. This normally takes only a few seconds.',
        'commercial_entitlement_unavailable' ||
        'commercial_entitlement_failed' =>
          'Ditch could not verify Commercial access. Check the connection and try again.',
        'commercial_entitlement_required' => error.message,
        _ => error.message,
      };
    }
    if (error is FormatException) return error.message;
    return 'Remote Control is temporarily unavailable. Try again.';
  }

  @override
  Widget build(BuildContext context) {
    final license = _entitlement == null
        ? null
        : DitchCurrentLicense.fromEntitlement(_entitlement!);
    final confirmedEntitlementRequired =
        (license != null && !license.hasCommercialAccess) ||
        (license == null && _errorCode == 'commercial_entitlement_required');
    final entitlementMismatch =
        license?.hasCommercialAccess == true &&
        _errorCode == 'commercial_entitlement_required';
    final remoteReady =
        !confirmedEntitlementRequired && _status != null && _error == null;
    final billingManagementAvailable =
        _entitlement?['billing_management_available'] == true;
    final renewalAvailable = _entitlement?['renewal_available'] == true;
    final devices =
        (_status?['devices'] as List?)?.whereType<Map>().toList() ?? const [];
    return AlertDialog(
      icon: const Icon(Icons.phone_iphone),
      title: Text(
        widget.sshAlias == null
            ? 'Remote Control'
            : 'Enroll ${widget.sshAlias}',
      ),
      content: SizedBox(
        width: 580,
        child: confirmedEntitlementRequired
            ? Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (widget.deploymentEnvironment == 'staging') ...[
                    StagingEnvironmentBanner(
                      key: const Key('remote-staging-environment'),
                      relayOrigin: widget.relayOrigin,
                    ),
                    const SizedBox(height: 12),
                  ],
                  Text(
                    license == null || !license.isCommercial
                        ? 'Commercial access is required for Remote Control.'
                        : '${license.displayName} is ${license.status}.',
                    key: const Key('remote-current-license'),
                    style: Theme.of(context).textTheme.titleSmall,
                  ),
                  const SizedBox(height: 8),
                  const Text('Local and SSH Ditch continue to work.'),
                  const SizedBox(height: 12),
                  FilledButton(
                    key: const Key('renew-commercial'),
                    onPressed: _billingBusy
                        ? null
                        : billingManagementAvailable
                        ? _openBillingManagement
                        : _viewCommercialPlans,
                    child: Text(
                      billingManagementAvailable && renewalAvailable
                          ? 'Renew'
                          : billingManagementAvailable
                          ? 'Manage Billing'
                          : 'View Plans',
                    ),
                  ),
                  if (_billingError != null) ...[
                    const SizedBox(height: 10),
                    SelectableText(
                      _billingError!,
                      style: TextStyle(
                        color: Theme.of(context).colorScheme.error,
                      ),
                    ),
                  ],
                ],
              )
            : _status == null
            ? Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (widget.deploymentEnvironment == 'staging') ...[
                    StagingEnvironmentBanner(
                      key: const Key('remote-staging-environment'),
                      relayOrigin: widget.relayOrigin,
                    ),
                    const SizedBox(height: 12),
                  ],
                  if (_error == null)
                    const Center(child: CircularProgressIndicator())
                  else ...[
                    if (_errorCode == 'commercial_entitlement_loading') ...[
                      const Center(child: CircularProgressIndicator()),
                      const SizedBox(height: 12),
                    ],
                    SelectableText(
                      entitlementMismatch
                          ? 'Ditch is synchronizing Commercial access with the local runtime. Try again in a moment.'
                          : _error!,
                      key: const Key('remote-transient-error'),
                    ),
                    const SizedBox(height: 12),
                    Align(
                      alignment: Alignment.center,
                      child: OutlinedButton(
                        key: const Key('retry-remote-control'),
                        onPressed: _refreshing ? null : _refresh,
                        child: const Text('Retry'),
                      ),
                    ),
                  ],
                ],
              )
            : SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    if (widget.deploymentEnvironment == 'staging') ...[
                      StagingEnvironmentBanner(
                        key: const Key('remote-staging-environment'),
                        relayOrigin: widget.relayOrigin,
                      ),
                      const SizedBox(height: 12),
                    ],
                    ListTile(
                      contentPadding: EdgeInsets.zero,
                      leading: Icon(
                        _status?['online'] == true
                            ? Icons.cloud_done_outlined
                            : Icons.cloud_off_outlined,
                      ),
                      title: Text(
                        _status?['machine_name']?.toString() ??
                            widget.sshAlias ??
                            'This Mac',
                      ),
                      subtitle: Text(
                        _status?['online'] == true ? 'Online' : 'Offline',
                      ),
                    ),
                    const Divider(),
                    Text(
                      'iPhones with access',
                      style: Theme.of(context).textTheme.titleSmall,
                    ),
                    if (devices.isEmpty)
                      const Padding(
                        padding: EdgeInsets.symmetric(vertical: 12),
                        child: Text('No iPhones have access to this Mac.'),
                      ),
                    ...devices.map((raw) {
                      final device = raw.cast<Object?, Object?>();
                      final deviceId =
                          device['device_id']?.toString() ?? 'Unavailable';
                      final deviceState =
                          device['state']?.toString() ?? 'unknown';
                      return ListTile(
                        leading: const Icon(Icons.phone_iphone),
                        title: Text(device['name']?.toString() ?? 'iPhone'),
                        subtitle: Text(
                          'Unique identifier: $deviceId\nStatus: $deviceState',
                        ),
                        isThreeLine: true,
                        trailing:
                            widget.sshAlias == null &&
                                device['state'] == 'active'
                            ? TextButton(
                                onPressed: _busy
                                    ? null
                                    : () => _revoke(deviceId),
                                child: const Text('Revoke'),
                              )
                            : null,
                      );
                    }),
                    if (_pairing != null) _pairingView(),
                    const SizedBox(height: 12),
                    const Text(
                      'Remote payloads are end-to-end encrypted; local source, secrets, and full transcripts are not projected.',
                    ),
                  ],
                ),
              ),
      ),
      actions: [
        if (!confirmedEntitlementRequired && billingManagementAvailable)
          TextButton(
            key: const Key('manage-commercial-billing'),
            onPressed: _billingBusy ? null : _openBillingManagement,
            child: const Text('Manage Billing'),
          ),
        if (_pairing == null && remoteReady)
          FilledButton.icon(
            key: const Key('connect-iphone'),
            onPressed: _busy || _status?['configured'] != true
                ? null
                : _connect,
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

  Widget _pairingView() {
    final state = _pairing?['state']?.toString();
    final qr = _pairing?['qr_payload']?.toString();
    if (state == 'claimed') {
      return Column(
        children: [
          const Text('Confirm the matching code shown on both devices.'),
          const SizedBox(height: 8),
          FilledButton(
            onPressed: _busy ? null : _confirm,
            child: const Text('Confirm iPhone'),
          ),
          TextButton(
            onPressed: _busy ? null : _cancel,
            child: const Text('Cancel'),
          ),
        ],
      );
    }
    return Column(
      children: [
        const Divider(),
        if (qr != null) QrImageView(data: qr, size: 220),
        const Text('Scan with the proprietary Ditch iPhone app.'),
        TextButton(
          onPressed: _busy ? null : _cancel,
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}
