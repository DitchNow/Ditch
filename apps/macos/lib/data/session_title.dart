const sessionTitlePending = 'Naming session…';
const sessionTitleFailed = 'Unnamed session';
const sessionTitleMaxUtf16 = 240;

String? normalizeSessionTitle(String? value) {
  if (value == null) return null;
  final compact = value.trim().replaceAll(RegExp(r'\s+'), ' ');
  final result = StringBuffer();
  var units = 0;
  for (final rune in compact.runes) {
    units += rune > 0xffff ? 2 : 1;
    if (units > sessionTitleMaxUtf16) break;
    result.writeCharCode(rune);
  }
  final title = result.toString().trimRight();
  return title.isEmpty ? null : title;
}

String sessionDisplayName(
  String? userTitle,
  String? codexName, {
  bool namingFailed = false,
}) =>
    normalizeSessionTitle(userTitle) ??
    normalizeSessionTitle(codexName) ??
    (namingFailed ? sessionTitleFailed : sessionTitlePending);
