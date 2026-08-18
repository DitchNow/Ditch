import 'package:flutter/material.dart';

@immutable
class DitchTokens extends ThemeExtension<DitchTokens> {
  const DitchTokens({
    required this.sidebar,
    required this.workspace,
    required this.inspector,
    required this.surface,
    required this.surfaceHover,
    required this.selection,
    required this.separator,
    required this.mutedText,
    required this.radiusSmall,
    required this.radiusMedium,
    required this.toolbarHeight,
  });

  final Color sidebar;
  final Color workspace;
  final Color inspector;
  final Color surface;
  final Color surfaceHover;
  final Color selection;
  final Color separator;
  final Color mutedText;
  final double radiusSmall;
  final double radiusMedium;
  final double toolbarHeight;

  static const light = DitchTokens(
    sidebar: Color(0xfff4f4f5),
    workspace: Color(0xffffffff),
    inspector: Color(0xfffafafa),
    surface: Color(0xffffffff),
    surfaceHover: Color(0xfff5f5f7),
    selection: Color(0xffdcecff),
    separator: Color(0x1f000000),
    mutedText: Color(0xff6e6e73),
    radiusSmall: 7,
    radiusMedium: 11,
    toolbarHeight: 52,
  );

  static const dark = DitchTokens(
    sidebar: Color(0xff202022),
    workspace: Color(0xff151516),
    inspector: Color(0xff1b1b1d),
    surface: Color(0xff242426),
    surfaceHover: Color(0xff2c2c2e),
    selection: Color(0xff183f69),
    separator: Color(0x33ffffff),
    mutedText: Color(0xffa1a1a6),
    radiusSmall: 7,
    radiusMedium: 11,
    toolbarHeight: 52,
  );

  @override
  DitchTokens copyWith({
    Color? sidebar,
    Color? workspace,
    Color? inspector,
    Color? surface,
    Color? surfaceHover,
    Color? selection,
    Color? separator,
    Color? mutedText,
    double? radiusSmall,
    double? radiusMedium,
    double? toolbarHeight,
  }) {
    return DitchTokens(
      sidebar: sidebar ?? this.sidebar,
      workspace: workspace ?? this.workspace,
      inspector: inspector ?? this.inspector,
      surface: surface ?? this.surface,
      surfaceHover: surfaceHover ?? this.surfaceHover,
      selection: selection ?? this.selection,
      separator: separator ?? this.separator,
      mutedText: mutedText ?? this.mutedText,
      radiusSmall: radiusSmall ?? this.radiusSmall,
      radiusMedium: radiusMedium ?? this.radiusMedium,
      toolbarHeight: toolbarHeight ?? this.toolbarHeight,
    );
  }

  @override
  DitchTokens lerp(ThemeExtension<DitchTokens>? other, double t) {
    if (other is! DitchTokens) return this;
    return DitchTokens(
      sidebar: Color.lerp(sidebar, other.sidebar, t)!,
      workspace: Color.lerp(workspace, other.workspace, t)!,
      inspector: Color.lerp(inspector, other.inspector, t)!,
      surface: Color.lerp(surface, other.surface, t)!,
      surfaceHover: Color.lerp(surfaceHover, other.surfaceHover, t)!,
      selection: Color.lerp(selection, other.selection, t)!,
      separator: Color.lerp(separator, other.separator, t)!,
      mutedText: Color.lerp(mutedText, other.mutedText, t)!,
      radiusSmall: radiusSmall + (other.radiusSmall - radiusSmall) * t,
      radiusMedium: radiusMedium + (other.radiusMedium - radiusMedium) * t,
      toolbarHeight: toolbarHeight + (other.toolbarHeight - toolbarHeight) * t,
    );
  }
}

class DitchTheme {
  const DitchTheme._();

  static ThemeData light() => _theme(Brightness.light, DitchTokens.light);
  static ThemeData dark() => _theme(Brightness.dark, DitchTokens.dark);

