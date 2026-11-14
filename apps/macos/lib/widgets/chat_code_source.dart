import 'package:flutter/material.dart';

/// Choose a fence longer than any backtick run in the source, so copying
/// Mermaid labels containing backticks still produces valid Markdown.
String mermaidMarkdown(String source) {
  var length = 3;
  for (final match in RegExp(r'`+').allMatches(source)) {
    if (match.group(0)!.length >= length) length = match.group(0)!.length + 1;
  }
  final fence = '`' * length;
  return '${fence}mermaid\n$source${source.endsWith('\n') ? '' : '\n'}$fence\n';
}

/// Selectable through the surrounding message's SelectionArea.
class ChatCodeSource extends StatelessWidget {
  const ChatCodeSource({required this.source, super.key});

  final String source;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
    scrollDirection: Axis.horizontal,
    primary: false,
    padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
    child: Text(
      source.endsWith('\n') ? source.substring(0, source.length - 1) : source,
      style: Theme.of(context).textTheme.bodySmall?.copyWith(
        fontFamily: 'SF Mono',
        fontSize: 12.5,
        height: 1.45,
      ),
    ),
  );
}
