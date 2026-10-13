import 'package:flutter/material.dart';

import '../design_system/ditch_theme.dart';

class UpdateAvailableDot extends StatefulWidget {
  const UpdateAvailableDot({super.key});

  @override
  State<UpdateAvailableDot> createState() => _UpdateAvailableDotState();
}

class _UpdateAvailableDotState extends State<UpdateAvailableDot>
    with SingleTickerProviderStateMixin {
  late final _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 900),
    lowerBound: 0.35,
    upperBound: 1,
    value: 1,
  );

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context) || !TickerMode.of(context)) {
      _pulse.stop();
      _pulse.value = 1;
    } else if (!_pulse.isAnimating) {
      _pulse.repeat(reverse: true);
    }
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Tooltip(
    message: 'Update available',
    excludeFromSemantics: true,
    child: Semantics(
      label: 'Update available',
      child: FadeTransition(
        opacity: _pulse,
        child: Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(
            color: context.ditch.accent,
            shape: BoxShape.circle,
          ),
        ),
      ),
    ),
  );
}