  static ThemeData _theme(Brightness brightness, DitchTokens tokens) {
    final dark = brightness == Brightness.dark;
    final scheme = ColorScheme(
      brightness: brightness,
      primary: dark ? const Color(0xff4da3ff) : const Color(0xff007aff),
      onPrimary: Colors.white,
      secondary: dark ? const Color(0xff64d2ff) : const Color(0xff007aff),
      onSecondary: Colors.white,
      error: dark ? const Color(0xffff6961) : const Color(0xffd70015),
      onError: Colors.white,
      surface: tokens.workspace,
      onSurface: dark ? const Color(0xfff5f5f7) : const Color(0xff1d1d1f),
    );
    final base = ThemeData(
      brightness: brightness,
      colorScheme: scheme,
      useMaterial3: true,
      visualDensity: VisualDensity.compact,
      scaffoldBackgroundColor: tokens.workspace,
      fontFamily: '.AppleSystemUIFont',
      splashFactory: NoSplash.splashFactory,
      extensions: [tokens],
    );
    return base.copyWith(
      dividerColor: tokens.separator,
      textTheme: base.textTheme.copyWith(
        headlineSmall: base.textTheme.headlineSmall?.copyWith(
          fontSize: 20,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.25,
        ),
        titleLarge: base.textTheme.titleLarge?.copyWith(
          fontSize: 17,
          fontWeight: FontWeight.w600,
          letterSpacing: -0.15,
        ),
        titleMedium: base.textTheme.titleMedium?.copyWith(
          fontSize: 14,
          fontWeight: FontWeight.w600,
        ),
        bodyMedium: base.textTheme.bodyMedium?.copyWith(fontSize: 13),
        bodySmall: base.textTheme.bodySmall?.copyWith(
          fontSize: 12,
          color: tokens.mutedText,
        ),
        labelLarge: base.textTheme.labelLarge?.copyWith(
          fontSize: 13,
          fontWeight: FontWeight.w600,
        ),
      ),
      iconTheme: IconThemeData(size: 18, color: scheme.onSurface),
      dividerTheme: DividerThemeData(
        color: tokens.separator,
        thickness: 1,
        space: 1,
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          minimumSize: const Size(0, 30),
          padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 7),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(tokens.radiusSmall),
          ),
          textStyle: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          minimumSize: const Size(0, 30),
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
          side: BorderSide(color: tokens.separator),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(tokens.radiusSmall),
          ),
        ),
      ),
      iconButtonTheme: IconButtonThemeData(
        style: IconButton.styleFrom(
          minimumSize: const Size(30, 30),
          maximumSize: const Size(34, 34),
          padding: const EdgeInsets.all(6),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(tokens.radiusSmall),
          ),
        ),
      ),
      dialogTheme: DialogThemeData(
        backgroundColor: tokens.surface,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: tokens.surface,
        isDense: true,
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(tokens.radiusSmall),
          borderSide: BorderSide(color: tokens.separator),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(tokens.radiusSmall),
          borderSide: BorderSide(color: tokens.separator),
        ),
      ),
    );
  }
}

extension DitchThemeContext on BuildContext {
  DitchTokens get ditch =>
      Theme.of(this).extension<DitchTokens>() ??
      (Theme.of(this).brightness == Brightness.dark
          ? DitchTokens.dark
          : DitchTokens.light);
}

class DitchSurface extends StatelessWidget {
  const DitchSurface({
    required this.child,
    this.padding = const EdgeInsets.all(12),
    this.selected = false,
    this.bordered = true,
    super.key,
  });

  final Widget child;
  final EdgeInsetsGeometry padding;
  final bool selected;
  final bool bordered;

  @override
  Widget build(BuildContext context) {
    final tokens = context.ditch;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: selected ? tokens.selection : tokens.surface,
        border: bordered ? Border.all(color: tokens.separator) : null,
        borderRadius: BorderRadius.circular(tokens.radiusMedium),
      ),
      child: Padding(padding: padding, child: child),
    );
  }
}
