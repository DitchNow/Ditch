import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:the_ditch/widgets/chat_visual_block.dart';

Map<String, Object> chart() => {
  'title': 'Trend',
  'type': 'bar',
  'labels': ['A', 'B'],
  'series': [
    {
      'name': 'Visitors',
      'values': [-2, 0],
    },
  ],
};

void main() {
  test(
    'validates chart schema, bounded counts, numeric values, and block size',
    () {
      for (final type in ['bar', 'line']) {
        expect(
          ChatVisualData.parse(
            'ditch-chart',
            jsonEncode(chart()..['type'] = type),
          ),
          isNotNull,
        );
      }
      for (final data in [
        chart()..['type'] = 'pie',
        chart()..['title'] = '',
        chart()..['labels'] = List.filled(49, 'x'),
        chart()
          ..['series'] = List.filled(5, {
            'name': 'n',
            'values': [1, 2],
          }),
        chart()
          ..['series'] = [
            {
              'name': 'n',
              'values': [1],
            },
          ],
        chart()
          ..['series'] = [
            {
              'name': 'n',
              'values': ['1', 2],
            },
          ],
        chart()
          ..['series'] = [
            {
              'name': 'n',
              'values': [1e13, 2],
            },
          ],
        chart()..['series'] = [],
        chart()..['labels'] = [null],
        chart()..['series'] = [null],
        chart()..['labels'] = ['x' * 81, 'B'],
      ]) {
        expect(ChatVisualData.parse('ditch-chart', jsonEncode(data)), isNull);
      }
      for (final text in [
        '{',
        'null',
        '[]',
        '${' ' * 16385}{}',
        '{"title":"t","type":"line","labels":["A"],"series":[{"name":"s","values":[1e999]}]}',
      ]) {
        expect(ChatVisualData.parse('ditch-chart', text), isNull);
      }
      expect(ChatVisualData.parse('html', jsonEncode(chart())), isNull);
    },
  );

  test('palette validates hex, labels and color count', () {
    Map<String, Object> palette(Object colors) => {
      'title': 'Colors',
      'colors': colors,
    };
    expect(
      ChatVisualData.parse(
        'ditch-palette',
        jsonEncode(
          palette([
            {'label': 'Blue', 'hex': '#abcdef'},
          ]),
        ),
      )!.colors,
      ['#ABCDEF'],
    );
    for (final colors in [
      [],
      List.filled(17, {'label': 'X', 'hex': '#FFFFFF'}),
      [
        {'label': '', 'hex': '#123456'},
      ],
      [
        {'label': 'X', 'hex': 'red'},
      ],
      [
        {'label': 'X', 'hex': '#123'},
      ],
      [
        {'label': 'X', 'hex': '#12345678'},
      ],
      [null],
    ]) {
      expect(
        ChatVisualData.parse('ditch-palette', jsonEncode(palette(colors))),
        isNull,
      );
    }
  });

  testWidgets('charts and palettes expose text and accessibility labels', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final palette = ChatVisualData.parse(
      'ditch-palette',
      '{"title":"Brand","colors":[{"label":"Ocean","hex":"#123456"}]}',
    )!;
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: SingleChildScrollView(
            child: SizedBox(
              width: 240,
              child: Column(
                children: [
                  ChatVisualBlock(data: palette),
                  ChatVisualBlock(
                    data: ChatVisualData.parse(
                      'ditch-chart',
                      jsonEncode(chart()),
                    )!,
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
    expect(find.bySemanticsLabel('Ocean, #123456'), findsOneWidget);
    expect(find.text('Ocean'), findsOneWidget);
    expect(find.text('#123456'), findsOneWidget);
    expect(find.text('A: -2.0; B: 0.0'), findsOneWidget);
    expect(tester.takeException(), isNull);
    semantics.dispose();
  });
}
