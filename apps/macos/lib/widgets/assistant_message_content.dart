import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_markdown_plus/flutter_markdown_plus.dart';
import 'package:markdown/markdown.dart' as md;

import '../design_system/ditch_theme.dart';
import '../application/chat_links.dart';
import 'assistant_message_image.dart';
import 'chat_code_source.dart';
import 'chat_mermaid_block.dart';
import 'chat_visual_block.dart';

typedef ChatLinkOpener = Future<bool> Function(Uri uri);

Future<bool> openChatLink(Uri uri) async {
  if (chatHttpsUri(uri.toString()) == null) return false;
  return await const MethodChannel(
        'the_ditch/application',
      ).invokeMethod<bool>('openURL', uri.toString()) ??
      false;
}

class AssistantMessageContent extends StatefulWidget {
  const AssistantMessageContent({
    required this.text,
    this.openLink = openChatLink,
    this.fetchImage = fetchChatImage,
    this.revealFile = revealChatFile,
    this.projectPath,
    this.isRemote = false,
    this.remoteHost,
    super.key,
  });
  final String text;
  final ChatLinkOpener openLink;
  final ChatImageFetcher fetchImage;
  final ChatFileRevealer revealFile;
  final String? projectPath;
  final bool isRemote;
  final String? remoteHost;

  @override
  State<AssistantMessageContent> createState() =>
      _AssistantMessageContentState();
}

class _AssistantMessageContentState extends State<AssistantMessageContent> {
  Widget? _body;
  ThemeData? _theme;
  String? _linkError;

  @override
  void didUpdateWidget(AssistantMessageContent oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.text != widget.text ||
        oldWidget.openLink != widget.openLink ||
        oldWidget.fetchImage != widget.fetchImage ||
        oldWidget.revealFile != widget.revealFile ||
        oldWidget.projectPath != widget.projectPath ||
        oldWidget.isRemote != widget.isRemote ||
        oldWidget.remoteHost != widget.remoteHost) {
      _body = null;
      _linkError = null;
    }
  }

  ChatLinkTarget _resolve(String destination) => resolveChatLink(
    destination,
    projectPath: widget.projectPath,
    isRemote: widget.isRemote,
    remoteHost: widget.remoteHost,
  );

  String _tooltip(String destination) => switch (_resolve(destination)) {
    ChatFileLink(:final path, :final withoutLineSuffix) =>
      'Show file in Finder\n${withoutLineSuffix ?? path}',
    _ => destination,
  };

  Future<void> _open(String destination) async {
    final target = _resolve(destination);
    final originalText = widget.text;
    final originalProject = (
      widget.projectPath,
      widget.isRemote,
      widget.remoteHost,
    );
    String? error;
    try {
      switch (target) {
        case ChatWebLink(:final uri):
          if (!await widget.openLink(uri)) error = 'Could not open this link.';
        case ChatFileLink():
          await widget.revealFile(target);
        case UnsupportedChatLink(:final message):
          error = message;
      }
    } on ChatLinkFailure catch (failure) {
      error = failure.message;
    } on MissingPluginException {
      error = target is ChatFileLink
          ? 'Finder integration is unavailable.'
          : 'Could not open this link.';
    } catch (_) {
      error = target is ChatFileLink
          ? 'Could not show this file in Finder.'
          : 'Could not open this link.';
    }
    if (mounted &&
        widget.text == originalText &&
        (widget.projectPath, widget.isRemote, widget.remoteHost) ==
            originalProject) {
      setState(() => _linkError = error);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    if (_theme != theme) {
      _theme = theme;
      _body = null;
    }
    final tokens = context.ditch;
    final bodyStyle = theme.textTheme.bodyMedium?.copyWith(
      color: tokens.ink,
      height: 1.45,
      fontSize: 14,
    );
    // Retain the Markdown widget and stylesheet on unrelated transcript rebuilds.
    // MarkdownBody also retains its parsed children when data/styles are equal.
    _body ??= MarkdownBody(
      // MarkdownBody reparses only on data/style changes. Context also changes
      // relative destinations and their tooltips, even with identical Markdown.
      key: ValueKey((widget.projectPath, widget.isRemote, widget.remoteHost)),
      data: widget.text,
      fitContent: false,
      extensionSet: md.ExtensionSet.gitHubFlavored,
      blockSyntaxes: const [_ClosedFenceSyntax(), _LiteralHtmlSyntax()],
      styleSheet: MarkdownStyleSheet.fromTheme(theme).copyWith(
        p: bodyStyle,
        a: bodyStyle?.copyWith(
          color: tokens.running,
          decoration: TextDecoration.underline,
        ),
        code: bodyStyle?.copyWith(
          fontFamily: 'SF Mono',
          fontSize: 12.5,
          // Span backgrounds paint over selection highlights. Explicitly clear
          // the Markdown default so inline code remains visibly selectable.
          backgroundColor: Colors.transparent,
        ),
        blockquote: bodyStyle,
        blockquoteDecoration: BoxDecoration(
          color: tokens.surfaceSoft,
          border: Border(left: BorderSide(color: tokens.accent, width: 3)),
        ),
        blockquotePadding: const EdgeInsets.all(12),
        codeblockDecoration: BoxDecoration(
          color: tokens.surfaceSoft,
          border: Border.all(color: tokens.line),
          borderRadius: BorderRadius.circular(8),
        ),
        tableHead: bodyStyle?.copyWith(fontWeight: FontWeight.w700),
        tableHeadCellsDecoration: BoxDecoration(color: tokens.surfaceSoft),
        tableBody: bodyStyle,
        tableBorder: TableBorder.all(color: tokens.line),
        tableCellsPadding: const EdgeInsets.symmetric(
          horizontal: 12,
          vertical: 8,
        ),
        tableColumnWidth: const IntrinsicColumnWidth(),
        tableHeadAlign: TextAlign.left,
      ),
      builders: {
        'pre': _CodeBuilder(),
        'a': _LinkBuilder(_open, _tooltip, widget.fetchImage),
      },
      checkboxBuilder: (checked) => Semantics(
        checked: checked,
        label: checked ? 'Completed task' : 'Incomplete task',
        child: ExcludeSemantics(
          child: Icon(
            checked ? Icons.check_box : Icons.check_box_outline_blank,
            size: 18,
            color: tokens.accent,
          ),
        ),
      ),
      imageBuilder: (uri, title, alt) => AssistantMessageImage(
        destination: uri.toString(),
        alt: alt ?? title ?? '',
        fetcher: widget.fetchImage,
      ),
    );
    return SelectionArea(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          _body!,
          if (_linkError != null)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Semantics(
                liveRegion: true,
                child: Text(_linkError!, style: TextStyle(color: tokens.error)),
              ),
            ),
        ],
      ),
    );
  }
}

