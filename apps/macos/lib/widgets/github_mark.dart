// GitHub's Octicons mark-github-16, converted to a Flutter path.
// https://github.com/primer/octicons/blob/main/icons/mark-github-16.svg
// MIT License
//
// Copyright (c) 2026 GitHub Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

import 'package:flutter/material.dart';

class GitHubMark extends StatelessWidget {
  const GitHubMark({this.size = 18, super.key});

  final double size;

  @override
  Widget build(BuildContext context) {
    final theme = IconTheme.of(context);
    final color = theme.color ?? Theme.of(context).colorScheme.onSurface;
    return CustomPaint(
      size: Size.square(size),
      painter: _GitHubMarkPainter(
        color.withValues(alpha: color.a * (theme.opacity ?? 1)),
      ),
    );
  }
}

class _GitHubMarkPainter extends CustomPainter {
  const _GitHubMarkPainter(this.color);
  final Color color;

  static final _path = Path()
    ..moveTo(6.766, 11.328)
    ..relativeCubicTo(-2.063, -0.25, -3.516, -1.734, -3.516, -3.656)
    ..relativeCubicTo(0, -0.781, 0.281, -1.625, 0.75, -2.188)
    ..relativeCubicTo(-0.203, -0.515, -0.172, -1.609, 0.063, -2.062)
    ..relativeCubicTo(0.625, -0.078, 1.468, 0.25, 1.968, 0.703)
    ..relativeCubicTo(0.594, -0.187, 1.219, -0.281, 1.985, -0.281)
    ..relativeCubicTo(0.765, 0, 1.39, 0.094, 1.953, 0.265)
    ..relativeCubicTo(0.484, -0.437, 1.344, -0.765, 1.969, -0.687)
    ..relativeCubicTo(0.218, 0.422, 0.25, 1.515, 0.046, 2.047)
    ..relativeCubicTo(0.5, 0.593, 0.766, 1.39, 0.766, 2.203)
    ..relativeCubicTo(0, 1.922, -1.453, 3.375, -3.547, 3.64)
    ..relativeCubicTo(0.531, 0.344, 0.89, 1.094, 0.89, 1.954)
    ..relativeLineTo(0, 1.625)
    ..relativeCubicTo(0, 0.468, 0.391, 0.734, 0.86, 0.547)
    ..cubicTo(13.781, 14.359, 16, 11.53, 16, 8.03)
    ..cubicTo(16, 3.61, 12.406, 0, 7.984, 0)
    ..cubicTo(3.563, 0, 0, 3.61, 0, 8.031)
    ..relativeArcToPoint(
      Offset(5.172, 7.422),
      radius: Radius.elliptical(7.88, 7.88),
      largeArc: false,
      clockwise: false,
    )
    ..relativeCubicTo(0.422, 0.156, 0.828, -0.125, 0.828, -0.547)
    ..relativeLineTo(0, -1.25)
    ..relativeCubicTo(-0.219, 0.094, -0.5, 0.156, -0.75, 0.156)
    ..relativeCubicTo(-1.031, 0, -1.64, -0.562, -2.078, -1.609)
    ..relativeCubicTo(-0.172, -0.422, -0.36, -0.672, -0.719, -0.719)
    ..relativeCubicTo(-0.187, -0.015, -0.25, -0.093, -0.25, -0.187)
    ..relativeCubicTo(0, -0.188, 0.313, -0.328, 0.625, -0.328)
    ..relativeCubicTo(0.453, 0, 0.844, 0.281, 1.25, 0.86)
    ..relativeCubicTo(0.313, 0.452, 0.64, 0.655, 1.031, 0.655)
    ..relativeCubicTo(0.391, 0, 0.641, -0.14, 1, -0.5)
    ..relativeCubicTo(0.266, -0.265, 0.47, -0.5, 0.657, -0.656);

  @override
  void paint(Canvas canvas, Size size) {
    canvas.save();
    canvas.scale(size.width / 16, size.height / 16);
    canvas.drawPath(_path, Paint()..color = color);
    canvas.restore();
  }

  @override
  bool shouldRepaint(_GitHubMarkPainter oldDelegate) =>
      color != oldDelegate.color;
}
