import Cocoa
import Foundation
import UserNotifications

@_silgen_name("ditch_runtime_run")
private func ditchRuntimeRun() -> Int32

@main
final class StatusHost: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
  private static let agentEventCategory = "DITCH_AGENT_EVENT"
  private static let openAgentAction = "DITCH_OPEN_AGENT"
  private static let notifiedAttentionDefaultsKey = "notifiedAttentionIds"
  private var statusItem: NSStatusItem?
  private let menu = NSMenu()
  private let stateItem = NSMenuItem(title: "Runtime: Starting", action: nil, keyEquivalent: "")
  private let sessionsItem = NSMenuItem(title: "0 active sessions", action: nil, keyEquivalent: "")
  private let attentionItem = NSMenuItem(title: "0 alerts", action: nil, keyEquivalent: "")
  private let codexHomeItem = NSMenuItem(title: "Codex home: Unknown", action: nil, keyEquivalent: "")
  private var timer: Timer?
  private var refreshInFlight = false
  private var runtimeStartInFlight = false
  private var runtimeThreadStarted = false
  private var shutdownRequested = false
  private var attentionStreamProcess: Process?
  private var attentionStreamBuffer = Data()
  private var activeAttentionIds = Set<String>()
  private var notifiedAttentionIds = Set<String>()
  private var lastRuntimeStatus: RuntimeStatus?
  private var lastStartAttempt = Date.distantPast
  private var appPath = ""
  private var helperDirectory = ""
  private var pidFilePath = ""
  private var logFilePath = ""

  static func main() {
    let app = NSApplication.shared
    let delegate = StatusHost()
    app.delegate = delegate
    app.setActivationPolicy(.accessory)
    app.run()
  }

  func applicationDidFinishLaunching(_ notification: Notification) {
    restoreRuntimeEnvironment()
    logFilePath = StatusHost.defaultLogFilePath()
    appPath = StatusHost.parentAppPath()
    helperDirectory = Bundle.main.executableURL?
      .deletingLastPathComponent()
      .path ?? URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().path
    pidFilePath = StatusHost.defaultPidFilePath()
    let obsoletePid = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch/ditch-status-host.pid").path
    try? FileManager.default.removeItem(atPath: obsoletePid)
    log("launch integrated ditchd appPath=\(appPath) helperDirectory=\(helperDirectory)")
    writePidFile()
    configureNotifications()
    configureStatusItem()
    ensureRuntimeAvailable()
    timer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak self] _ in
      self?.refreshRuntimeStatus()
    }
  }

  private func configureNotifications() {
    let center = UNUserNotificationCenter.current()
    center.delegate = self
    let open = UNNotificationAction(
      identifier: Self.openAgentAction,
      title: "Open",
      options: [.foreground])
    center.setNotificationCategories([
      UNNotificationCategory(
        identifier: Self.agentEventCategory,
        actions: [open],
        intentIdentifiers: [],
        options: [])
    ])
    notifiedAttentionIds = Set(
      UserDefaults.standard.stringArray(forKey: Self.notifiedAttentionDefaultsKey) ?? [])
  }

  func applicationWillTerminate(_ notification: Notification) {
    timer?.invalidate()
    stopAttentionStream()
    if !pidFilePath.isEmpty {
      try? FileManager.default.removeItem(atPath: pidFilePath)
    }
  }

  private static func defaultPidFilePath() -> String {
    let support = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch", isDirectory: true)
    try? FileManager.default.createDirectory(at: support, withIntermediateDirectories: true)
    return support.appendingPathComponent("ditchd.pid").path
  }

  private static func defaultLogFilePath() -> String {
    let logs = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch/logs", isDirectory: true)
    try? FileManager.default.createDirectory(at: logs, withIntermediateDirectories: true)
    return logs.appendingPathComponent("runtime.log").path
  }

  private static func parentAppPath() -> String {
    var candidate = Bundle.main.bundleURL
    while candidate.path != "/" {
      if candidate.pathExtension == "app",
        candidate.lastPathComponent == "The Ditch.app"
      {
        return candidate.path
      }
      candidate.deleteLastPathComponent()
    }
    return Bundle.main.bundleURL.path
  }

  private func restoreRuntimeEnvironment() {
    let file = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch/codex-home")
    guard let value = try? String(contentsOf: file, encoding: .utf8)
      .trimmingCharacters(in: .whitespacesAndNewlines),
      !value.isEmpty,
      value.hasPrefix("/")
    else { return }
    setenv("CODEX_HOME", value, 1)
  }

  private func log(_ message: String) {
    guard !logFilePath.isEmpty else {
      return
    }
    let line = "\(Date()) \(message)\n"
    guard let data = line.data(using: .utf8) else {
      return
    }
    if FileManager.default.fileExists(atPath: logFilePath),
      let handle = try? FileHandle(forWritingTo: URL(fileURLWithPath: logFilePath))
    {
      handle.seekToEndOfFile()
      handle.write(data)
      try? handle.close()
    } else {
      try? data.write(to: URL(fileURLWithPath: logFilePath))
    }
  }

  private func writePidFile() {
    guard !pidFilePath.isEmpty else {
      return
    }
    try? "\(ProcessInfo.processInfo.processIdentifier)\n".write(
      toFile: pidFilePath,
      atomically: true,
      encoding: .utf8)
  }

  private func configureStatusItem() {
    let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
    statusItem = item
    if let button = item.button {
      button.image = statusImage()
      button.imagePosition = .imageLeft
      button.title = ""
      button.toolTip = "The Ditch Runtime"
    }

    menu.autoenablesItems = false

    let showItem = NSMenuItem(
      title: "Show The Ditch",
      action: #selector(showTheDitch),
      keyEquivalent: "")
    showItem.target = self
    menu.addItem(showItem)
    menu.addItem(NSMenuItem.separator())

    stateItem.isEnabled = false
    sessionsItem.isEnabled = false
    attentionItem.isEnabled = false
    codexHomeItem.isEnabled = false
    menu.addItem(stateItem)
    menu.addItem(sessionsItem)
    menu.addItem(attentionItem)
    menu.addItem(codexHomeItem)
    menu.addItem(NSMenuItem.separator())

    let quitItem = NSMenuItem(
      title: "Quit The Ditch Runtime",
      action: #selector(quitRuntime),
      keyEquivalent: "q")
    quitItem.target = self
    menu.addItem(quitItem)

    item.menu = menu
  }

  private func refreshRuntimeStatus() {
    guard !refreshInFlight else {
      return
    }
    refreshInFlight = true
    DispatchQueue.global(qos: .utility).async { [weak self] in
      guard let self else {
        return
      }
      let status = self.runtimeStatus()
      DispatchQueue.main.async {
        self.refreshInFlight = false
        self.applyRuntimeStatus(status)
        if status == nil && !self.shutdownRequested {
          self.ensureRuntimeAvailable()
        }
      }
    }
  }

  private func applyRuntimeStatus(_ status: RuntimeStatus?) {
    lastRuntimeStatus = status
    guard let status else {
      stopAttentionStream()
      stateItem.title = "Runtime: Offline"
      sessionsItem.title = "0 active sessions"
      attentionItem.title = "0 alerts"
      codexHomeItem.title = "Codex home: Unknown"
      updateStatusButton(activeSessionCount: 0, attentionCount: 0)
      statusItem?.button?.toolTip = "The Ditch Runtime • Offline"
      return
    }

    stateItem.title = "Runtime: Running"
    sessionsItem.title = status.activeSessionCount == 1
      ? "1 active session"
      : "\(status.activeSessionCount) active sessions"
    attentionItem.title = status.attentionCount == 1
      ? "1 alert"
      : "\(status.attentionCount) alerts"
    codexHomeItem.title = "Codex home: \(status.codexHome ?? "Default (~/.codex)")"
    updateStatusButton(
      activeSessionCount: status.activeSessionCount,
      attentionCount: status.attentionCount)
    statusItem?.button?.toolTip =
      "The Ditch Runtime • \(status.activeSessionCount) active"
    startAttentionStream()
  }

  private func startAttentionStream() {
    guard !shutdownRequested, attentionStreamProcess == nil else { return }
    let cliPath = URL(fileURLWithPath: helperDirectory).appendingPathComponent("ditch_cli").path
    guard FileManager.default.isExecutableFile(atPath: cliPath) else {
      log("ditch_cli not executable at \(cliPath)")
      return
    }

    let process = Process()
    let stdout = Pipe()
    let stderr = Pipe()
    process.executableURL = URL(fileURLWithPath: cliPath)
    process.arguments = ["runtime", "attention-stream"]
    process.standardOutput = stdout
    process.standardError = stderr
    attentionStreamBuffer.removeAll(keepingCapacity: true)
    attentionStreamProcess = process

    stdout.fileHandleForReading.readabilityHandler = { [weak self] handle in
      let data = handle.availableData
      guard !data.isEmpty else { return }
      DispatchQueue.main.async { self?.consumeAttentionStream(data) }
    }
    stderr.fileHandleForReading.readabilityHandler = { [weak self] handle in
      let data = handle.availableData
      guard !data.isEmpty, let message = String(data: data, encoding: .utf8) else { return }
      self?.log("attention stream: \(message.trimmingCharacters(in: .whitespacesAndNewlines))")
    }
    process.terminationHandler = { [weak self, weak process] _ in
      DispatchQueue.main.async {
        guard let self, let process, self.attentionStreamProcess === process else { return }
        stdout.fileHandleForReading.readabilityHandler = nil
        stderr.fileHandleForReading.readabilityHandler = nil
        self.attentionStreamProcess = nil
        self.attentionStreamBuffer.removeAll(keepingCapacity: true)
      }
    }

    do {
      try process.run()
      log("attention stream connected")
    } catch {
      stdout.fileHandleForReading.readabilityHandler = nil
      stderr.fileHandleForReading.readabilityHandler = nil
      attentionStreamProcess = nil
      log("attention stream failed to start: \(error)")
    }
  }

  private func stopAttentionStream() {
    guard let process = attentionStreamProcess else { return }
    attentionStreamProcess = nil
    if let stdout = process.standardOutput as? Pipe {
      stdout.fileHandleForReading.readabilityHandler = nil
    }
    if let stderr = process.standardError as? Pipe {
      stderr.fileHandleForReading.readabilityHandler = nil
    }
    if process.isRunning { process.terminate() }
    attentionStreamBuffer.removeAll(keepingCapacity: true)
  }

  private func consumeAttentionStream(_ data: Data) {
    attentionStreamBuffer.append(data)
    while let newline = attentionStreamBuffer.firstIndex(of: 0x0A) {
      let line = attentionStreamBuffer[..<newline]
      attentionStreamBuffer.removeSubrange(...newline)
      guard !line.isEmpty else { continue }
      handleAttentionStreamLine(Data(line))
    }
  }

  private func handleAttentionStreamLine(_ data: Data) {
    guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let body = root["body"] as? [String: Any],
      let event = body["event"] as? [String: Any]
    else {
      log("ignored invalid attention stream event")
      return
    }

    if let values = event["AttentionSnapshotReplaced"] as? [Any] {
      reconcileNotifications(values.compactMap(Self.agentAttention))
      return
    }
    if let value = event["AttentionRaised"], let attention = Self.agentAttention(value) {
      activeAttentionIds.insert(attention.id)
      requestNotifications([attention])
      return
    }
    if let value = event["AttentionDismissed"] as? [String: Any],
      let id = Self.identifier(value["attention_id"])
    {
      activeAttentionIds.remove(id)
      notifiedAttentionIds.remove(id)
      persistNotifiedAttentionIds()
      let notificationId = "ditch-attention-\(id)"
      let center = UNUserNotificationCenter.current()
      center.removeDeliveredNotifications(withIdentifiers: [notificationId])
      center.removePendingNotificationRequests(withIdentifiers: [notificationId])
    }
  }

  private static func agentAttention(_ value: Any) -> AgentAttention? {
    guard let item = value as? [String: Any],
      let id = identifier(item["id"]),
      let kind = item["kind"] as? String,
      let projectId = identifier(item["project_id"]),
      let agentId = identifier(item["agent_id"]),
      let title = item["title"] as? String,
      let body = item["body"] as? String
    else { return nil }
    return AgentAttention(
      id: id,
      kind: kind,
      projectId: projectId,
      agentId: agentId,
      title: title,
      body: body)
  }

  private static func identifier(_ value: Any?) -> String? {
    if let value = value as? String { return value }
    if let value = value as? [String: Any] { return value["0"] as? String }
    return nil
  }

  private func reconcileNotifications(_ attention: [AgentAttention]) {
    let currentIds = Set(attention.map(\.id))
    activeAttentionIds = currentIds
    notifiedAttentionIds.formIntersection(currentIds)
    persistNotifiedAttentionIds()

    let center = UNUserNotificationCenter.current()
    center.getDeliveredNotifications { delivered in
      let obsolete = delivered
        .map(\.request.identifier)
        .filter { $0.hasPrefix("ditch-attention-") && !currentIds.contains(String($0.dropFirst("ditch-attention-".count))) }
      if !obsolete.isEmpty { center.removeDeliveredNotifications(withIdentifiers: obsolete) }
    }

    requestNotifications(attention.filter { !notifiedAttentionIds.contains($0.id) })
  }

  private func requestNotifications(_ attention: [AgentAttention]) {
    let newAttention = attention.filter { !notifiedAttentionIds.contains($0.id) }
    guard !newAttention.isEmpty else { return }
    let center = UNUserNotificationCenter.current()
    center.getNotificationSettings { [weak self] settings in
      guard let self else { return }
      switch settings.authorizationStatus {
      case .notDetermined:
        center.requestAuthorization(options: [.alert]) { granted, error in
          if let error { self.log("notification authorization failed: \(error)") }
          if granted { self.postNotifications(newAttention) }
        }
      case .authorized, .provisional, .ephemeral:
        self.postNotifications(newAttention)
      case .denied:
        break
      @unknown default:
        break
      }
    }
  }

  private func postNotifications(_ attention: [AgentAttention]) {
    let center = UNUserNotificationCenter.current()
    for item in attention {
      DispatchQueue.main.async { [weak self] in
        guard let self, !self.notifiedAttentionIds.contains(item.id) else { return }
        self.notifiedAttentionIds.insert(item.id)
        self.persistNotifiedAttentionIds()

        let content = UNMutableNotificationContent()
        content.title = item.title
        content.body = item.body
        content.categoryIdentifier = Self.agentEventCategory
        content.threadIdentifier = item.projectId
        content.userInfo = [
          "attentionId": item.id,
          "projectId": item.projectId,
          "agentId": item.agentId,
        ]
        let request = UNNotificationRequest(
          identifier: "ditch-attention-\(item.id)",
          content: content,
          trigger: nil)
        center.add(request) { error in
          guard let error else { return }
          self.log("notification delivery failed: \(error)")
          DispatchQueue.main.async {
            self.notifiedAttentionIds.remove(item.id)
            self.persistNotifiedAttentionIds()
          }
        }
      }
    }
  }

  private func persistNotifiedAttentionIds() {
    UserDefaults.standard.set(
      notifiedAttentionIds.sorted(),
      forKey: Self.notifiedAttentionDefaultsKey)
  }

  func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
  ) {
    if #available(macOS 11.0, *) {
      completionHandler([.banner, .list])
    } else {
      completionHandler([.alert])
    }
  }

  func userNotificationCenter(
    _ center: UNUserNotificationCenter,
    didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void
  ) {
    defer { completionHandler() }
    guard response.actionIdentifier == Self.openAgentAction
        || response.actionIdentifier == UNNotificationDefaultActionIdentifier,
      let projectId = response.notification.request.content.userInfo["projectId"] as? String,
      let agentId = response.notification.request.content.userInfo["agentId"] as? String,
      let attentionId = response.notification.request.content.userInfo["attentionId"] as? String
    else { return }
    openAgent(projectId: projectId, agentId: agentId, attentionId: attentionId)
  }

  private func openAgent(projectId: String, agentId: String, attentionId: String) {
    guard UUID(uuidString: projectId) != nil,
      UUID(uuidString: agentId) != nil,
      UUID(uuidString: attentionId) != nil,
      !appPath.isEmpty
    else { return }
    var components = URLComponents()
    components.scheme = "theditch"
    components.host = "agent"
    components.queryItems = [
      URLQueryItem(name: "project", value: projectId),
      URLQueryItem(name: "agent", value: agentId),
      URLQueryItem(name: "attention", value: attentionId),
    ]
    guard let url = components.url else { return }
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = true
    NSWorkspace.shared.open(
      [url],
      withApplicationAt: URL(fileURLWithPath: appPath),
      configuration: configuration) { [weak self] _, error in
        if let error { self?.log("opening notification target failed: \(error)") }
      }
  }

  /// AppKit and the Rust runtime share this process. AppKit remains on the main
  /// thread while the blocking Unix-socket server runs on a utility thread.
  private func ensureRuntimeAvailable() {
    guard !shutdownRequested, !runtimeStartInFlight else { return }
    guard Date().timeIntervalSince(lastStartAttempt) >= 3 else { return }
    runtimeStartInFlight = true
    lastStartAttempt = Date()

    DispatchQueue.global(qos: .utility).async { [weak self] in
      guard let self else { return }
      if let status = self.runtimeStatus() {
        DispatchQueue.main.async {
          self.runtimeStartInFlight = false
          self.applyRuntimeStatus(status)
        }
        return
      }

      if !self.runtimeThreadStarted {
        self.runtimeThreadStarted = true
        self.log("starting integrated runtime pid=\(ProcessInfo.processInfo.processIdentifier)")
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
          let exitCode = ditchRuntimeRun()
          self?.log("integrated runtime exited code=\(exitCode)")
          DispatchQueue.main.async {
            self?.runtimeThreadStarted = false
            self?.applyRuntimeStatus(nil)
          }
        }
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
      }
    }
  }

  private func updateStatusButton(activeSessionCount: Int, attentionCount: Int) {
    guard let button = statusItem?.button else {
      return
    }

    if activeSessionCount > 0 {
      button.title = " \(activeSessionCount)"
      statusItem?.length = NSStatusItem.variableLength
    } else if attentionCount > 0 {
      button.title = " !\(attentionCount)"
      statusItem?.length = NSStatusItem.variableLength
    } else {
      button.title = ""
      statusItem?.length = NSStatusItem.squareLength
    }
  }

  private func runtimeStatus() -> RuntimeStatus? {
    let output = runDitchCli(arguments: ["runtime", "status"])
    guard output.exitCode == 0,
      let data = output.stdout.data(using: .utf8),
      let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
      let status = root["RuntimeStatus"] as? [String: Any],
      let pid = status["pid"] as? Int,
      let activeSessionCount = status["active_session_count"] as? Int,
      let attentionCount = status["attention_count"] as? Int,
      let instanceId = status["instance_id"] as? String
    else {
      return nil
    }
    return RuntimeStatus(
      pid: Int32(pid),
      activeSessionCount: activeSessionCount,
      attentionCount: attentionCount,
      instanceId: instanceId,
      codexHome: status["codex_home"] as? String)
  }

  private func runDitchCli(arguments: [String]) -> CommandOutput {
    let cliPath = URL(fileURLWithPath: helperDirectory).appendingPathComponent("ditch_cli").path
    guard FileManager.default.isExecutableFile(atPath: cliPath) else {
      log("ditch_cli not executable at \(cliPath)")
      return CommandOutput(exitCode: 127, stdout: "", stderr: "ditch_cli not found")
    }

    let process = Process()
    let stdout = Pipe()
    let stderr = Pipe()
    process.executableURL = URL(fileURLWithPath: cliPath)
    process.arguments = arguments
    process.standardOutput = stdout
    process.standardError = stderr

    do {
      try process.run()
      process.waitUntilExit()
    } catch {
      log("ditch_cli \(arguments.joined(separator: " ")) failed to run: \(error)")
      return CommandOutput(exitCode: 1, stdout: "", stderr: "\(error)")
    }

    let stdoutText = String(
      data: stdout.fileHandleForReading.readDataToEndOfFile(),
      encoding: .utf8) ?? ""
    let stderrText = String(
      data: stderr.fileHandleForReading.readDataToEndOfFile(),
      encoding: .utf8) ?? ""
    return CommandOutput(exitCode: process.terminationStatus, stdout: stdoutText, stderr: stderrText)
  }

  @objc private func showTheDitch() {
    if let app = NSRunningApplication.runningApplications(withBundleIdentifier: "ai.theditch.app").first {
      app.activate(options: [.activateAllWindows])
      return
    }

    guard !appPath.isEmpty else {
      return
    }
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = true
    NSWorkspace.shared.openApplication(
      at: URL(fileURLWithPath: appPath),
      configuration: configuration)
  }

  @objc private func quitRuntime() {
    if let status = lastRuntimeStatus, status.activeSessionCount > 0 {
      let alert = NSAlert()
      alert.messageText = "Quit The Ditch Runtime?"
      alert.informativeText =
        "This will stop \(status.activeSessionCount) active agent session(s). The Ditch window may remain open but will disconnect."
      alert.addButton(withTitle: "Quit and Stop Agents")
      alert.addButton(withTitle: "Cancel")
      alert.alertStyle = .warning
      guard alert.runModal() == .alertFirstButtonReturn else { return }
    }

    shutdownRequested = true
    let output = runDitchCli(arguments: ["runtime", "stop"])
    if output.exitCode != 0 && lastRuntimeStatus != nil {
      shutdownRequested = false
      let alert = NSAlert()
      alert.messageText = "The Ditch Runtime could not be stopped"
      alert.informativeText = output.stderr.isEmpty
        ? "The runtime may still be running."
        : output.stderr
      alert.runModal()
      return
    }
    NSApp.terminate(nil)
  }

  private func statusImage() -> NSImage {
    let image = NSImage(size: NSSize(width: 18, height: 18))
    image.lockFocus()

    NSColor.black.setFill()

    // Use more of the standard 18-point status-item canvas so the mark has the
    // same optical weight as neighboring menu-bar icons.
    let scale = NSAffineTransform()
    scale.translateX(by: 9, yBy: 9)
    scale.scale(by: 1.09)
    scale.translateX(by: -9, yBy: -9)
    scale.concat()

    // A compact, clean rendering of The Ditch's chip-shaped D mark. Drawing it
    // here keeps the login-item helper self-contained and resolution independent.
    let mark = NSBezierPath()
    mark.windingRule = .evenOdd
    mark.appendRoundedRect(
      NSRect(x: 3.25, y: 3.25, width: 11.5, height: 11.5),
      xRadius: 2.4,
      yRadius: 2.4)

    // The counter has a straight left edge and a rounded right edge, forming D.
    mark.move(to: NSPoint(x: 6.4, y: 6.25))
    mark.line(to: NSPoint(x: 8.85, y: 6.25))
    mark.curve(
      to: NSPoint(x: 12.05, y: 9),
      controlPoint1: NSPoint(x: 10.8, y: 6.25),
      controlPoint2: NSPoint(x: 12.05, y: 7.35))
    mark.curve(
      to: NSPoint(x: 8.85, y: 11.75),
      controlPoint1: NSPoint(x: 12.05, y: 10.65),
      controlPoint2: NSPoint(x: 10.8, y: 11.75))
    mark.line(to: NSPoint(x: 6.4, y: 11.75))
    mark.close()
    mark.fill()

    let pins: [NSRect] = [
      NSRect(x: 5.1, y: 13.9, width: 1.55, height: 3.1),
      NSRect(x: 8.2, y: 13.9, width: 1.55, height: 3.1),
      NSRect(x: 11.35, y: 13.9, width: 1.55, height: 3.1),
      NSRect(x: 5.1, y: 1, width: 1.55, height: 3.1),
      NSRect(x: 8.2, y: 1, width: 1.55, height: 3.1),
      NSRect(x: 11.35, y: 1, width: 1.55, height: 3.1),
      NSRect(x: 1, y: 5.1, width: 3.1, height: 1.55),
      NSRect(x: 1, y: 8.2, width: 3.1, height: 1.55),
      NSRect(x: 1, y: 11.35, width: 3.1, height: 1.55),
      NSRect(x: 13.9, y: 5.1, width: 3.1, height: 1.55),
      NSRect(x: 13.9, y: 8.2, width: 3.1, height: 1.55),
      NSRect(x: 13.9, y: 11.35, width: 3.1, height: 1.55),
    ]
    for pin in pins {
      NSBezierPath(roundedRect: pin, xRadius: 0.75, yRadius: 0.75).fill()
    }

    image.unlockFocus()
    image.isTemplate = true
    image.accessibilityDescription = "The Ditch Runtime"
    return image
  }
}

private struct RuntimeStatus {
  let pid: Int32
  let activeSessionCount: Int
  let attentionCount: Int
  let instanceId: String
  let codexHome: String?
}

private struct CommandOutput {
  let exitCode: Int32
  let stdout: String
  let stderr: String
}

private struct AgentAttention {
  let id: String
  let kind: String
  let projectId: String
  let agentId: String
  let title: String
  let body: String
}
