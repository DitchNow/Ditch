import Cocoa
import FlutterMacOS

@main
class AppDelegate: FlutterAppDelegate {
  private var statusItem: NSStatusItem?
  private var statusTimer: Timer?
  private var runtimeStartInFlight = false
  private var runtimeWaiters: [(Bool) -> Void] = []
  private var lastRuntimeStatus: RuntimeStatus?

  override func applicationDidFinishLaunching(_ notification: Notification) {
    configureStatusItem()
    ensureRuntimeAvailable()
    statusTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
      self?.refreshRuntimeStatus()
    }
  }

  func configureStatusBarChannel(messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(
      name: "the_ditch/status_bar",
      binaryMessenger: messenger)

    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "update":
        result(nil)
      case "showWindow":
        self.showMainWindow()
        result(nil)
      case "ensureRuntime":
        self.ensureRuntimeAvailable {
          result($0)
        }
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

  private func configureStatusItem() {
    let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    statusItem = item
    item.button?.image = statusImage()
    item.button?.toolTip = "The Ditch Runtime"

    let menu = NSMenu()
    let showItem = NSMenuItem(title: "Show The Ditch", action: #selector(showTheDitch), keyEquivalent: "")
    showItem.target = self
    menu.addItem(showItem)
    menu.addItem(NSMenuItem.separator())
    let status = NSMenuItem(title: "Runtime: Starting", action: nil, keyEquivalent: "")
    status.tag = 1001
    status.isEnabled = false
    menu.addItem(status)
    let codexHome = NSMenuItem(title: "Codex home: Unknown", action: nil, keyEquivalent: "")
    codexHome.tag = 1002
    codexHome.isEnabled = false
    menu.addItem(codexHome)
    let quitItem = NSMenuItem(title: "Quit The Ditch", action: #selector(quitTheDitch), keyEquivalent: "q")
    quitItem.target = self
    menu.addItem(NSMenuItem.separator())
    menu.addItem(quitItem)
    item.menu = menu
  }

  private func statusImage() -> NSImage {
    let image = NSImage(size: NSSize(width: 18, height: 18))
    image.lockFocus()
    NSColor.labelColor.setStroke()
    let outer = NSBezierPath(roundedRect: NSRect(x: 2, y: 2, width: 14, height: 14), xRadius: 3, yRadius: 3)
    outer.lineWidth = 1.8
    outer.stroke()
    let inner = NSBezierPath(ovalIn: NSRect(x: 6, y: 6, width: 6, height: 6))
    inner.lineWidth = 1.5
    inner.stroke()
    image.unlockFocus()
    image.isTemplate = true
    return image
  }

  private func logRuntime(_ message: String) {
    let logsURL = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library", isDirectory: true)
      .appendingPathComponent("Application Support", isDirectory: true)
      .appendingPathComponent("The Ditch", isDirectory: true)
      .appendingPathComponent("logs", isDirectory: true)
    try? FileManager.default.createDirectory(at: logsURL, withIntermediateDirectories: true)
    let logURL = logsURL.appendingPathComponent("runtime-coordinator.log")
    let line = "\(Date()) \(message)\n"
    guard let data = line.data(using: .utf8) else {
      return
    }
    if FileManager.default.fileExists(atPath: logURL.path),
      let handle = try? FileHandle(forWritingTo: logURL)
    {
      handle.seekToEndOfFile()
      handle.write(data)
      try? handle.close()
    } else {
      try? data.write(to: logURL)
    }
  }

  private func bundledExecutable(_ name: String) -> String {
    Bundle.main.bundleURL
      .appendingPathComponent("Contents/MacOS")
      .appendingPathComponent(name)
      .path
  }

  private func runtimeStatus() -> RuntimeStatus? {
    let cli = bundledExecutable("ditch_cli")
    guard FileManager.default.isExecutableFile(atPath: cli) else {
      return nil
    }
    let process = Process()
    let stdout = Pipe()
    process.executableURL = URL(fileURLWithPath: cli)
    process.arguments = ["runtime", "status"]
    process.standardOutput = stdout
    process.standardError = Pipe()
    do {
      try process.run()
      process.waitUntilExit()
    } catch {
      return nil
    }
    guard process.terminationStatus == 0,
      let root = try? JSONSerialization.jsonObject(
        with: stdout.fileHandleForReading.readDataToEndOfFile()) as? [String: Any],
      let status = root["RuntimeStatus"] as? [String: Any],
      let pid = status["pid"] as? Int,
      let active = status["active_session_count"] as? Int,
      let attention = status["attention_count"] as? Int
    else {
      return nil
    }
    let codexHome = status["codex_home"] as? String
    return RuntimeStatus(pid: Int32(pid), active: active, attention: attention, codexHome: codexHome)
  }

  private func refreshRuntimeStatus() {
    DispatchQueue.global(qos: .utility).async { [weak self] in
      guard let self else { return }
      let status = self.runtimeStatus()
      DispatchQueue.main.async {
        self.applyRuntimeStatus(status)
      }
    }
  }

  private func applyRuntimeStatus(_ status: RuntimeStatus?) {
    lastRuntimeStatus = status
    let statusMenuItem = statusItem?.menu?.item(withTag: 1001)
    let codexHomeItem = statusItem?.menu?.item(withTag: 1002)
    guard let status else {
      statusMenuItem?.title = "Runtime: Offline"
      codexHomeItem?.title = "Codex home: Unknown"
      statusItem?.button?.title = ""
      statusItem?.length = NSStatusItem.squareLength
      statusItem?.button?.toolTip = "The Ditch Runtime • Offline"
      return
    }
    statusMenuItem?.title = "Runtime: Running • \(status.active) active • \(status.attention) alerts"
    codexHomeItem?.title = "Codex home: \(status.codexHome ?? "Default (~/.codex)")"
    if status.active > 0 {
      statusItem?.button?.title = " \(status.active)"
      statusItem?.length = NSStatusItem.variableLength
    } else if status.attention > 0 {
      statusItem?.button?.title = " !\(status.attention)"
      statusItem?.length = NSStatusItem.variableLength
    } else {
      statusItem?.button?.title = ""
      statusItem?.length = NSStatusItem.squareLength
    }
    statusItem?.button?.toolTip = "The Ditch Runtime • \(status.active) active"
  }

  private func ensureRuntimeAvailable(completion: ((Bool) -> Void)? = nil) {
    if let completion {
      runtimeWaiters.append(completion)
    }
    guard !runtimeStartInFlight else {
      return
    }
    runtimeStartInFlight = true
    DispatchQueue.global(qos: .utility).async { [weak self] in
      guard let self else { return }
      if let status = self.runtimeStatus() {
        DispatchQueue.main.async {
          self.runtimeStartInFlight = false
          self.applyRuntimeStatus(status)
          self.finishRuntimeWaiters(true)
        }
        return
      }

      let runtime = self.bundledExecutable("ditchd")
      guard FileManager.default.isExecutableFile(atPath: runtime) else {
        self.logRuntime("bundled ditchd is missing or not executable: \(runtime)")
        DispatchQueue.main.async {
          self.runtimeStartInFlight = false
          self.finishRuntimeWaiters(false)
        }
        return
      }
      let process = Process()
      process.executableURL = URL(fileURLWithPath: runtime)
      process.currentDirectoryURL = URL(fileURLWithPath: Bundle.main.bundlePath)
      process.environment = ProcessInfo.processInfo.environment
      process.standardOutput = FileHandle.nullDevice
      process.standardError = FileHandle.nullDevice
      do {
        try process.run()
        let codexHome = process.environment?["CODEX_HOME"] ?? "<default ~/.codex>"
        self.logRuntime("started bundled ditchd pid=\(process.processIdentifier) CODEX_HOME=\(codexHome)")
      } catch {
        self.logRuntime("failed to start bundled ditchd: \(error)")
      }

      var status: RuntimeStatus?
      for _ in 0..<20 {
        Thread.sleep(forTimeInterval: 0.1)
        status = self.runtimeStatus()
        if status != nil { break }
      }
      DispatchQueue.main.async {
        self.runtimeStartInFlight = false
        self.applyRuntimeStatus(status)
        self.finishRuntimeWaiters(status != nil)
      }
    }
  }

  private func finishRuntimeWaiters(_ ready: Bool) {
    let waiters = runtimeWaiters
    runtimeWaiters.removeAll()
    for waiter in waiters {
      waiter(ready)
    }
  }

  @objc private func showTheDitch() {
    showMainWindow()
  }

  @objc private func quitTheDitch() {
    if let status = lastRuntimeStatus, status.active > 0 {
      let alert = NSAlert()
      alert.messageText = "Quit and stop active agents?"
      alert.informativeText = "This will stop \(status.active) active agent session(s) and quit The Ditch."
      alert.addButton(withTitle: "Quit and Stop Agents")
      alert.addButton(withTitle: "Cancel")
      guard alert.runModal() == .alertFirstButtonReturn else { return }
    }
    let cli = bundledExecutable("ditch_cli")
    if FileManager.default.isExecutableFile(atPath: cli) {
      let process = Process()
      process.executableURL = URL(fileURLWithPath: cli)
      process.arguments = ["runtime", "stop"]
      try? process.run()
      process.waitUntilExit()
    }
    NSApp.terminate(nil)
  }

  private func showMainWindow() {
    NSApp.activate(ignoringOtherApps: true)
    if let window = NSApp.windows.first(where: { $0 is MainFlutterWindow }) ?? NSApp.windows.first {
      window.deminiaturize(nil)
      window.makeKeyAndOrderFront(nil)
      return
    }
    NSApp.sendAction(#selector(NSApplicationDelegate.applicationShouldHandleReopen(_:hasVisibleWindows:)),
      to: nil,
      from: NSApp)
  }
}

private struct RuntimeStatus {
  let pid: Int32
  let active: Int
  let attention: Int
  let codexHome: String?
}
