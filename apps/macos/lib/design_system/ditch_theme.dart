import 'package:flutter/material.dart';

@immutable
class DitchTokens extends ThemeExtension<DitchTokens> {
  const DitchTokens({
    required this.background,
    required this.surface,
    required this.surfaceSoft,
    required this.surfaceRaised,
    required this.sidebar,
    required this.controlBackground,
    required this.ink,
    required this.muted,
    required this.strongMuted,
    required this.bodyStrong,
    required this.line,
    required this.accent,
    required this.accentDark,
    required this.accentSoft,
    required this.running,
    required this.runningSoft,
    required this.waiting,
    required this.waitingSoft,
    required this.success,
    required this.successSoft,
    required this.error,
    required this.errorSoft,
    required this.neutral,
    required this.neutralSoft,
    this.radiusCompact = 10,
    this.radiusControl = 12,
    this.radiusCard = 14,
    this.toolbarHeight = 56,
  });

  final Color background;
  final Color surface;
  final Color surfaceSoft;
  final Color surfaceRaised;
  final Color sidebar;
  final Color controlBackground;
  final Color ink;
  final Color muted;
  final Color strongMuted;
  final Color bodyStrong;
  final Color line;
  final Color accent;
  final Color accentDark;
  final Color accentSoft;
  final Color running;
  final Color runningSoft;
  final Color waiting;
  final Color waitingSoft;
  final Color success;
  final Color successSoft;
  final Color error;
  final Color errorSoft;
  final Color neutral;
  final Color neutralSoft;
  final double radiusCompact;
  final double radiusControl;
  final double radiusCard;
  final double toolbarHeight;

  // Compatibility names keep product widgets declarative while their
  // surfaces map onto the canonical brand vocabulary.
  Color get workspace => background;
  Color get inspector => surfaceSoft;
  Color get surfaceHover => surfaceRaised;
  Color get selection => accentSoft;
  Color get separator => line;
  Color get mutedText => muted;
  double get radiusSmall => radiusCompact;
  double get radiusMedium => radiusCard;

  static const light = DitchTokens(
    background: Color(0xfff7f6f2),
    surface: Color(0xffffffff),
    surfaceSoft: Color(0xffefede7),
    surfaceRaised: Color(0xfffaf9f6),
    sidebar: Color(0xfff3f1eb),
    controlBackground: Color(0xffffffff),
    ink: Color(0xff111827),
    muted: Color(0xff626773),
    strongMuted: Color(0xff404653),
    bodyStrong: Color(0xff272d39),
    line: Color(0xffdedbd3),
    accent: Color(0xfff47a12),
    accentDark: Color(0xffd85f08),
    accentSoft: Color(0xfffff0e2),
    running: Color(0xff3155a6),
    runningSoft: Color(0xffe9efff),
    waiting: Color(0xffa15c00),
    waitingSoft: Color(0xfffff0df),
    success: Color(0xff18794e),
    successSoft: Color(0xffe7f5ed),
    error: Color(0xffb83a2d),
    errorSoft: Color(0xffffe9e6),
    neutral: Color(0xff555762),
    neutralSoft: Color(0xffeeedf0),
  );

  static const dark = DitchTokens(
    background: Color(0xff090c12),
    surface: Color(0xff111620),
    surfaceSoft: Color(0xff171d28),
    surfaceRaised: Color(0xff151a25),
    sidebar: Color(0xff0e131c),
    controlBackground: Color(0xff0d121b),
    ink: Color(0xfff6f3ed),
    muted: Color(0xffa6acb8),
    strongMuted: Color(0xffd9dde5),
    bodyStrong: Color(0xffeef0f4),
    line: Color(0xff293140),
    accent: Color(0xffff8a1f),
    accentDark: Color(0xffff9d43),
    accentSoft: Color(0x21ff8a1f),
    running: Color(0xffafc2f4),
    runningSoft: Color(0x297191e1),
    waiting: Color(0xfff1b45b),
    waitingSoft: Color(0x24f1b45b),
    success: Color(0xff6fd0a0),
    successSoft: Color(0x246fd0a0),
    error: Color(0xffff8f82),
    errorSoft: Color(0x24ff8f82),
    neutral: Color(0xffd5d7dc),
    neutralSoft: Color(0x14f4f4f5),
  );

