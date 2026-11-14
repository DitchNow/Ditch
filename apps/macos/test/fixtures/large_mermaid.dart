/// Generated fixtures; no user conversation history is used.
String largeMermaid({int nodes = 240}) {
  final source = StringBuffer('flowchart TB\n');
  for (var group = 0; group < (nodes / 20).ceil(); group++) {
    source.writeln('subgraph group$group["Architecture module $group"]');
    source.writeln('subgraph services$group["Services"]');
    for (var i = group * 20; i < (group + 1) * 20 && i < nodes; i++) {
      source.writeln('N$i["Service $i with a descriptive architecture label"]');
    }
    source.writeln('end\nend');
  }
  for (var i = 0; i < nodes - 1; i++) {
    source.writeln('N$i --> N${i + 1}');
  }
  return source.toString();
}