class _LinkBuilder extends MarkdownElementBuilder {
  _LinkBuilder(this.open, this.tooltip, this.fetchImage);
  final Future<void> Function(String) open;
  final String Function(String) tooltip;
  final ChatImageFetcher fetchImage;

  InlineSpan _span(md.Node node) {
    if (node is! md.Element) return TextSpan(text: node.textContent);
    if (node.tag == 'img') {
      return WidgetSpan(
        child: AssistantMessageImage(
          destination: node.attributes['src'] ?? '',
          alt: node.attributes['alt'] ?? '',
          fetcher: fetchImage,
        ),
      );
    }
    final style = switch (node.tag) {
      'strong' => const TextStyle(fontWeight: FontWeight.bold),
      'em' => const TextStyle(fontStyle: FontStyle.italic),
      'del' => const TextStyle(decoration: TextDecoration.lineThrough),
      'code' => const TextStyle(fontFamily: 'SF Mono'),
      _ => null,
    };
    return TextSpan(
      style: style,
      text: node.tag == 'br' ? '\n' : null,
      children: node.children?.map(_span).toList(),
    );
  }

  @override
  Widget visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) {
    final destination = element.attributes['href'] ?? '';
    return Tooltip(
      message: tooltip(destination),
      child: Semantics(
        link: true,
        child: InkWell(
          onTap: () => open(destination),
          child: Text.rich(
            TextSpan(children: element.children?.map(_span).toList()),
            style: preferredStyle,
          ),
        ),
      ),
    );
  }
}

/// Keep raw HTML visible without passing it through any HTML renderer.
class _LiteralHtmlSyntax extends md.HtmlBlockSyntax {
  const _LiteralHtmlSyntax();

  @override
  md.Node parse(md.BlockParser parser) =>
      md.Element('p', [super.parse(parser)]);
}

/// Let the standard parser handle escaping/indentation, but mark unclosed
/// fences so streaming JSON does not prematurely turn into a visual.
class _ClosedFenceSyntax extends md.FencedCodeBlockSyntax {
  const _ClosedFenceSyntax();

  @override
  md.Node parse(md.BlockParser parser) {
    final opening = pattern.firstMatch(parser.current.content)!;
    final marker =
        opening.namedGroup('backtick') ?? opening.namedGroup('tilde')!;
    var closed = false;
    for (var i = 1; parser.peek(i) != null; i++) {
      final line = parser.peek(i)!.content.trimLeft();
      if (line.startsWith(marker) &&
          line.replaceAll(marker[0], '').trim().isEmpty) {
        closed = true;
        break;
      }
    }
    final element = super.parse(parser) as md.Element;
    element.attributes['data-closed'] = '$closed';
    return element;
  }
}

class _CodeBuilder extends MarkdownElementBuilder {
  @override
  Widget visitElementAfterWithContext(
    BuildContext context,
    md.Element element,
    TextStyle? preferredStyle,
    TextStyle? parentStyle,
  ) {
    final code = element.children?.whereType<md.Element>().firstOrNull;
    final language = (code?.attributes['class'] ?? '').replaceFirst(
      'language-',
      '',
    );
    final source = element.textContent;
    final visual = element.attributes['data-closed'] == 'true'
        ? ChatVisualData.parse(language, source)
        : null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 12, right: 4),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  language.isEmpty ? 'Code' : language,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.labelSmall,
                ),
              ),
              IconButton(
                tooltip: language == 'mermaid' ? 'Copy as Markdown' : 'Copy code',
                visualDensity: VisualDensity.compact,
                icon: const Icon(Icons.copy_outlined, size: 16),
                onPressed: () => Clipboard.setData(ClipboardData(
                  text: language == 'mermaid' ? mermaidMarkdown(source) : source,
                )),
              ),
            ],
          ),
        ),
        if (language == 'mermaid' && element.attributes['data-closed'] == 'true')
          ChatMermaidBlock(source: source)
        else if (visual != null)
          ChatVisualBlock(data: visual)
        else
          ChatCodeSource(source: source),
      ],
    );
  }
}
