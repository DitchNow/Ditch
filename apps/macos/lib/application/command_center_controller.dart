import 'package:flutter/foundation.dart';

enum RuntimeConnectionPhase { connecting, connected, reconnecting, unavailable }

@immutable
class CommandCenterPresentationState {
  const CommandCenterPresentationState({
    this.connection = RuntimeConnectionPhase.connecting,
    this.connectionError,
    this.inspectorVisible = true,
    this.sidebarVisible = true,
  });

  final RuntimeConnectionPhase connection;
  final String? connectionError;
  final bool inspectorVisible;
  final bool sidebarVisible;

  CommandCenterPresentationState copyWith({
    RuntimeConnectionPhase? connection,
    String? connectionError,
    bool clearConnectionError = false,
    bool? inspectorVisible,
    bool? sidebarVisible,
  }) {
    return CommandCenterPresentationState(
      connection: connection ?? this.connection,
      connectionError: clearConnectionError
          ? null
          : connectionError ?? this.connectionError,
      inspectorVisible: inspectorVisible ?? this.inspectorVisible,
      sidebarVisible: sidebarVisible ?? this.sidebarVisible,
    );
  }
}

/// Observable UI presentation state. Runtime/session data remains authoritative
/// in ditchd and is applied separately from window-only concerns.
class CommandCenterController
    extends ValueNotifier<CommandCenterPresentationState> {
  CommandCenterController() : super(const CommandCenterPresentationState());

  void connecting({bool reconnecting = false}) {
    value = value.copyWith(
      connection: reconnecting
          ? RuntimeConnectionPhase.reconnecting
          : RuntimeConnectionPhase.connecting,
      clearConnectionError: true,
    );
  }

  void connected() {
    value = value.copyWith(
      connection: RuntimeConnectionPhase.connected,
      clearConnectionError: true,
    );
  }

  void unavailable(Object error) {
    value = value.copyWith(
      connection: RuntimeConnectionPhase.unavailable,
      connectionError: error.toString(),
    );
  }

  void toggleSidebar() {
    value = value.copyWith(sidebarVisible: !value.sidebarVisible);
  }

  void toggleInspector() {
    value = value.copyWith(inspectorVisible: !value.inspectorVisible);
  }
}
