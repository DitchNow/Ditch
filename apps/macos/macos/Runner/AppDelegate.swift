import Cocoa
import Darwin
import FlutterMacOS
import ServiceManagement
import UserNotifications

/// Native shell for the foreground control surface.
///
/// Runtime supervision deliberately belongs to the independent integrated
/// `ditchd` menu-bar process.
/// Terminating this process must never stop The Ditch Runtime or its agents.
@main
class AppDelegate: FlutterAppDelegate {
  private static let runtimeLoginItemIdentifier = "ai.theditch.runtime"
  private static let obsoleteLoginItemIdentifier = "ai.theditch.status"
  private static let processTimeoutExitCode: Int32 = 124
  private static let notificationControlSocketName = "notification-control.sock"

  private var applicationChannel: FlutterMethodChannel?
  private var pendingAgentNavigation: [String: String]?
  private var uninstallInProgress = false

  override func applicationDidFinishLaunching(_ notification: Notification) {
    applyThemeMode(UserDefaults.standard.string(forKey: "themeMode") ?? "system")
    persistRuntimeEnvironment()
    registerStatusHelper()
    configureUninstallMenuItem()
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
      let obsolete = SMAppService.loginItem(identifier: Self.obsoleteLoginItemIdentifier)
      if obsolete.status == .enabled {
        try? obsolete.unregister()
      }
      let service = SMAppService.loginItem(identifier: Self.runtimeLoginItemIdentifier)
      guard service.status != .enabled else { return }
      do {
        try service.register()
      } catch {
        NSLog("The Ditch could not register its runtime: \(error)")
      }
    } else {
      _ = SMLoginItemSetEnabled(Self.runtimeLoginItemIdentifier as CFString, true)
    }
  }

  private func configureUninstallMenuItem() {
    guard let applicationMenu = NSApp.mainMenu?.items.first?.submenu,
      !applicationMenu.items.contains(where: { $0.action == #selector(uninstallApplication(_:)) })
    else { return }

    let item = NSMenuItem(
      title: "Uninstall The Ditch…",
      action: #selector(uninstallApplication(_:)),
      keyEquivalent: "")
    item.target = self
    applicationMenu.insertItem(item, at: max(0, applicationMenu.numberOfItems - 2))
  }

  @objc private func uninstallApplication(_ sender: Any?) {
    guard !uninstallInProgress else { return }

    let alert = NSAlert()
    alert.messageText = "Uninstall The Ditch?"
    alert.informativeText =
      "This stops all running agents, removes The Ditch's background service and app data, and moves the application to Trash. Your project folders will not be deleted."
    alert.alertStyle = .critical
    alert.addButton(withTitle: "Uninstall")
    alert.addButton(withTitle: "Cancel")
    guard alert.runModal() == .alertFirstButtonReturn else { return }

    uninstallInProgress = true
    let applicationURL = Bundle.main.bundleURL.standardizedFileURL
    DispatchQueue.global(qos: .userInitiated).async { [weak self] in
      guard let self else { return }
      _ = Self.runProcess(
        executable: self.bundledExecutable("ditch_cli"),
        arguments: ["runtime", "stop"],
        timeout: 5)
      DispatchQueue.main.async {
        do {
          try self.unregisterRuntimeLoginItems()
        } catch {
          self.presentUninstallFailure(
            "The background service could not be unregistered. \(error.localizedDescription)")
          return
        }
        self.removeNotificationsAndDefaults()
        DispatchQueue.global(qos: .userInitiated).async {
          Self.terminateRemainingRuntimeProcesses()
          let failures = Self.removeOwnedApplicationData()
          DispatchQueue.main.async {
            guard failures.isEmpty else {
              self.presentUninstallFailure(
                "Some app data could not be removed:\n\n\(failures.joined(separator: "\n"))")
              return
            }
            self.recycleApplication(at: applicationURL)
          }
        }
      }
    }
  }

  private func unregisterRuntimeLoginItems() throws {
    if #available(macOS 13.0, *) {
      for identifier in [
        Self.runtimeLoginItemIdentifier,
        Self.obsoleteLoginItemIdentifier,
      ] {
        let service = SMAppService.loginItem(identifier: identifier)
        if service.status == .enabled || service.status == .requiresApproval {
          try service.unregister()
        }
      }
    } else {
      _ = SMLoginItemSetEnabled(Self.runtimeLoginItemIdentifier as CFString, false)
      _ = SMLoginItemSetEnabled(Self.obsoleteLoginItemIdentifier as CFString, false)
    }
  }

  private func removeNotificationsAndDefaults() {
    let notifications = UNUserNotificationCenter.current()
    notifications.removeAllDeliveredNotifications()
    notifications.removeAllPendingNotificationRequests()
    for identifier in [
      "ai.theditch.app",
      Self.runtimeLoginItemIdentifier,
      Self.obsoleteLoginItemIdentifier,
    ] {
      UserDefaults.standard.removePersistentDomain(forName: identifier)
    }
  }

  private func recycleApplication(at applicationURL: URL) {
    guard FileManager.default.fileExists(atPath: applicationURL.path) else {
      exit(EXIT_SUCCESS)
    }
    NSWorkspace.shared.recycle([applicationURL]) { [weak self] _, error in
      if let error {
        self?.presentUninstallFailure(
          "The application could not be moved to Trash. \(error.localizedDescription)")
        return
      }
      exit(EXIT_SUCCESS)
    }
  }

  private func presentUninstallFailure(_ message: String) {
    uninstallInProgress = false
    let alert = NSAlert()
    alert.messageText = "The Ditch could not be completely uninstalled"
    alert.informativeText = message
    alert.alertStyle = .warning
    alert.runModal()
  }

  static func uninstallArtifactURLs(homeDirectory: URL) -> [URL] {
    let library = homeDirectory.appendingPathComponent("Library", isDirectory: true)
    let identifiers = [
      "ai.theditch.app",
      runtimeLoginItemIdentifier,
      obsoleteLoginItemIdentifier,
    ]
    var urls = [
      library.appendingPathComponent("Application Support/The Ditch", isDirectory: true),
    ]
    for identifier in identifiers {
      urls.append(
        contentsOf: [
          library.appendingPathComponent("Caches/\(identifier)", isDirectory: true),
          library.appendingPathComponent("Preferences/\(identifier).plist"),
          library.appendingPathComponent(
            "Saved Application State/\(identifier).savedState", isDirectory: true),
          library.appendingPathComponent("HTTPStorages/\(identifier)", isDirectory: true),
          library.appendingPathComponent("HTTPStorages/\(identifier).binarycookies"),
          library.appendingPathComponent("Cookies/\(identifier).binarycookies"),
          library.appendingPathComponent("WebKit/\(identifier)", isDirectory: true),
        ])
    }
    return urls
  }

  private static func removeOwnedApplicationData() -> [String] {
    let fileManager = FileManager.default
    var failures = [String]()
    for url in uninstallArtifactURLs(homeDirectory: fileManager.homeDirectoryForCurrentUser) {
      guard fileManager.fileExists(atPath: url.path) else { continue }
      do {
        try fileManager.removeItem(at: url)
      } catch {
        failures.append("\(url.path): \(error.localizedDescription)")
      }
    }
    return failures
  }

  private static func terminateRemainingRuntimeProcesses() {
    for processName in ["ditch_cli", "ditchd"] {
      _ = runProcess(
        executable: "/usr/bin/pkill",
        arguments: ["-x", processName],
        timeout: 2)
    }
  }

  func configureApplicationChannel(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(
      name: "the_ditch/application",
      binaryMessenger: messenger)
    applicationChannel = channel

    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else {
        result(false)
        return
      }
      switch call.method {
      case "showWindow":
        self.showMainWindow()
        result(true)
      case "consumePendingNavigation":
        let pending = self.pendingAgentNavigation
        self.pendingAgentNavigation = nil
        result(pending)
      case "runtimeAvailable":
        self.runtimeAvailable { result($0) }
      case "getThemeMode":
        result(UserDefaults.standard.string(forKey: "themeMode") ?? "system")
      case "setThemeMode":
        let mode = call.arguments as? String ?? "system"
        UserDefaults.standard.set(mode, forKey: "themeMode")
        self.applyThemeMode(mode)
        result(true)
      case "getPaneWidths":
        result([
          "projects": UserDefaults.standard.double(forKey: "projectSidebarWidth"),
          "inspector": UserDefaults.standard.double(forKey: "inspectorWidth"),
        ])
      case "setPaneWidths":
        let widths = call.arguments as? [String: Any]
        if let projects = widths?["projects"] as? NSNumber {
          UserDefaults.standard.set(projects.doubleValue, forKey: "projectSidebarWidth")
        }
        if let inspector = widths?["inspector"] as? NSNumber {
          UserDefaults.standard.set(inspector.doubleValue, forKey: "inspectorWidth")
        }
        result(true)
      case "revealInFinder":
        guard let path = call.arguments as? String,
          FileManager.default.fileExists(atPath: path)
        else {
          result(false)
          return
        }
        let projectURL = URL(fileURLWithPath: path, isDirectory: true)
        result(NSWorkspace.shared.open(projectURL))
      case "openActivityMonitor":
        let url = URL(fileURLWithPath: "/System/Applications/Utilities/Activity Monitor.app")
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        NSWorkspace.shared.openApplication(at: url, configuration: configuration) { _, error in
          result(error == nil)
        }
      case "openURL":
        guard let rawURL = call.arguments as? String,
          let url = URL(string: rawURL),
          url.scheme == "https"
        else {
          result(false)
          return
        }
        result(NSWorkspace.shared.open(url))
      case "openCodexLogin":
        guard let binary = call.arguments as? String else {
          result(false)
          return
        }
        result(self.openCodexLogin(binary: binary))
      case "notificationAuthorizationStatus":
        self.notificationAuthorizationStatus(requestAuthorization: false, result: result)
      case "requestNotificationAuthorization":
        self.notificationAuthorizationStatus(requestAuthorization: true, result: result)
      case "openNotificationSettings":
        result(self.openNotificationSettings())
      case "quitUI":
        result(true)
        NSApp.terminate(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  private func notificationAuthorizationStatus(
    requestAuthorization: Bool,
    result: @escaping FlutterResult
  ) {
    let command = requestAuthorization
      ? "requestAuthorization"
      : "status"
    let timeout: TimeInterval = requestAuthorization ? 120 : 10
    requestNotificationControl(command: command, timeout: timeout, result: result)
  }

  private func requestNotificationControl(
    command: String,
    timeout: TimeInterval,
    result: @escaping FlutterResult
  ) {
    DispatchQueue.global(qos: .userInitiated).async {
      do {
        let values = try Self.sendNotificationControlCommand(command, timeout: timeout)
        DispatchQueue.main.async { self.finishNotificationControl(values, result: result) }
        return
      } catch {
        // The helper can still be starting after login-item registration. If it
        // is not running, launch the app bundle through LaunchServices so macOS
        // gives it a real application identity. Never execute its Mach-O file
        // directly for notification authorization.
      }
      DispatchQueue.main.async {
        self.launchRuntimeApplication { launchError in
          if let launchError {
            result(FlutterError(
              code: "notification_helper_unavailable",
              message: "The notification runtime could not be started.",
              details: launchError.localizedDescription))
            return
          }
          DispatchQueue.global(qos: .userInitiated).async {
            let deadline = Date().addingTimeInterval(5)
            var lastError: Error?
            repeat {
              do {
                let values = try Self.sendNotificationControlCommand(
                  command, timeout: timeout)
                DispatchQueue.main.async {
                  self.finishNotificationControl(values, result: result)
                }
                return
              } catch {
                lastError = error
                Thread.sleep(forTimeInterval: 0.1)
              }
            } while Date() < deadline
            DispatchQueue.main.async {
              result(FlutterError(
                code: "notification_helper_unavailable",
                message: "The notification runtime did not become ready.",
                details: lastError?.localizedDescription))
            }
          }
        }
      }
    }
  }

  private func finishNotificationControl(
    _ values: [String: Any],
    result: @escaping FlutterResult
  ) {
    if let error = values["error"] as? [String: Any] {
      result(FlutterError(
        code: "notification_authorization_failed",
        message: "macOS could not update notification permission.",
        details: error))
      return
    }
    guard values["bundleIdentifier"] as? String == Self.runtimeLoginItemIdentifier else {
      result(FlutterError(
        code: "notification_helper_identity_mismatch",
        message: "The notification response did not come from The Ditch Runtime.",
        details: values["bundleIdentifier"]))
      return
    }
    result(values)
  }

  private func launchRuntimeApplication(completion: @escaping (Error?) -> Void) {
    if !NSRunningApplication.runningApplications(
      withBundleIdentifier: Self.runtimeLoginItemIdentifier
    ).isEmpty {
      completion(nil)
      return
    }
    let helper = Bundle.main.bundleURL.appendingPathComponent(
      "Contents/Library/LoginItems/The Ditch Runtime.app", isDirectory: true)
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = false
    NSWorkspace.shared.openApplication(
      at: helper,
      configuration: configuration
    ) { _, error in
      completion(error)
    }
  }

  private static func notificationControlSocketPath() -> String {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch", isDirectory: true)
      .appendingPathComponent(notificationControlSocketName)
      .path
  }

  private static func sendNotificationControlCommand(
    _ command: String,
    timeout: TimeInterval
  ) throws -> [String: Any] {
    let descriptor = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard descriptor >= 0 else {
      throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }
    defer { Darwin.close(descriptor) }

    var noSigPipe: Int32 = 1
    let noSigPipeSize = socklen_t(MemoryLayout.size(ofValue: noSigPipe))
    _ = withUnsafePointer(to: &noSigPipe) {
      setsockopt(descriptor, SOL_SOCKET, SO_NOSIGPIPE, $0, noSigPipeSize)
    }
    var socketTimeout = timeval(
      tv_sec: Int(timeout),
      tv_usec: Int32((timeout.truncatingRemainder(dividingBy: 1)) * 1_000_000))
    let socketTimeoutSize = socklen_t(MemoryLayout.size(ofValue: socketTimeout))
    _ = withUnsafePointer(to: &socketTimeout) {
      setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, $0, socketTimeoutSize)
    }

    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let path = notificationControlSocketPath()
    guard path.utf8.count < MemoryLayout.size(ofValue: address.sun_path) else {
      throw CocoaError(.fileWriteInvalidFileName)
    }
    withUnsafeMutablePointer(to: &address.sun_path) { pointer in
      path.withCString { source in
        _ = strcpy(UnsafeMutableRawPointer(pointer).assumingMemoryBound(to: CChar.self), source)
      }
    }
    let addressLength = socklen_t(MemoryLayout<sa_family_t>.size + path.utf8.count + 1)
    let connected = withUnsafePointer(to: &address) { pointer in
      pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        Darwin.connect(descriptor, $0, addressLength)
      }
    }
    guard connected == 0 else {
      throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }

    let request = try JSONSerialization.data(withJSONObject: ["command": command]) + Data([0x0A])
    try request.withUnsafeBytes { bytes in
      var offset = 0
      while offset < bytes.count {
        let written = Darwin.write(descriptor, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
        guard written > 0 else {
          throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        offset += written
      }
    }

    var response = Data()
    var buffer = [UInt8](repeating: 0, count: 4096)
    while response.count <= 64 * 1024 {
      let count = Darwin.read(descriptor, &buffer, buffer.count)
      guard count > 0 else {
        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
      }
      response.append(buffer, count: count)
      if let newline = response.firstIndex(of: 0x0A) {
        response = response[..<newline]
        break
      }
    }
    guard response.count <= 64 * 1024,
      let values = try JSONSerialization.jsonObject(with: response) as? [String: Any]
    else {
      throw CocoaError(.fileReadCorruptFile)
    }
    return values
  }

  private func openNotificationSettings() -> Bool {
    guard let url = URL(
      string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension")
    else { return false }
    return NSWorkspace.shared.open(url)
  }

  private func openCodexLogin(binary: String) -> Bool {
    guard binary.hasPrefix("/"),
      FileManager.default.isExecutableFile(atPath: binary)
    else { return false }

    let support = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch", isDirectory: true)
    let script = support.appendingPathComponent("Sign in to Codex.command")
    let quotedBinary = "'" + binary.replacingOccurrences(of: "'", with: "'\\''") + "'"
    let contents = """
      #!/bin/zsh
      clear
      echo "The Ditch is opening your selected Codex CLI:"
      echo \(quotedBinary)
      echo
      \(quotedBinary) login
      status=$?
      echo
      if [ $status -eq 0 ]; then
        echo "Codex sign-in completed. Return to The Ditch."
      else
        echo "Codex sign-in failed with exit code $status."
      fi
      echo "Press any key to close this window."
      read -k 1
      exit $status
      """
    do {
      try FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
      try contents.write(to: script, atomically: true, encoding: .utf8)
      try FileManager.default.setAttributes(
        [.posixPermissions: NSNumber(value: 0o700)],
        ofItemAtPath: script.path)
      return NSWorkspace.shared.open(script)
    } catch {
      NSLog("The Ditch could not open Codex sign-in: \(error)")
      return false
    }
  }

  override func application(_ application: NSApplication, open urls: [URL]) {
    for url in urls where handleAgentNavigationURL(url) {
      return
    }
  }

  @discardableResult
  private func handleAgentNavigationURL(_ url: URL) -> Bool {
    guard url.scheme == "theditch", url.host == "agent",
      let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
    else { return false }
    let values = Dictionary(
      uniqueKeysWithValues: (components.queryItems ?? []).compactMap { item in
        item.value.map { (item.name, $0) }
      })
    guard let projectId = values["project"], UUID(uuidString: projectId) != nil,
      let agentId = values["agent"], UUID(uuidString: agentId) != nil
    else { return false }

    var target = ["projectId": projectId, "agentId": agentId]
    if let attentionId = values["attention"], UUID(uuidString: attentionId) != nil {
      target["attentionId"] = attentionId
    }
    pendingAgentNavigation = target
    showMainWindow()
    applicationChannel?.invokeMethod("openAgent", arguments: target) { [weak self] response in
      if (response as? Bool) == true {
        self?.pendingAgentNavigation = nil
      }
    }
    return true
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
      if Self.runProcess(
        executable: cli,
        arguments: ["runtime", "status"],
        timeout: 2) == 0
      {
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
    }
  }

  private func runtimeResponds(cli: String) -> Bool {
    Self.runProcess(
      executable: cli,
      arguments: ["runtime", "status"],
      timeout: 2) == 0
  }

  private static func runProcess(
    executable: String,
    arguments: [String],
    timeout: TimeInterval
  ) -> Int32 {
    guard FileManager.default.isExecutableFile(atPath: executable) else {
      return 127
    }
    let process = Process()
    let finished = DispatchSemaphore(value: 0)
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    process.terminationHandler = { _ in finished.signal() }
    do {
      try process.run()
    } catch {
      return 1
    }

    if finished.wait(timeout: .now() + timeout) == .success {
      return process.terminationStatus
    }

    if process.isRunning {
      process.terminate()
    }
    if finished.wait(timeout: .now() + 0.5) == .timedOut && process.isRunning {
      Darwin.kill(process.processIdentifier, SIGKILL)
      _ = finished.wait(timeout: .now() + 0.5)
    }
    return processTimeoutExitCode
  }

  /// Starts the independent integrated runtime as an application. Launching
  /// the nested bundle through LaunchServices preserves its macOS identity for
  /// login-item and notification services.
  private func startRuntimeHost() -> Bool {
    let helper = Bundle.main.bundleURL.appendingPathComponent(
      "Contents/Library/LoginItems/The Ditch Runtime.app", isDirectory: true)
    guard FileManager.default.fileExists(atPath: helper.path) else {
      return false
    }
    var opened = false
    DispatchQueue.main.sync {
      opened = NSWorkspace.shared.open(helper)
    }
    if !opened { NSLog("The Ditch could not start its runtime application") }
    return opened
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

  private func applyThemeMode(_ mode: String) {
    switch mode {
    case "light":
      NSApp.appearance = NSAppearance(named: .aqua)
    case "dark":
      NSApp.appearance = NSAppearance(named: .darkAqua)
    default:
      NSApp.appearance = nil
    }
    for window in NSApp.windows {
      window.appearance = NSApp.appearance
      window.backgroundColor = NSColor.windowBackgroundColor
    }
  }
}
