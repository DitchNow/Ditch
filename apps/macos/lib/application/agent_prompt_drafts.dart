import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import '../data/prompt_attachment.dart';

/// Session-only prompt and pending submission, shared across composer remounts.
class AgentPromptDraft extends ChangeNotifier {
  AgentPromptDraft(this._text);

  String _text;
  int revision = 0;
  bool submitting = false;
  String get text => _text;
  set text(String value) {
    if (_text == value) return;
    _text = value;
    revision++;
  }

  final attachments = <PromptAttachment>[];

  bool beginSubmission() {
    if (submitting) return false;
    submitting = true;
    notifyListeners();
    return true;
  }

  void finishSubmission({
    required bool accepted,
    required int submittedRevision,
    required List<PromptAttachment> submittedAttachments,
  }) {
    // An acknowledgement belongs to the captured draft, even after navigation.
    // Keep all newer edits, including text edited and then restored to its old value.
    if (accepted &&
        revision == submittedRevision &&
        listEquals(attachments, submittedAttachments)) {
      text = '';
      attachments.clear();
    }
    submitting = false;
    notifyListeners();
  }
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
    this.isRemote = false,
    required super.child,
    super.key,
  });

  final AgentPromptDrafts drafts;
  final String projectKey;
  final bool isRemote;

  static AgentPromptDraftScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AgentPromptDraftScope>();

  AgentPromptDraft forConversation(String id, String initialText) =>
      id == 'new-agent'
      ? drafts.forNewAgent(projectKey, initialText)
      : drafts.forAgent(id, initialText);

  @override
  bool updateShouldNotify(AgentPromptDraftScope oldWidget) =>
      drafts != oldWidget.drafts ||
      projectKey != oldWidget.projectKey ||
      isRemote != oldWidget.isRemote;
}
