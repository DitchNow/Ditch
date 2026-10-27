import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../design_system/ditch_theme.dart';

/// Deliberately small, declarative formats. Invalid input stays ordinary code.
class ChatVisualData {
  const ChatVisualData._(
    this.kind,
    this.title,
    this.labels,
    this.series,
    this.colors,
  );

  final String kind;
  final String title;
  final List<String> labels;
  final List<({String name, List<double> values})> series;
  final List<String> colors;

  static const maxBlockLength = 16384;
  static const maxPoints = 48;
  static const maxSeries = 4;
  static const maxColors = 16;

  static ChatVisualData? parse(String language, String source) {
    if (source.length > maxBlockLength ||
        (language != 'ditch-chart' && language != 'ditch-palette')) {
      return null;
    }
    bool label(Object? value) =>
        value is String && value.trim().isNotEmpty && value.length <= 80;
    try {
      final json = jsonDecode(source);
      if (json is! Map<String, dynamic> || !label(json['title'])) return null;
      if (language == 'ditch-palette') {
        final entries = json['colors'];
        if (entries is! List || entries.isEmpty || entries.length > maxColors) {
          return null;
        }
        final labels = <String>[];
        final colors = <String>[];
        for (final entry in entries) {
          if (entry is! Map ||
              !label(entry['label']) ||
              entry['hex'] is! String ||
              !RegExp(r'^#[0-9a-fA-F]{6}$').hasMatch(entry['hex'] as String)) {
            return null;
          }
          labels.add(entry['label'] as String);
          colors.add((entry['hex'] as String).toUpperCase());
        }
        return ChatVisualData._(
          'palette',
          json['title'],
          labels,
          const [],
          colors,
        );
      }
      if (json['type'] != 'bar' && json['type'] != 'line') return null;
      final labels = json['labels'];
      final series = json['series'];
      if (labels is! List ||
          labels.isEmpty ||
          labels.length > maxPoints ||
          !labels.every(label) ||
          series is! List ||
          series.isEmpty ||
          series.length > maxSeries) {
        return null;
      }
      final parsed = <({String name, List<double> values})>[];
      for (final item in series) {
        if (item is! Map || !label(item['name'])) return null;
        final values = item['values'];
        if (values is! List ||
            values.length != labels.length ||
            !values.every((v) => v is num && v.isFinite && v.abs() <= 1e12)) {
          return null;
        }
        parsed.add((
          name: item['name'] as String,
          values: values.map((v) => (v as num).toDouble()).toList(),
        ));
      }
      return ChatVisualData._(
        json['type'],
        json['title'],
        labels.cast<String>(),
        parsed,
        const [],
      );
    } on FormatException {
      return null;
    }
  }
}

class ChatVisualBlock extends StatelessWidget {
  const ChatVisualBlock({required this.data, super.key});
  final ChatVisualData data;

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;
    final seriesColors = [
      tokens.running,
      tokens.accentDark,
      tokens.success,
      tokens.error,
    ];
    return Padding(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(data.title, style: Theme.of(context).textTheme.titleSmall),
          const SizedBox(height: 8),
          if (data.kind == 'palette')
            Wrap(
              spacing: 16,
              runSpacing: 12,
              children: [
                for (var i = 0; i < data.colors.length; i++)
                  Semantics(
                    label: '${data.labels[i]}, ${data.colors[i]}',
                    excludeSemantics: true,
                    child: SizedBox(
                      width: 140,
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Container(
                            height: 40,
                            decoration: BoxDecoration(
                              color: Color(
                                int.parse(
                                  'FF${data.colors[i].substring(1)}',
                                  radix: 16,
                                ),
                              ),
                              border: Border.all(color: tokens.line),
                              borderRadius: BorderRadius.circular(6),
                            ),
                          ),
                          Text(data.labels[i]),
                          Text(
                            data.colors[i],
                            style: const TextStyle(fontFamily: 'SF Mono'),
                          ),
                        ],
                      ),
                    ),
                  ),
              ],
            )
          else ...[
            // Values remain accessible without interpreting the plot or colors.
            SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              primary: false,
              child: SizedBox(
                width: math.max(300, data.labels.length * 80).toDouble(),
                child: Column(
                  children: [
                    ExcludeSemantics(
                      child: SizedBox(
                        height: 180,
                        width: double.infinity,
                        child: CustomPaint(
                          painter: _ChartPainter(
                            data,
                            seriesColors,
                            tokens.line,
                          ),
                        ),
                      ),
                    ),
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        for (var i = 0; i < data.labels.length; i++)
                          Expanded(
                            child: Padding(
                              padding: const EdgeInsets.all(4),
                              child: Text(
                                data.labels[i],
                                textAlign: TextAlign.center,
                              ),
                            ),
                          ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
            for (var s = 0; s < data.series.length; s++) ...[
              const SizedBox(height: 8),
              Text(
                data.series[s].name,
                style: TextStyle(
                  color: seriesColors[s],
                  fontWeight: FontWeight.w700,
                ),
              ),
              Text(
                [
                  for (var i = 0; i < data.labels.length; i++)
                    '${data.labels[i]}: ${data.series[s].values[i]}',
                ].join('; '),
              ),
            ],
          ],
        ],
      ),
    );
  }
}

class _ChartPainter extends CustomPainter {
  _ChartPainter(this.data, this.colors, this.gridColor);
  final ChatVisualData data;
  final List<Color> colors;
  final Color gridColor;

  @override
  void paint(Canvas canvas, Size size) {
    final values = data.series.expand((s) => s.values);
    final min = math.min(0.0, values.reduce(math.min));
    final max = math.max(0.0, values.reduce(math.max));
    final range = max == min ? 1.0 : max - min;
    double y(double value) => 8 + (max - value) / range * (size.height - 16);
    final step = size.width / data.labels.length;
    canvas.drawLine(
      Offset(0, y(0)),
      Offset(size.width, y(0)),
      Paint()..color = gridColor,
    );
    for (var s = 0; s < data.series.length; s++) {
      final paint = Paint()
        ..color = colors[s]
        ..strokeWidth = 2;
      final path = Path();
      for (var i = 0; i < data.labels.length; i++) {
        final value = data.series[s].values[i];
        final x = (i + 0.5) * step;
        if (data.kind == 'bar') {
          final width = step * 0.7 / data.series.length;
          final left = i * step + step * 0.15 + s * width;
          canvas.drawRect(
            Rect.fromLTRB(
              left,
              math.min(y(0), y(value)),
              left + width - 2,
              math.max(y(0), y(value)),
            ),
            paint,
          );
        } else {
          if (i == 0) {
            path.moveTo(x, y(value));
          } else {
            path.lineTo(x, y(value));
          }
          canvas.drawCircle(Offset(x, y(value)), 3, paint);
        }
      }
      if (data.kind == 'line') {
        canvas.drawPath(path, paint..style = PaintingStyle.stroke);
      }
    }
  }

  @override
  bool shouldRepaint(_ChartPainter oldDelegate) =>
      data != oldDelegate.data ||
      colors != oldDelegate.colors ||
      gridColor != oldDelegate.gridColor;
}
