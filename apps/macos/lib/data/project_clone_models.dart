class ProjectCloneStatus {
  const ProjectCloneStatus({
    required this.operationId,
    required this.destination,
    required this.phase,
    this.percentage,
    this.progressLabel,
    this.error,
    this.project,
    this.checkoutReady = false,
  });

  factory ProjectCloneStatus.fromResponse(Map<String, dynamic> response) {
    final value = response['ProjectClone'];
    if (value is! Map ||
        value['operation_id'] is! String ||
        value['destination'] is! String ||
        !const {
          'validating',
          'cloning',
          'checkout',
          'configuring',
          'completed',
          'failed',
          'cancelled',
        }.contains(value['phase'])) {
      throw const FormatException(
        'Clone support is unavailable. Update and restart the Ditch runtime.',
      );
    }
    final project = value['project'];
    if (value['phase'] == 'completed' &&
        (project is! Map ||
            project['id'] is! String ||
            project['root'] is! String ||
            project['name'] is! String)) {
      throw const FormatException(
        'The runtime did not return a registered project. Update and restart it.',
      );
    }
    final percentage = value['percentage'];
    return ProjectCloneStatus(
      operationId: value['operation_id'] as String,
      destination: value['destination'] as String,
      phase: value['phase'] as String,
      percentage: percentage is num && percentage >= 0 && percentage <= 100
          ? percentage.toDouble() / 100
          : null,
      progressLabel: value['progress_label'] as String?,
      error: value['error'] as String?,
      project: project is Map ? Map<String, dynamic>.from(project) : null,
      checkoutReady: value['checkout_ready'] == true,
    );
  }

  final String operationId;
  final String destination;
  final String phase;
  final double? percentage;
  final String? progressLabel;
  final String? error;
  final Map<String, dynamic>? project;
  final bool checkoutReady;

  bool get terminal =>
      const {'completed', 'failed', 'cancelled'}.contains(phase);
  String get phaseLabel => switch (phase) {
    'validating' => 'Validating destination',
    'cloning' => 'Cloning repository',
    'checkout' => 'Checking out files',
    'configuring' => 'Configuring Ditch project',
    'completed' => 'Project ready',
    'cancelled' => 'Clone cancelled',
    _ => 'Setup incomplete',
  };
}

abstract interface class ProjectCloneClient {
  Future<ProjectCloneStatus> startProjectClone({
    required String operationId,
    required String url,
    required String parent,
    required String folder,
    required String name,
  });
  Future<ProjectCloneStatus> projectCloneStatus(String operationId);
  Future<ProjectCloneStatus> cancelProjectClone(String operationId);
  Future<ProjectCloneStatus> retryProjectCloneRegistration(String operationId);
}

String repositoryFolderName(String url) {
  final path = url.trim().replaceAll(RegExp(r'/+$'), '');
  final name = path
      .split(RegExp(r'[/ :]'))
      .last
      .replaceFirst(RegExp(r'\.git$'), '');
  return name.replaceAll(RegExp(r'[^a-zA-Z0-9._-]'), '-');
}

String? validateCloneFields(
  String url,
  String parent,
  String folder,
  String name,
) {
  if (url.isEmpty || RegExp(r'\s|[\x00-\x1f\x7f]').hasMatch(url)) {
    return 'Enter an HTTPS or SSH Git URL.';
  }
  final parsed = Uri.tryParse(url);
  final standard =
      parsed != null &&
      const {'https', 'ssh'}.contains(parsed.scheme) &&
      parsed.host.isNotEmpty &&
      parsed.path.replaceAll('/', '').isNotEmpty &&
      !parsed.hasQuery &&
      !parsed.hasFragment &&
      (parsed.scheme == 'https'
          ? parsed.userInfo.isEmpty
          : !parsed.userInfo.contains(':'));
  final scp = RegExp(
    r'^(?:[a-zA-Z0-9_][a-zA-Z0-9_.-]*@)?[a-zA-Z0-9_][a-zA-Z0-9_.-]*:[^/:?#\s][^:?#\s]*$',
  ).hasMatch(url);
  if (!standard && !scp) {
    return 'Use an HTTPS or SSH Git URL without embedded passwords or tokens.';
  }
  if (!parent.startsWith('/')) return 'Choose a destination parent folder.';
  if (folder.isEmpty ||
      folder == '.' ||
      folder == '..' ||
      folder.startsWith('-') ||
      folder.length > 255 ||
      RegExp(r'[/\\\x00-\x1f\x7f]').hasMatch(folder)) {
    return 'Enter a new repository folder name without slashes.';
  }
  if (name.isEmpty || name.length > 256) {
    return 'Enter a project name (up to 256 characters).';
  }
  return null;
}
