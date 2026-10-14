import 'dart:io';

class DitchProject {
  const DitchProject({
    this.id,
    required this.name,
    required this.path,
    this.gitPolicy = ProjectGitPolicy.requireRepository,
    this.remoteMachineId,
    this.sshHostAlias,
  });

  final String? id;
  final String name;
  final String path;
  final ProjectGitPolicy gitPolicy;
  final String? remoteMachineId;
  final String? sshHostAlias;

  bool get isRemote => sshHostAlias != null;
}

enum ProjectGitPolicy {
  requireRepository,
  initializeRepository,
  allowOutsideGit,
}

String canonicalProjectPath(String path) {
  final directory = Directory(path).absolute;
  try {
    return directory.resolveSymbolicLinksSync();
  } on FileSystemException {
    return directory.path;
  }
}

String projectKey(DitchProject project) =>
    project.id ?? projectFallbackKey(project);

String projectFallbackKey(DitchProject project) => project.isRemote
    ? 'remote:${project.sshHostAlias}:${project.path}'
    : canonicalProjectPath(project.path);

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
