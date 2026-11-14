import 'package:flutter/material.dart';
import 'package:the_ditch/design_system/ditch_theme.dart';
import 'package:the_ditch/widgets/assistant_message_content.dart';

/// Run with flutter run -d macos -t example/chat_formatting_preview.dart.
void main() => runApp(const ChatFormattingPreview());

class ChatFormattingPreview extends StatefulWidget {
  const ChatFormattingPreview({super.key});

  @override
  State<ChatFormattingPreview> createState() => _ChatFormattingPreviewState();
}

class _ChatFormattingPreviewState extends State<ChatFormattingPreview> {
  bool _dark = true;
  bool _compact = false;

  @override
  Widget build(BuildContext context) => MaterialApp(
    theme: DitchTheme.light(),
    darkTheme: DitchTheme.dark(),
    themeMode: _dark ? ThemeMode.dark : ThemeMode.light,
    home: Builder(
      builder: (context) => Scaffold(
        appBar: AppBar(
          title: const Text('AI answer formatting'),
          actions: [
            IconButton(
              tooltip: _compact
                  ? 'Enlarge conversation'
                  : 'Compact conversation',
              onPressed: () => setState(() => _compact = !_compact),
              icon: Icon(
                _compact ? Icons.open_in_full : Icons.close_fullscreen,
              ),
            ),
            IconButton(
              tooltip: _dark ? 'Use light theme' : 'Use dark theme',
              onPressed: () => setState(() => _dark = !_dark),
              icon: Icon(_dark ? Icons.light_mode : Icons.dark_mode),
            ),
          ],
        ),
        body: SingleChildScrollView(
          padding: const EdgeInsets.all(24),
          child: Center(
            child: Container(
              width: _compact ? 340 : 720,
              padding: const EdgeInsets.all(20),
              decoration: BoxDecoration(
                color: context.ditch.surface,
                border: Border.all(color: context.ditch.line),
                borderRadius: BorderRadius.circular(16),
              ),
              child: const AssistantMessageContent(text: chatFormattingSample),
            ),
          ),
        ),
      ),
    ),
  );
}

const chatFormattingSample = '''
## A clearer picture of your project

Here is the **delivery flow**, followed by the weekly results. Diagrams and
charts use the same colors as the rest of Ditch.

```mermaid
flowchart LR
  A[Plan] --> B[Build]
  B --> C{Review}
  C -->|Approved| D[Ship]
  C -->|Changes| B
```

### Weekly progress

```ditch-chart
{"title":"Completed tasks","type":"bar","labels":["Mon","Tue","Wed","Thu","Fri"],"series":[{"name":"This week","values":[12,18,15,24,28]},{"name":"Last week","values":[9,14,13,16,20]}]}
```

```ditch-chart
{"title":"Response time (ms)","type":"line","labels":["Mon","Tue","Wed","Thu","Fri"],"series":[{"name":"Median","values":[220,180,195,145,120]},{"name":"95th percentile","values":[360,310,295,240,210]}]}
```

### How a request travels

```mermaid
sequenceDiagram
  participant User
  participant Agent
  participant Tools
  User->>Agent: Explain this project
  Agent->>Tools: Read project context
  Tools-->>Agent: Relevant files
  Agent-->>User: Formatted answer
```

### Work distribution

```mermaid
pie title Completed work
  "Features" : 45
  "Fixes" : 35
  "Documentation" : 20
```

| Result | Status |
| --- | --- |
| Diagrams | **Ready for review** |
| Charts | Labels, colors, hover values |
| Source | Selectable and copyable |

```ditch-palette
{"title":"Project palette","colors":[{"label":"Orange","hex":"#F47A12"},{"label":"Blue","hex":"#3155A6"},{"label":"Green","hex":"#18794E"}]}
```

> Expand a diagram to pan and zoom, or show its source to inspect the Mermaid.
''';
