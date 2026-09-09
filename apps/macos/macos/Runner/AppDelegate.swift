import Cocoa
import Darwin
import FlutterMacOS
import ServiceManagement
import Sparkle
import UserNotifications

/// Native shell for the foreground control surface.
///
/// Runtime supervision deliberately belongs to the independent integrated
/// `ditchd` menu-bar process.
/// Terminating this process must never stop The Ditch Runtime or its agents.
@main
class AppDelegate: FlutterAppDelegate, SPUUpdaterDelegate {
  struct DeploymentConfiguration {
    let environment: String
    let edition: String
    let relayOrigin: String
    let allowedUpdateHosts: Set<String>
    let buildIdentifier: String
    let buildNumber: String
    let releaseSequence: Int
    let communityRevision: String

    static func load() -> DeploymentConfiguration {
      let production = DeploymentConfiguration(
        environment: "production",
        edition: "community",
        relayOrigin: "https://relay.ditchnow.nl",
        allowedUpdateHosts: ["relay.ditchnow.nl"],
        buildIdentifier: "",
        buildNumber: "",
        releaseSequence: 0,
        communityRevision: "")
      let invalid = DeploymentConfiguration(
        environment: "invalid",
        edition: "invalid",
        relayOrigin: "",
        allowedUpdateHosts: [],
        buildIdentifier: "",
        buildNumber: "",
        releaseSequence: 0,
        communityRevision: "")
      guard let url = Bundle.main.url(forResource: "DitchEnvironment", withExtension: "plist"),
        let data = try? Data(contentsOf: url),
        let propertyList = try? PropertyListSerialization.propertyList(
          from: data,
          options: [],
          format: nil),
        let values = propertyList as? [String: Any],
        let environment = values["DeploymentEnvironment"] as? String,
        let edition = values["Edition"] as? String,
        let relayOrigin = values["RelayOrigin"] as? String,
        let rawHosts = values["AllowedUpdateHosts"] as? String,
        let buildIdentifier = values["BuildIdentifier"] as? String,
        let buildNumber = values["BuildNumber"] as? String,
        let releaseSequence = values["ReleaseSequence"] as? NSNumber,
        let communityRevision = values["CommunityRevision"] as? String
      else { return invalid }

      let hosts = Set(
        rawHosts.split(separator: ",")
          .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
          .filter { !$0.isEmpty })
      guard ["staging", "production"].contains(environment),
        ["community", "commercial"].contains(edition),
        let origin = URL(string: relayOrigin),
        origin.scheme == "https",
        let relayHost = origin.host?.lowercased(),
        hosts.contains(relayHost),
        environment != "production" || relayOrigin == production.relayOrigin,
        environment != "staging" || relayOrigin != production.relayOrigin,
        !buildIdentifier.isEmpty,
        !buildNumber.isEmpty,
        releaseSequence.intValue >= 0,
        !communityRevision.isEmpty
      else { return invalid }

      return DeploymentConfiguration(
        environment: environment,
        edition: edition,
        relayOrigin: relayOrigin,
        allowedUpdateHosts: hosts,
        buildIdentifier: buildIdentifier,
        buildNumber: buildNumber,
        releaseSequence: releaseSequence.intValue,
        communityRevision: communityRevision)
    }
  }

  struct RunningRuntimeStatus {
    let activeSessionCount: Int
    let edition: String
    let deploymentEnvironment: String
    let buildIdentifier: String
    let buildNumber: String
    let releaseSequence: Int
    let communityRevision: String?

    func matches(_ configuration: DeploymentConfiguration) -> Bool {
      edition == configuration.edition
        && deploymentEnvironment == configuration.environment
        && buildIdentifier == configuration.buildIdentifier
        && buildNumber == configuration.buildNumber
        && releaseSequence == configuration.releaseSequence
        && communityRevision == configuration.communityRevision
    }
  }

  static func validatedCommercialUpdateExpiry(_ rawExpiry: String, now: Date = Date()) -> Date? {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    // Relay emits milliseconds; also accept whole-second RFC 3339 timestamps.
    let expiry = formatter.date(from: rawExpiry)
      ?? ISO8601DateFormatter().date(from: rawExpiry)
    guard let expiry, expiry > now, expiry <= now.addingTimeInterval(24 * 60 * 60)
    else { return nil }
    return expiry
  }

  private struct AuthorizedUpdateContext {
    let releaseID: String
    let appcastURL: URL
    let artifactURL: URL
    let artifactSize: UInt64
    let version: String
    let build: String
    let channel: String
    let bearer: String
    let expiresAt: Date

