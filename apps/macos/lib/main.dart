import 'package:flutter/material.dart';
import 'package:the_ditch/main.dart' as community;

import 'commercial_remote_surface.dart';

export 'package:the_ditch/main.dart';
export 'commercial_remote_surface.dart';

void main() {
  runApp(
    const community.TheDitchApp(editionSurface: CommercialEditionSurface()),
  );
}
