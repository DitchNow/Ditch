import 'dart:convert';
import 'dart:math' as math;

import 'package:fl_chart/fl_chart.dart';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';

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
            LayoutBuilder(
              builder: (context, constraints) => SingleChildScrollView(
                scrollDirection: Axis.horizontal,
                primary: false,
                child: SizedBox(
                  width: math
                      .max(
                        constraints.maxWidth.isFinite
                            ? constraints.maxWidth
                            : 300,
                        math.max(300, data.labels.length * 80),
                      )
                      .toDouble(),
                  height: 240,
                  child: ExcludeSemantics(
                    child: _NativeChart(data: data, colors: seriesColors),
                  ),
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

/// The same bounded data contract rendered by fl_chart, with Ditch colors.
class _NativeChart extends StatelessWidget {
  const _NativeChart({required this.data, required this.colors});

  final ChatVisualData data;
  final List<Color> colors;

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;
    final originalValues = data.series.expand((series) => series.values);
    final magnitude = originalValues
        .map((value) => value.abs())
        .reduce(math.max);
    // Avoid a zero grid interval when a finite subnormal value is divided by
    // the tick count. Labels and tooltips still show the original units.
    final scale = magnitude > 0 && magnitude < 1e-6 ? magnitude : 1.0;
    final values = originalValues.map((value) => value / scale);
    final low = math.min(0.0, values.reduce(math.min));
    final high = math.max(0.0, values.reduce(math.max));
    final range = high == low ? 1.0 : high - low;
    final minY = low < 0 ? low - range * 0.1 : 0.0;
    final maxY = high > 0 ? high + range * 0.1 : (low == 0 ? 1.0 : 0.0);
    final labelStyle = TextStyle(color: tokens.muted, fontSize: 11);
    final tooltipStyle = TextStyle(color: tokens.ink, fontSize: 12);
    final titles = FlTitlesData(
      topTitles: const AxisTitles(),
      rightTitles: const AxisTitles(),
      leftTitles: AxisTitles(
        sideTitles: SideTitles(
          showTitles: true,
          reservedSize: 44,
          minIncluded: false,
          maxIncluded: false,
          getTitlesWidget: (value, meta) {
            final actual = value * scale;
            return Text(
              actual != 0 && actual.abs() < 0.001
                  ? actual.toStringAsExponential(0)
                  : NumberFormat.compact().format(actual),
              style: labelStyle,
            );
          },
        ),
      ),
      bottomTitles: AxisTitles(
        sideTitles: SideTitles(
          showTitles: true,
          reservedSize: 52,
          interval: 1,
          minIncluded: false,
          maxIncluded: false,
          getTitlesWidget: (value, meta) {
            final index = value.round();
            if (value != index || index < 0 || index >= data.labels.length) {
              return const SizedBox.shrink();
            }
            return Padding(
              padding: const EdgeInsets.only(top: 8),
              child: SizedBox(
                width: 68,
                child: Text(
                  data.labels[index],
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.center,
                  style: labelStyle,
                ),
              ),
            );
          },
        ),
      ),
    );
    final grid = FlGridData(
      drawVerticalLine: false,
      getDrawingHorizontalLine: (_) =>
          FlLine(color: tokens.line, strokeWidth: 1, dashArray: [4, 4]),
    );
    final border = FlBorderData(show: false);
    if (data.kind == 'bar') {
      return BarChart(
        BarChartData(
          minY: minY,
          maxY: maxY,
          titlesData: titles,
          gridData: grid,
          borderData: border,
          barTouchData: BarTouchData(
            touchTooltipData: BarTouchTooltipData(
              getTooltipColor: (_) => tokens.surface,
              fitInsideHorizontally: true,
              fitInsideVertically: true,
              getTooltipItem: (group, groupIndex, rod, rodIndex) => BarTooltipItem(
                '${data.series[rodIndex].name}\n'
                '${data.labels[groupIndex]}: ${data.series[rodIndex].values[groupIndex]}',
                tooltipStyle,
              ),
            ),
          ),
          barGroups: [
            for (var i = 0; i < data.labels.length; i++)
              BarChartGroupData(
                x: i,
                barsSpace: 3,
                barRods: [
                  for (var s = 0; s < data.series.length; s++)
                    BarChartRodData(
                      toY: data.series[s].values[i] / scale,
                      color: colors[s],
                      width: 40 / data.series.length,
                      borderRadius: BorderRadius.circular(3),
                    ),
                ],
              ),
          ],
        ),
        duration: Duration.zero,
      );
    }
    return LineChart(
      LineChartData(
        minX: -0.5,
        maxX: data.labels.length - 0.5,
        minY: minY,
        maxY: maxY,
        titlesData: titles,
        gridData: grid,
        borderData: border,
        lineTouchData: LineTouchData(
          touchTooltipData: LineTouchTooltipData(
            getTooltipColor: (_) => tokens.surface,
            fitInsideHorizontally: true,
            fitInsideVertically: true,
            getTooltipItems: (spots) => [
              for (final spot in spots)
                LineTooltipItem(
                  '${data.series[spot.barIndex].name}\n'
                  '${data.labels[spot.x.toInt()]}: ${data.series[spot.barIndex].values[spot.x.toInt()]}',
                  tooltipStyle,
                ),
            ],
          ),
        ),
        lineBarsData: [
          for (var s = 0; s < data.series.length; s++)
            LineChartBarData(
              color: colors[s],
              barWidth: 2.5,
              spots: [
                for (var i = 0; i < data.labels.length; i++)
                  FlSpot(i.toDouble(), data.series[s].values[i] / scale),
              ],
              dotData: FlDotData(
                getDotPainter: (_, _, _, _) => FlDotCirclePainter(
                  radius: 3.5,
                  color: colors[s],
                  strokeWidth: 1.5,
                  strokeColor: tokens.surfaceSoft,
                ),
              ),
            ),
        ],
      ),
      duration: Duration.zero,
    );
  }
}