    func matches(_ item: SUAppcastItem) -> Bool {
      item.fileURL == artifactURL
        && item.contentLength == artifactSize
        && item.displayVersionString == version
        && item.versionString == build
        && (item.channel ?? "stable") == channel
    }
  }

  private static let runtimeLoginItemIdentifier = "ai.theditch.runtime"
  private static let obsoleteLoginItemIdentifier = "ai.theditch.status"
  private static let processTimeoutExitCode: Int32 = 124
  private static let notificationControlSocketName = "notification-control.sock"
  private static let runtimeDeploymentEnvironmentDefaultsKey =
    "runtimeDeploymentEnvironment"
  private static let highestAcceptedReleaseSequenceDefaultsKey =
    "highestAcceptedReleaseSequence"

  private var applicationChannel: FlutterMethodChannel?
  private var authorizedUpdateContext: AuthorizedUpdateContext?
  private var communityUpdateRequested = false
  private static let communityUpdateFeedURL = URL(
    string: "https://updates.ditchnow.nl/community/appcast.xml")!
  private lazy var deploymentConfiguration = DeploymentConfiguration.load()
  private lazy var updaterController = SPUStandardUpdaterController(
    startingUpdater: hasUpdateVerificationKey,
    updaterDelegate: self,
    userDriverDelegate: nil)
  private var pendingAgentNavigation: [String: String]?
  private var uninstallInProgress = false

  private var hasUpdateVerificationKey: Bool {
    (Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") as? String)?
      .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false
  }

  override func applicationDidFinishLaunching(_ notification: Notification) {
    _ = updaterController
    NSLog(
      "Ditch \(deploymentConfiguration.edition) \(deploymentConfiguration.buildIdentifier) in \(deploymentConfiguration.environment); Relay: \(deploymentConfiguration.relayOrigin)")
    guard acceptBundledReleaseSequence() else { return }
    applyThemeMode(UserDefaults.standard.string(forKey: "themeMode") ?? "system")
    persistRuntimeEnvironment()
    registerStatusHelper()
    configureUninstallMenuItem()
  }

  private func acceptBundledReleaseSequence() -> Bool {
    let sequence = deploymentConfiguration.releaseSequence
    // Zero identifies an ordinary local development build. Official staging
    // and production publication requires a positive global sequence.
    guard sequence > 0 else { return true }
    let defaults = UserDefaults.standard
    let highest = defaults.integer(forKey: Self.highestAcceptedReleaseSequenceDefaultsKey)
    guard sequence >= highest else {
      let alert = NSAlert()
      alert.messageText = "Ditch refused an older release"
      alert.informativeText =
        "This installation has already accepted release \(highest), but the opened app is release \(sequence). Install the latest official Ditch build."
      alert.alertStyle = .critical
      alert.addButton(withTitle: "Quit")
      alert.runModal()
      NSApp.terminate(nil)
      return false
    }
    if sequence > highest {
      defaults.set(sequence, forKey: Self.highestAcceptedReleaseSequenceDefaultsKey)
    }
    return true
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
      NSLog("Ditch could not persist CODEX_HOME: \(error)")
    }
  }