  @override
  DitchTokens copyWith({
    Color? background,
    Color? surface,
    Color? surfaceSoft,
    Color? surfaceRaised,
    Color? sidebar,
    Color? controlBackground,
    Color? ink,
    Color? muted,
    Color? strongMuted,
    Color? bodyStrong,
    Color? line,
    Color? accent,
    Color? accentDark,
    Color? accentSoft,
    Color? running,
    Color? runningSoft,
    Color? waiting,
    Color? waitingSoft,
    Color? success,
    Color? successSoft,
    Color? error,
    Color? errorSoft,
    Color? neutral,
    Color? neutralSoft,
    double? radiusCompact,
    double? radiusControl,
    double? radiusCard,
    double? toolbarHeight,
  }) => DitchTokens(
    background: background ?? this.background,
    surface: surface ?? this.surface,
    surfaceSoft: surfaceSoft ?? this.surfaceSoft,
    surfaceRaised: surfaceRaised ?? this.surfaceRaised,
    sidebar: sidebar ?? this.sidebar,
    controlBackground: controlBackground ?? this.controlBackground,
    ink: ink ?? this.ink,
    muted: muted ?? this.muted,
    strongMuted: strongMuted ?? this.strongMuted,
    bodyStrong: bodyStrong ?? this.bodyStrong,
    line: line ?? this.line,
    accent: accent ?? this.accent,
    accentDark: accentDark ?? this.accentDark,
    accentSoft: accentSoft ?? this.accentSoft,
    running: running ?? this.running,
    runningSoft: runningSoft ?? this.runningSoft,
    waiting: waiting ?? this.waiting,
    waitingSoft: waitingSoft ?? this.waitingSoft,
    success: success ?? this.success,
    successSoft: successSoft ?? this.successSoft,
    error: error ?? this.error,
    errorSoft: errorSoft ?? this.errorSoft,
    neutral: neutral ?? this.neutral,
    neutralSoft: neutralSoft ?? this.neutralSoft,
    radiusCompact: radiusCompact ?? this.radiusCompact,
    radiusControl: radiusControl ?? this.radiusControl,
    radiusCard: radiusCard ?? this.radiusCard,
    toolbarHeight: toolbarHeight ?? this.toolbarHeight,
  );

  @override
  DitchTokens lerp(ThemeExtension<DitchTokens>? other, double t) {
    if (other is! DitchTokens) return this;
    Color mix(Color a, Color b) => Color.lerp(a, b, t)!;
    return DitchTokens(
      background: mix(background, other.background),
      surface: mix(surface, other.surface),
      surfaceSoft: mix(surfaceSoft, other.surfaceSoft),
      surfaceRaised: mix(surfaceRaised, other.surfaceRaised),
      sidebar: mix(sidebar, other.sidebar),
      controlBackground: mix(controlBackground, other.controlBackground),
      ink: mix(ink, other.ink),
      muted: mix(muted, other.muted),
      strongMuted: mix(strongMuted, other.strongMuted),
      bodyStrong: mix(bodyStrong, other.bodyStrong),
      line: mix(line, other.line),
      accent: mix(accent, other.accent),
      accentDark: mix(accentDark, other.accentDark),
      accentSoft: mix(accentSoft, other.accentSoft),
      running: mix(running, other.running),
      runningSoft: mix(runningSoft, other.runningSoft),
      waiting: mix(waiting, other.waiting),
      waitingSoft: mix(waitingSoft, other.waitingSoft),
      success: mix(success, other.success),
      successSoft: mix(successSoft, other.successSoft),
      error: mix(error, other.error),
      errorSoft: mix(errorSoft, other.errorSoft),
      neutral: mix(neutral, other.neutral),
      neutralSoft: mix(neutralSoft, other.neutralSoft),
      radiusCompact: radiusCompact + (other.radiusCompact - radiusCompact) * t,
      radiusControl: radiusControl + (other.radiusControl - radiusControl) * t,
      radiusCard: radiusCard + (other.radiusCard - radiusCard) * t,
      toolbarHeight: toolbarHeight + (other.toolbarHeight - toolbarHeight) * t,
    );
  }
}

class DitchTheme {
  const DitchTheme._();

  static ThemeData light() => _theme(Brightness.light, DitchTokens.light);
  static ThemeData dark() => _theme(Brightness.dark, DitchTokens.dark);

