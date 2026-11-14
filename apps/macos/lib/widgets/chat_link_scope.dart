import 'package:flutter/widgets.dart';

/// The project belonging to the visible conversation, shared by compact and
/// enlarged chat layouts. Kept separate from prompt drafts and message storage.
class ChatLinkScope extends InheritedWidget {
  const ChatLinkScope({
    required this.projectPath,
    this.isRemote = false,
    this.remoteHost,
    required super.child,
    super.key,
  });

  final String projectPath;
  final bool isRemote;
  final String? remoteHost;

  static ChatLinkScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ChatLinkScope>();

  @override
  bool updateShouldNotify(ChatLinkScope oldWidget) =>
      projectPath != oldWidget.projectPath ||
      isRemote != oldWidget.isRemote ||
      remoteHost != oldWidget.remoteHost;
}