  private func registerStatusHelper() {
    let environment = deploymentConfiguration.environment
    guard environment == "staging" || environment == "production" else {
      NSLog("Ditch refused to register a runtime with invalid deployment configuration")
      return
    }
    let defaults = UserDefaults.standard
    let previousEnvironment = defaults.string(
      forKey: Self.runtimeDeploymentEnvironmentDefaultsKey)
    let runningRuntime = runningRuntimeStatus()
    let replacingEnvironment = previousEnvironment != nil && previousEnvironment != environment
    let replacingBuild = runningRuntime.map { !$0.matches(deploymentConfiguration) } ?? false
    let replacingRuntime = replacingEnvironment || replacingBuild
    if replacingRuntime {
      let activeSessions = runningRuntime?.activeSessionCount
      let socketExists = FileManager.default.fileExists(atPath: runtimeSocketURL.path)
      if let activeSessions, activeSessions > 0 {
        refuseRuntimeEnvironmentSwitch(
          "The running \(runningRuntime?.edition ?? previousEnvironment ?? "existing") runtime has \(activeSessions) active agent\(activeSessions == 1 ? "" : "s"). Let them finish or stop them explicitly before installing Ditch \(deploymentConfiguration.edition.capitalized) \(deploymentConfiguration.buildIdentifier).")
        return
      }
      if activeSessions == nil && socketExists && runtimeProcessAppearsAlive {
        refuseRuntimeEnvironmentSwitch(
          "Ditch could not verify whether the running runtime has active agents. Stop it explicitly before installing Ditch \(deploymentConfiguration.edition.capitalized) \(deploymentConfiguration.buildIdentifier).")
        return
      }
      _ = Self.runProcess(
        executable: bundledExecutable("ditch_cli"),
        arguments: ["runtime", "stop"],
        timeout: 5)
    }

    if #available(macOS 13.0, *) {
      // Clean up the superseded three-process architecture. Unregistering the
      // obsolete login item does not send a shutdown request to the runtime.
      let obsolete = SMAppService.loginItem(identifier: Self.obsoleteLoginItemIdentifier)
      if obsolete.status == .enabled {
        try? obsolete.unregister()
      }
      let service = SMAppService.loginItem(identifier: Self.runtimeLoginItemIdentifier)
      var shouldRegister = service.status != .enabled
      if replacingRuntime,
        service.status == .enabled || service.status == .requiresApproval
      {
        do {
          try service.unregister()
        } catch {
          NSLog("Ditch could not replace its runtime environment: \(error)")
          return
        }
        shouldRegister = true
      }
      guard shouldRegister else { return }
      do {
        try service.register()
        defaults.set(environment, forKey: Self.runtimeDeploymentEnvironmentDefaultsKey)
      } catch {
        NSLog("Ditch could not register its runtime: \(error)")
      }
    } else {
      if replacingRuntime {
        _ = SMLoginItemSetEnabled(Self.runtimeLoginItemIdentifier as CFString, false)
      }
      _ = SMLoginItemSetEnabled(Self.runtimeLoginItemIdentifier as CFString, true)
      defaults.set(environment, forKey: Self.runtimeDeploymentEnvironmentDefaultsKey)
    }
  }

  private var runtimeSocketURL: URL {
    FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch", isDirectory: true)
      .appendingPathComponent("ditchd.sock")
  }

  private var runtimeProcessAppearsAlive: Bool {
    let pidURL = FileManager.default.homeDirectoryForCurrentUser
      .appendingPathComponent("Library/Application Support/The Ditch", isDirectory: true)
      .appendingPathComponent("ditchd.pid")
    guard let rawPID = try? String(contentsOf: pidURL, encoding: .utf8),
      let pid = Int32(rawPID.trimmingCharacters(in: .whitespacesAndNewlines)),
      let application = NSRunningApplication(processIdentifier: pid)
    else { return false }
    return application.bundleIdentifier == Self.runtimeLoginItemIdentifier
  }

  private func runningRuntimeStatus() -> RunningRuntimeStatus? {
    let result = Self.runProcessCapturingOutput(
      executable: bundledExecutable("ditch_cli"),
      arguments: ["runtime", "status"],
      timeout: 5)
    guard result.status == 0,
      let value = try? JSONSerialization.jsonObject(with: result.output),
      let envelope = value as? [String: Any],
      let status = envelope["RuntimeStatus"] as? [String: Any],
      let count = status["active_session_count"] as? NSNumber,
      let edition = status["edition"] as? String,
      let deploymentEnvironment = status["deployment_environment"] as? String,
      let buildIdentifier = status["build_identifier"] as? String,
      let buildNumber = status["build_number"] as? String,
      let releaseSequence = status["release_sequence"] as? NSNumber
    else { return nil }
    return RunningRuntimeStatus(
      activeSessionCount: count.intValue,
      edition: edition,
      deploymentEnvironment: deploymentEnvironment,
      buildIdentifier: buildIdentifier,
      buildNumber: buildNumber,
      releaseSequence: releaseSequence.intValue,
      communityRevision: status["community_revision"] as? String)
  }

  private func refuseRuntimeEnvironmentSwitch(_ message: String) {
    NSLog("Ditch refused runtime environment switch: \(message)")
    let alert = NSAlert()
    alert.messageText = "Ditch cannot switch environments yet"
    alert.informativeText = message
    alert.alertStyle = .warning
    alert.addButton(withTitle: "Quit")
    alert.runModal()
    NSApp.terminate(nil)
  }

  private func configureUninstallMenuItem() {
    guard let applicationMenu = NSApp.mainMenu?.items.first?.submenu,
      !applicationMenu.items.contains(where: { $0.action == #selector(uninstallApplication(_:)) })
    else { return }

    let item = NSMenuItem(
      title: "Uninstall Ditch…",
      action: #selector(uninstallApplication(_:)),
      keyEquivalent: "")
    item.target = self
    applicationMenu.insertItem(item, at: max(0, applicationMenu.numberOfItems - 2))
  }

  @objc private func uninstallApplication(_ sender: Any?) {
    guard !uninstallInProgress else { return }

    let alert = NSAlert()
    alert.messageText = "Uninstall Ditch?"
    alert.informativeText =
      "This stops all running agents, removes Ditch's background service and app data, and moves the application to Trash. Your project folders will not be deleted."
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
    alert.messageText = "Ditch could not be completely uninstalled"
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
      case "appVersion":
        let version =
          Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        result([
          "version": version,
          "build": build,
        ])
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
      case "installCommercialUpdate":
        guard self.hasUpdateVerificationKey else {
          result(FlutterError(
            code: "update_verification_not_configured",
            message: "This app does not contain the public Sparkle verification key required for official updates.",
            details: nil))
          return
        }
        guard self.updaterController.updater.canCheckForUpdates,
          self.authorizedUpdateContext == nil,
          !self.communityUpdateRequested
        else {
          result(FlutterError(
            code: "update_already_in_progress",
            message: "A verified Commercial update is already in progress.",
            details: nil))
          return
        }
        guard let arguments = call.arguments as? [String: Any],
          let releaseID = arguments["release_id"] as? String,
          UUID(uuidString: releaseID)?.uuidString.lowercased() == releaseID,
          let rawURL = arguments["appcast_url"] as? String,
          let url = URL(string: rawURL),
          url.scheme == "https",
          url.user == nil,
          url.password == nil,
          url.port == nil,
          url.query == nil,
          url.fragment == nil,
          let host = url.host
        else {
          result(FlutterError(
            code: "invalid_update_feed",
            message: "The authorized Commercial update feed is not a valid HTTPS URL.",
            details: nil))
          return
        }
        guard let rawArtifactURL = arguments["artifact_url"] as? String,
          let artifactURL = URL(string: rawArtifactURL),
          artifactURL.scheme == "https",
          artifactURL.user == nil,
          artifactURL.password == nil,
          artifactURL.port == nil,
          artifactURL.query == nil,
          artifactURL.fragment == nil,
          let artifactHost = artifactURL.host,
          let artifactSize = arguments["artifact_size"] as? NSNumber,
          artifactSize.int64Value > 0,
          let version = arguments["version"] as? String,
          !version.isEmpty,
          let build = arguments["build"] as? String,
          !build.isEmpty,
          let channel = arguments["channel"] as? String,
          channel == (self.deploymentConfiguration.environment == "staging" ? "beta" : "stable"),
          let relayHost = URL(string: self.deploymentConfiguration.relayOrigin)?.host?.lowercased(),
          host.lowercased() == relayHost,
          artifactHost.lowercased() == relayHost,
          self.deploymentConfiguration.allowedUpdateHosts.contains(host.lowercased()),
          self.deploymentConfiguration.allowedUpdateHosts.contains(artifactHost.lowercased()),
          url.path == "/v1/commercial/releases/\(releaseID)/appcast",
          artifactURL.path.hasPrefix("/v1/commercial/releases/\(releaseID)/artifact/")
        else {
          result(FlutterError(
            code: "unauthorized_update_host",
            message: "The Commercial release does not match this Ditch environment.",
            details: nil))
          return
        }
        guard let bearer = arguments["authorization_bearer"] as? String,
            bearer.count >= 32,
            bearer.count <= 512,
            bearer.unicodeScalars.allSatisfy({
              CharacterSet.alphanumerics.contains($0) || "-_.".unicodeScalars.contains($0)
            }),
            let rawExpiry = arguments["expires_at"] as? String,
            let expiry = Self.validatedCommercialUpdateExpiry(rawExpiry)
        else {
          result(FlutterError(
            code: "invalid_update_session",
            message: "The Relay returned an invalid or expired Commercial update session.",
            details: nil))
          return
        }
        self.authorizedUpdateContext = AuthorizedUpdateContext(
          releaseID: releaseID.lowercased(),
          appcastURL: url,
          artifactURL: artifactURL,
          artifactSize: artifactSize.uint64Value,
          version: version,
          build: build,
          channel: channel,
          bearer: bearer,
          expiresAt: expiry)
        self.updaterController.updater.httpHeaders = [
          "Authorization": "Bearer \(bearer)"
        ]
        _ = self.updaterController.updater.clearFeedURLFromUserDefaults()
        self.updaterController.checkForUpdates(nil)
        result(true)
      case "checkCommunityUpdate":
        guard self.hasUpdateVerificationKey else {
          result(FlutterError(
            code: "update_verification_not_configured",
            message: "This app does not contain the public Sparkle verification key required for official updates.",
            details: nil))
          return
        }
        guard self.updaterController.updater.canCheckForUpdates,
          self.authorizedUpdateContext == nil,
          !self.communityUpdateRequested
        else {
          result(FlutterError(
            code: "update_already_in_progress",
            message: "An update check is already in progress.",
            details: nil))
          return
        }
        self.communityUpdateRequested = true
        self.updaterController.updater.httpHeaders = nil
        _ = self.updaterController.updater.clearFeedURLFromUserDefaults()
        self.updaterController.checkForUpdates(nil)
        result(true)
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

  func allowedChannels(for updater: SPUUpdater) -> Set<String> {
    deploymentConfiguration.environment == "staging" ? ["beta"] : []
  }

  func feedURLString(for updater: SPUUpdater) -> String? {
    if let context = authorizedUpdateContext {
      return context.appcastURL.absoluteString
    }
    return communityUpdateRequested
      ? Self.communityUpdateFeedURL.absoluteString
      : nil
  }

  func updater(
    _ updater: SPUUpdater,
    shouldProceedWithUpdate updateItem: SUAppcastItem,
    updateCheck: SPUUpdateCheck
  ) throws {
    if let context = authorizedUpdateContext {
      guard context.expiresAt > Date(), context.matches(updateItem) else {
        clearUpdateSession()
        throw NSError(
          domain: "ai.theditch.update-authorization",
          code: 1,
          userInfo: [
            NSLocalizedDescriptionKey:
              "The update feed did not match the release authorized by Ditch Relay."
          ])
      }
      updater.httpHeaders = nil
      return
    }
    guard communityUpdateRequested,
      updateItem.fileURL?.scheme == "https",
      updateItem.fileURL?.host?.lowercased()
        == Self.communityUpdateFeedURL.host?.lowercased()
    else {
      clearUpdateSession()
      throw NSError(
        domain: "ai.theditch.update-authorization",
        code: 2,
        userInfo: [
          NSLocalizedDescriptionKey:
            "The Community update did not come from the official Ditch update host."
        ])
    }
  }

  func updater(
    _ updater: SPUUpdater,
    shouldDownloadReleaseNotesForUpdate updateItem: SUAppcastItem
  ) -> Bool {
    false
  }

  func updater(
    _ updater: SPUUpdater,
    willDownloadUpdate item: SUAppcastItem,
    with request: NSMutableURLRequest
  ) {
    request.setValue(nil, forHTTPHeaderField: "Authorization")
    if communityUpdateRequested {
      guard request.url?.scheme == "https",
        request.url?.host?.lowercased()
          == Self.communityUpdateFeedURL.host?.lowercased()
      else {
        request.url = nil
        clearUpdateSession()
        return
      }
      return
    }
    guard let context = authorizedUpdateContext,
      context.expiresAt > Date(),
      context.matches(item),
      request.url == context.artifactURL
    else {
      request.url = nil
      clearUpdateSession()
      return
    }
    request.setValue("Bearer \(context.bearer)", forHTTPHeaderField: "Authorization")
  }

  func updater(
    _ updater: SPUUpdater,
    didFinishUpdateCycleFor updateCheck: SPUUpdateCheck,
    error: Error?
  ) {
    clearUpdateSession()
  }

  func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
    clearUpdateSession()
  }

  private func clearUpdateSession() {
    updaterController.updater.httpHeaders = nil
    authorizedUpdateContext = nil
    communityUpdateRequested = false
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
        message: "The notification response did not come from Ditch Runtime.",
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
      echo "Ditch is opening your selected Codex CLI:"
      echo \(quotedBinary)
      echo
      \(quotedBinary) login
      status=$?
      echo
      if [ $status -eq 0 ]; then
        echo "Codex sign-in completed. Return to Ditch."
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
      NSLog("Ditch could not open Codex sign-in: \(error)")
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

  private static func runProcessCapturingOutput(
    executable: String,
    arguments: [String],
    timeout: TimeInterval
  ) -> (status: Int32, output: Data) {
    guard FileManager.default.isExecutableFile(atPath: executable) else {
      return (127, Data())
    }
    let process = Process()
    let finished = DispatchSemaphore(value: 0)
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    process.terminationHandler = { _ in finished.signal() }
    do {
      try process.run()
    } catch {
      return (1, Data())
    }

    guard finished.wait(timeout: .now() + timeout) == .success else {
      if process.isRunning { process.terminate() }
      if finished.wait(timeout: .now() + 0.5) == .timedOut && process.isRunning {
        Darwin.kill(process.processIdentifier, SIGKILL)
        _ = finished.wait(timeout: .now() + 0.5)
      }
      return (processTimeoutExitCode, Data())
    }
    return (process.terminationStatus, output.fileHandleForReading.readDataToEndOfFile())
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
    if !opened { NSLog("Ditch could not start its runtime application") }
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
