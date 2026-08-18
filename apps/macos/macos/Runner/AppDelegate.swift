import Cocoa
import FlutterMacOS
import ServiceManagement

/// Native shell for the foreground control surface.
///
/// Runtime supervision deliberately belongs to the independent integrated
/// `ditchd` menu-bar process.
/// Terminating this process must never stop The Ditch Runtime or its agents.
@main
class AppDelegate: FlutterAppDelegate {
  override func applicationDidFinishLaunching(_ notification: Notification) {
    persistRuntimeEnvironment()
    registerStatusHelper()
  }

  /// Login items are launched by launchd and do not inherit the shell that
  /// launched the foreground UI. Persist the environment selected for this app
  /// so the independent runtime uses the same CODEX_HOME after UI restarts and
  /// future logins.
  private func persistRuntimeEnvironment() {
    guard let codexHome = ProcessInfo.processInfo.environment["CODEX_HOME"],
      !codexHome.isEmpty,
      codexHome.hasPrefix("/")
    else { return }
    let support = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch", isDirectory: true)
    do {
      try FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
      try codexHome.write(
        to: support.appendingPathComponent("codex-home"),
        atomically: true,
        encoding: .utf8)
    } catch {
      NSLog("The Ditch could not persist CODEX_HOME: \(error)")
    }
  }

  private func registerStatusHelper() {
    if #available(macOS 13.0, *) {
      // Clean up the superseded three-process architecture. Unregistering the
      // obsolete login item does not send a shutdown request to the runtime.
      let obsolete = SMAppService.loginItem(identifier: "ai.theditch.status")
      if obsolete.status == .enabled {
        try? obsolete.unregister()
      }
      let service = SMAppService.loginItem(identifier: "ai.theditch.runtime")
      guard service.status != .enabled else { return }
      do {
        try service.register()
      } catch {
        NSLog("The Ditch could not register its runtime: \(error)")
      }
    } else {
      _ = SMLoginItemSetEnabled("ai.theditch.runtime" as CFString, true)
    }
  }

  func configureApplicationChannel(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(
      name: "the_ditch/application",
      binaryMessenger: messenger)

    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else {
        result(false)
        return
      }
      switch call.method {
      case "showWindow":
        self.showMainWindow()
        result(true)
      case "runtimeAvailable":
        self.runtimeAvailable { result($0) }
      case "openActivityMonitor":
        let url = URL(fileURLWithPath: "/System/Applications/Utilities/Activity Monitor.app")
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        NSWorkspace.shared.openApplication(at: url, configuration: configuration) { _, error in
          result(error == nil)
        }
      case "quitUI":
        result(true)
        NSApp.terminate(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  func configureProjectPickerChannel(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(
      name: "the_ditch/project_picker",
      binaryMessenger: messenger)

    channel.setMethodCallHandler { call, result in
      guard call.method == "chooseDirectory" else {
        result(FlutterMethodNotImplemented)
        return
      }

      let panel = NSOpenPanel()
      panel.title = "Choose a project folder"
      panel.prompt = "Choose"
      panel.canChooseFiles = false
      panel.canChooseDirectories = true
      panel.allowsMultipleSelection = false
      panel.canCreateDirectories = true
      panel.resolvesAliases = true

      if panel.runModal() == .OK {
        result(panel.url?.standardizedFileURL.path)
      } else {
        result(nil)
      }
    }
  }

  override func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
    return false
  }

  override func applicationShouldHandleReopen(
    _ sender: NSApplication,
    hasVisibleWindows flag: Bool
  ) -> Bool {
    showMainWindow()
    return true
  }

  override func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool {
    return true
  }

  private func runtimeAvailable(completion: @escaping (Bool) -> Void) {
    let cli = bundledExecutable("ditch_cli")
    guard FileManager.default.isExecutableFile(atPath: cli) else {
      completion(false)
      return
    }
    DispatchQueue.global(qos: .utility).async {
      let process = Process()
      process.executableURL = URL(fileURLWithPath: cli)
      process.arguments = ["runtime", "status"]
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      do {
        try process.run()
        process.waitUntilExit()
        if process.terminationStatus == 0 {
          DispatchQueue.main.async { completion(true) }
          return
        }

        guard self.startRuntimeHost() else {
          DispatchQueue.main.async { completion(false) }
          return
        }
        for _ in 0..<30 {
          Thread.sleep(forTimeInterval: 0.1)
          if self.runtimeResponds(cli: cli) {
            DispatchQueue.main.async { completion(true) }
            return
          }
        }
        DispatchQueue.main.async { completion(false) }
      } catch {
        DispatchQueue.main.async { completion(false) }
      }
    }
  }

  private func runtimeResponds(cli: String) -> Bool {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: cli)
    process.arguments = ["runtime", "status"]
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    do {
      try process.run()
      process.waitUntilExit()
      return process.terminationStatus == 0
    } catch {
      return false
    }
  }

  /// Starts the independent integrated runtime process. The process inherits
  /// CODEX_HOME from this UI launch but has no lifecycle dependency on the UI.
  private func startRuntimeHost() -> Bool {
    let executable = Bundle.main.bundleURL
      .appendingPathComponent("Contents/Library/LoginItems/The Ditch Runtime.app/Contents/MacOS/ditchd")
    guard FileManager.default.isExecutableFile(atPath: executable.path) else {
      return false
    }
    let process = Process()
    process.executableURL = executable
    process.currentDirectoryURL = Bundle.main.bundleURL
    process.environment = ProcessInfo.processInfo.environment
    do {
      try process.run()
      return true
    } catch {
      NSLog("The Ditch could not start its runtime: \(error)")
      return false
    }
  }

  private func bundledExecutable(_ name: String) -> String {
    Bundle.main.bundleURL
      .appendingPathComponent("Contents/MacOS")
      .appendingPathComponent(name)
      .path
  }

  private func showMainWindow() {
    NSApp.activate(ignoringOtherApps: true)
    if let window = NSApp.windows.first(where: { $0 is MainFlutterWindow }) ?? NSApp.windows.first {
      window.deminiaturize(nil)
      window.makeKeyAndOrderFront(nil)
    }
  }
}
