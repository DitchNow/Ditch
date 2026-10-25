import 'package:flutter/widgets.dart';

/// Session-only text. It deliberately has no listeners, timers, or disk writes.
class AgentPromptDraft {
  AgentPromptDraft(this.text);

  String text;
}

class AgentPromptDrafts {
  final _agents = <String, AgentPromptDraft>{};
  final _projects = <String, AgentPromptDraft>{};

  AgentPromptDraft forAgent(String id, String initialText) =>
      _agents.putIfAbsent(id, () => AgentPromptDraft(initialText));

  AgentPromptDraft forNewAgent(String projectKey, String initialText) =>
      _projects.putIfAbsent(projectKey, () => AgentPromptDraft(initialText));

  void removeAgent(String id) => _agents.remove(id);

  void removeProject(String projectKey) => _projects.remove(projectKey);

  void retainAgents(Set<String> ids) =>
      _agents.removeWhere((id, _) => !ids.contains(id));

  void retainProjects(Set<String> keys) =>
      _projects.removeWhere((key, _) => !keys.contains(key));
}

class AgentPromptDraftScope extends InheritedWidget {
  const AgentPromptDraftScope({
    required this.drafts,
    required this.projectKey,
    required super.child,
    super.key,
  });

  final AgentPromptDrafts drafts;
  final String projectKey;

  static AgentPromptDraftScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AgentPromptDraftScope>();

  AgentPromptDraft forConversation(String id, String initialText) =>
      id == 'new-agent'
      ? drafts.forNewAgent(projectKey, initialText)
      : drafts.forAgent(id, initialText);

  @override
  bool updateShouldNotify(AgentPromptDraftScope oldWidget) =>
      drafts != oldWidget.drafts || projectKey != oldWidget.projectKey;
}