  static ThemeData _theme(Brightness brightness, DitchTokens tokens) {
    final scheme = ColorScheme(
      brightness: brightness,
      primary: tokens.accent,
      onPrimary: const Color(0xff17120d),
      primaryContainer: tokens.accentSoft,
      onPrimaryContainer: tokens.ink,
      secondary: tokens.strongMuted,
      onSecondary: tokens.surface,
      secondaryContainer: tokens.neutralSoft,
      onSecondaryContainer: tokens.ink,
      tertiary: tokens.running,
      onTertiary: tokens.surface,
      tertiaryContainer: tokens.runningSoft,
      onTertiaryContainer: tokens.running,
      error: tokens.error,
      onError: tokens.surface,
      errorContainer: tokens.errorSoft,
      onErrorContainer: tokens.error,
      surface: tokens.surface,
      onSurface: tokens.ink,
      outline: tokens.line,
      outlineVariant: tokens.line,
    );
    final base = ThemeData(
      brightness: brightness,
      colorScheme: scheme,
      useMaterial3: true,
      visualDensity: VisualDensity.compact,
      scaffoldBackgroundColor: tokens.background,
      fontFamily: 'Avenir Next',
      splashFactory: NoSplash.splashFactory,
      extensions: [tokens],
    );
    final display = const TextStyle(
      fontFamily: 'Arial Black',
      fontFamilyFallback: ['Avenir Next', '.AppleSystemUIFont'],
      fontWeight: FontWeight.w900,
    );
    return base.copyWith(
      dividerColor: tokens.line,
      textTheme: base.textTheme.copyWith(
        headlineSmall: display.copyWith(
          color: tokens.ink,
          fontSize: 21,
          height: 1.1,
          letterSpacing: -0.55,
        ),
        titleLarge: display.copyWith(
          color: tokens.ink,
          fontSize: 17,
          height: 1.15,
          letterSpacing: -0.3,
        ),
        titleMedium: base.textTheme.titleMedium?.copyWith(
          color: tokens.bodyStrong,
          fontSize: 14,
          fontWeight: FontWeight.w700,
        ),
        bodyMedium: base.textTheme.bodyMedium?.copyWith(
          color: tokens.bodyStrong,
          fontSize: 14,
          height: 1.45,
        ),
        bodySmall: base.textTheme.bodySmall?.copyWith(
          color: tokens.muted,
          fontSize: 12.5,
          height: 1.4,
        ),
        labelLarge: base.textTheme.labelLarge?.copyWith(
          fontWeight: FontWeight.w800,
          fontSize: 13,
        ),
        labelMedium: base.textTheme.labelMedium?.copyWith(
          fontWeight: FontWeight.w700,
        ),
      ),
      iconTheme: IconThemeData(size: 18, color: tokens.ink),
      dividerTheme: DividerThemeData(
        color: tokens.line,
        thickness: 1,
        space: 1,
      ),
      cardTheme: CardThemeData(
        color: tokens.surface,
        elevation: 0,
        shape: RoundedRectangleBorder(
          side: BorderSide(color: tokens.line),
          borderRadius: BorderRadius.circular(tokens.radiusCard),
        ),
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: ButtonStyle(
          minimumSize: const WidgetStatePropertyAll(Size(0, 40)),
          padding: const WidgetStatePropertyAll(
            EdgeInsets.symmetric(horizontal: 18, vertical: 10),
          ),
          backgroundColor: WidgetStateProperty.resolveWith(
            (states) => states.contains(WidgetState.disabled)
                ? tokens.accent.withValues(alpha: 0.38)
                : states.contains(WidgetState.hovered)
                ? tokens.accentDark
                : tokens.accent,
          ),
          foregroundColor: const WidgetStatePropertyAll(Color(0xff17120d)),
          textStyle: const WidgetStatePropertyAll(
            TextStyle(fontWeight: FontWeight.w800, fontSize: 13),
          ),
          shape: WidgetStatePropertyAll(
            RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(tokens.radiusControl),
            ),
          ),
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          minimumSize: const Size(0, 40),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 9),
          foregroundColor: tokens.ink,
          side: BorderSide(color: tokens.line),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(tokens.radiusControl),
          ),
          textStyle: const TextStyle(fontWeight: FontWeight.w700),
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(
          foregroundColor: tokens.accentDark,
          minimumSize: const Size(0, 40),
          textStyle: const TextStyle(fontWeight: FontWeight.w700),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(tokens.radiusControl),
          ),
        ),
      ),
      iconButtonTheme: IconButtonThemeData(
        style: IconButton.styleFrom(
          foregroundColor: tokens.strongMuted,
          minimumSize: const Size(40, 40),
          maximumSize: const Size(40, 40),
          padding: const EdgeInsets.all(8),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(tokens.radiusCompact),
          ),
        ),
      ),
      dialogTheme: DialogThemeData(
        backgroundColor: tokens.surface,
        shape: RoundedRectangleBorder(
          side: BorderSide(color: tokens.line),
          borderRadius: BorderRadius.circular(tokens.radiusCard),
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: tokens.controlBackground,
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(
          horizontal: 12,
          vertical: 12,
        ),
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(tokens.radiusControl),
          borderSide: BorderSide(color: tokens.line),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(tokens.radiusControl),
          borderSide: BorderSide(color: tokens.line),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(tokens.radiusControl),
          borderSide: BorderSide(color: tokens.accent, width: 1.5),
        ),
      ),
      focusColor: tokens.accent.withValues(alpha: 0.20),
      hoverColor: tokens.accentSoft,
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
    this.padding = const EdgeInsets.all(16),
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
        color: selected ? tokens.accentSoft : tokens.surface,
        border: bordered ? Border.all(color: tokens.line) : null,
        borderRadius: BorderRadius.circular(tokens.radiusCard),
      ),
      child: Padding(padding: padding, child: child),
    );
  }
}
