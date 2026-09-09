import Cocoa
import FlutterMacOS
import XCTest

@testable import Ditch

class RunnerTests: XCTestCase {

  func testCommercialUpdateExpiryAcceptsRelayMillisecondsAndWholeSeconds() {
    let now = ISO8601DateFormatter().date(from: "2026-09-09T14:00:00Z")!
    for (timestamp, seconds) in [
      ("2026-09-09T14:30:00Z", 1800.0),
      ("2026-09-09T14:30:00.000Z", 1800.0),
      ("2026-09-09T14:30:00.123Z", 1800.123),
      ("2026-09-09T16:30:00.123+02:00", 1800.123),
    ] {
      let expiry = AppDelegate.validatedCommercialUpdateExpiry(timestamp, now: now)
      XCTAssertNotNil(expiry, timestamp)
      if let expiry {
        XCTAssertEqual(expiry.timeIntervalSince(now), seconds, accuracy: 0.001, timestamp)
      }
    }
  }

  func testCommercialUpdateExpiryRejectsMalformedAndExpiredPermissions() {
    let now = ISO8601DateFormatter().date(from: "2026-09-09T14:00:00Z")!
    for timestamp in [
      "", "not-a-date", "1788964200000",
      "2026-09-09T13:59:59Z", "2026-09-09T13:59:59.999Z",
      "2026-09-09T14:00:00Z", "2026-09-09T14:00:00.000Z",
    ] {
      XCTAssertNil(AppDelegate.validatedCommercialUpdateExpiry(timestamp, now: now), timestamp)
    }
  }

  func testCommercialUpdateExpiryPreservesThe24HourLimit() {
    let now = ISO8601DateFormatter().date(from: "2026-09-09T14:00:00Z")!
    XCTAssertNotNil(AppDelegate.validatedCommercialUpdateExpiry("2026-09-10T14:00:00Z", now: now))
    XCTAssertNotNil(AppDelegate.validatedCommercialUpdateExpiry("2026-09-10T14:00:00.000Z", now: now))
    XCTAssertNil(AppDelegate.validatedCommercialUpdateExpiry("2026-09-10T14:00:01Z", now: now))
    XCTAssertNil(AppDelegate.validatedCommercialUpdateExpiry("2026-09-10T14:00:00.001Z", now: now))
  }

  func testComposerConfigurationDisablesRichTextAndGraphics() {
    let textView = NSTextView()
    textView.isRichText = true
    textView.importsGraphics = true

    ComposerTextView.configureAsPlainText(textView)

    XCTAssertFalse(textView.isRichText)
    XCTAssertFalse(textView.importsGraphics)
  }

  func testComposerReadsOnlyThePlainStringClipboardRepresentation() {
    let pasteboard = NSPasteboard(
      name: NSPasteboard.Name("ai.theditch.tests.composer.\(UUID().uuidString)"))
    pasteboard.clearContents()
    pasteboard.setString("plain prompt", forType: .string)
    pasteboard.setData(Data("{\\rtf1\\b formatted}".utf8), forType: .rtf)

    XCTAssertEqual(ComposerTextView.plainText(from: pasteboard), "plain prompt")
  }

  func testComposerPlaceholderNeverInterceptsEditorClicks() {
    let placeholder = ComposerPlaceholderLabel(labelWithString: "Send a follow-up")
    placeholder.frame = NSRect(x: 0, y: 0, width: 300, height: 40)

    XCTAssertNil(placeholder.hitTest(NSPoint(x: 1, y: 20)))
    XCTAssertNil(placeholder.hitTest(NSPoint(x: 150, y: 20)))
    XCTAssertNil(placeholder.hitTest(NSPoint(x: 299, y: 20)))
  }

  func testComposerReportsAndCanReassertNativeFocusOwnership() {
    let window = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 320, height: 120),
      styleMask: .borderless,
      backing: .buffered,
      defer: false)
    let textView = ComposerTextView(frame: window.contentView?.bounds ?? .zero)
    window.contentView?.addSubview(textView)

    var focusClaimCount = 0
    var focusLostCount = 0
    textView.onFocusClaimed = { focusClaimCount += 1 }
    textView.onFocusLost = { focusLostCount += 1 }

    XCTAssertTrue(window.makeFirstResponder(textView))
    XCTAssertTrue(window.firstResponder === textView)
    XCTAssertEqual(focusClaimCount, 1)

    // A click on an already-focused native view must still repair Flutter's
    // independently tracked logical focus if another widget retained it.
    textView.reassertFocusClaim()
    XCTAssertEqual(focusClaimCount, 2)

    XCTAssertTrue(window.makeFirstResponder(nil))
    XCTAssertEqual(focusLostCount, 1)
  }

  func testUninstallArtifactsAreLimitedToDitchOwnedLibraryPaths() {
    let home = URL(fileURLWithPath: "/Users/tester", isDirectory: true)

    let paths = Set(
      AppDelegate.uninstallArtifactURLs(homeDirectory: home).map(\.path))

    XCTAssertTrue(paths.allSatisfy { $0.hasPrefix("/Users/tester/Library/") })
    XCTAssertTrue(paths.contains("/Users/tester/Library/Application Support/The Ditch"))
    XCTAssertTrue(paths.contains("/Users/tester/Library/Preferences/ai.theditch.app.plist"))
    XCTAssertTrue(paths.contains("/Users/tester/Library/Caches/ai.theditch.runtime"))
    XCTAssertTrue(paths.contains("/Users/tester/Library/Preferences/ai.theditch.status.plist"))
    XCTAssertFalse(paths.contains { $0.contains("/Documents/") })
  }

  func testRuntimeIdentityRequiresTheExactEditionEnvironmentAndBuild() {
    let expected = AppDelegate.DeploymentConfiguration(
      environment: "staging",
      edition: "commercial",
      relayOrigin: "https://staging.example.test",
      allowedUpdateHosts: ["staging.example.test"],
      buildIdentifier: "0.1.1-abcdef1234567890",
      buildNumber: "110",
      releaseSequence: 110,
      communityRevision: String(repeating: "a", count: 40))
    let matching = AppDelegate.RunningRuntimeStatus(
      activeSessionCount: 0,
      edition: "commercial",
      deploymentEnvironment: "staging",
      buildIdentifier: "0.1.1-abcdef1234567890",
      buildNumber: "110",
      releaseSequence: 110,
      communityRevision: String(repeating: "a", count: 40))

    XCTAssertTrue(matching.matches(expected))
    let installedHelper = URL(fileURLWithPath:
      "/Applications/Ditch.app/Contents/Library/LoginItems/The Ditch Runtime.app")
    let mountedHelper = URL(fileURLWithPath:
      "/Volumes/Old Ditch/Ditch.app/Contents/Library/LoginItems/The Ditch Runtime.app")
    XCTAssertTrue(matching.matches(expected, bundleURL: installedHelper, expectedBundleURL: installedHelper))
    XCTAssertFalse(matching.matches(expected, bundleURL: mountedHelper, expectedBundleURL: installedHelper))
    XCTAssertFalse(matching.matches(expected, bundleURL: nil, expectedBundleURL: installedHelper))

    XCTAssertFalse(
      AppDelegate.RunningRuntimeStatus(
        activeSessionCount: 0,
        edition: "commercial",
        deploymentEnvironment: "production",
        buildIdentifier: matching.buildIdentifier,
        buildNumber: matching.buildNumber,
        releaseSequence: matching.releaseSequence,
        communityRevision: matching.communityRevision
      ).matches(expected))
    XCTAssertFalse(
      AppDelegate.RunningRuntimeStatus(
        activeSessionCount: 0,
        edition: "community",
        deploymentEnvironment: matching.deploymentEnvironment,
        buildIdentifier: matching.buildIdentifier,
        buildNumber: matching.buildNumber,
        releaseSequence: matching.releaseSequence,
        communityRevision: matching.communityRevision
      ).matches(expected))
    XCTAssertFalse(
      AppDelegate.RunningRuntimeStatus(
        activeSessionCount: 0,
        edition: matching.edition,
        deploymentEnvironment: matching.deploymentEnvironment,
        buildIdentifier: matching.buildIdentifier,
        buildNumber: "111",
        releaseSequence: 111,
        communityRevision: matching.communityRevision
      ).matches(expected))
  }

}

// MARK: Runtime startup regression tests
final class RuntimeStartupFixture {
  typealias Observation = RuntimeStartupCoordinator.Observation
  typealias Status = RuntimeStartupCoordinator.Status
  var time: TimeInterval = 0
  var frames: [Observation] = [idle]
  var events: [String] = []
  var stopAccepted = true
  var onRegister: (() throws -> Void)?
  var onLaunch: (() -> Bool)?
  var launchTimes: [TimeInterval] = []

  static var idle: Observation {
    Observation(status: nil, processIDs: [], connectionsBusy: false)
  }
  static var ready: Observation {
    Observation(status: Status(pid: 200, matches: true, activeSessions: 0, build: "116"),
      processIDs: [200], connectionsBusy: true)
  }
  static func old(active: Int = 0) -> Observation {
    Observation(status: Status(pid: 100, matches: false, activeSessions: active, build: "115"),
      processIDs: [100], connectionsBusy: true)
  }

  func coordinator(timeout: TimeInterval = 4) -> RuntimeStartupCoordinator {
    RuntimeStartupCoordinator(
      inspect: {
        let observation = self.frames[0]
        if self.frames.count > 1 { self.frames.removeFirst() }
        return observation
      },
      stop: { self.events.append("stop"); return self.stopAccepted },
      unregister: { self.events.append("unregister") },
      register: { self.events.append("register"); try self.onRegister?() },
      launch: {
        self.events.append("launch")
        self.launchTimes.append(self.time)
        return self.onLaunch?() ?? false
      },
      now: { self.time },
      pause: { self.time += $0 },
      timeout: timeout,
      log: { _ in })
  }
}

final class RuntimeStartupTests: XCTestCase {
  func testNewBuildStartedByLoginServicesDuringHandoverIsAdopted() throws {
    let fixture = RuntimeStartupFixture()
    fixture.frames = [RuntimeStartupFixture.old(), RuntimeStartupFixture.ready]
    try fixture.coordinator().run()
    XCTAssertEqual(fixture.events, ["stop", "unregister", "register"])
  }

  func testHandoverWaitsForBothProcessExitAndConnectionRelease() throws {
    let fixture = RuntimeStartupFixture()
    fixture.frames = [
      .init(status: RuntimeStartupFixture.old().status, processIDs: [100], connectionsBusy: true),
      RuntimeStartupFixture.old(),
      .init(status: nil, processIDs: [100], connectionsBusy: false),
      .init(status: nil, processIDs: [], connectionsBusy: true),
      RuntimeStartupFixture.idle,
    ]
    fixture.onLaunch = { fixture.frames = [RuntimeStartupFixture.ready]; return true }
    try fixture.coordinator().run()
    XCTAssertEqual(fixture.events, ["stop", "unregister", "register", "launch"])
    XCTAssertGreaterThanOrEqual(fixture.launchTimes[0], 1.6)
  }

  func testShutdownTimeoutDoesNotLaunchOrRegisterReplacement() {
    let fixture = RuntimeStartupFixture()
    fixture.frames = [RuntimeStartupFixture.old()]
    XCTAssertThrowsError(try fixture.coordinator().run()) {
      XCTAssertEqual($0 as? RuntimeStartupCoordinator.Failure, .shutdownTimedOut)
    }
    XCTAssertEqual(fixture.events, ["stop", "unregister"])
  }

  func testActiveAgentsPreventRuntimeReplacement() {
    let fixture = RuntimeStartupFixture()
    fixture.frames = [RuntimeStartupFixture.old(active: 1)]
    XCTAssertThrowsError(try fixture.coordinator().run()) {
      XCTAssertEqual($0 as? RuntimeStartupCoordinator.Failure, .activeSessions)
    }
    XCTAssertTrue(fixture.events.isEmpty)
  }

  func testFailedShutdownDoesNotUnregisterOrLaunch() {
    let fixture = RuntimeStartupFixture()
    fixture.frames = [RuntimeStartupFixture.old()]
    fixture.stopAccepted = false
    XCTAssertThrowsError(try fixture.coordinator().run()) {
      XCTAssertEqual($0 as? RuntimeStartupCoordinator.Failure, .stopFailed)
    }
    XCTAssertEqual(fixture.events, ["stop"])
  }

  func testUnresponsiveRuntimeIsLeftRunningToProtectAgents() {
    let fixture = RuntimeStartupFixture()
    fixture.frames = [.init(status: nil, processIDs: [100], connectionsBusy: true)]
    XCTAssertThrowsError(try fixture.coordinator().run()) {
      XCTAssertEqual($0 as? RuntimeStartupCoordinator.Failure, .unverifiedRuntime)
    }
    XCTAssertTrue(fixture.events.isEmpty)
  }

  func testMatchingRuntimeIsReusedWithoutStoppingOrLaunching() throws {
    let fixture = RuntimeStartupFixture()
    fixture.frames = [RuntimeStartupFixture.ready]
    try fixture.coordinator().run()
    XCTAssertEqual(fixture.events, ["register"])
  }

  func testRegistrationLaunchIsObservedBeforeExplicitLaunch() throws {
    let fixture = RuntimeStartupFixture()
    fixture.onRegister = { fixture.frames = [RuntimeStartupFixture.ready] }
    try fixture.coordinator().run()
    XCTAssertEqual(fixture.events, ["unregister", "register"])
  }

  func testLaunchCollisionIsRetriedAfterConnectionClears() throws {
    let fixture = RuntimeStartupFixture()
    var launches = 0
    fixture.onLaunch = {
      launches += 1
      if launches == 1 {
        fixture.frames = [
          .init(status: nil, processIDs: [], connectionsBusy: true),
          .init(status: nil, processIDs: [], connectionsBusy: true),
          RuntimeStartupFixture.idle,
        ]
      } else {
        fixture.frames = [RuntimeStartupFixture.ready]
      }
      return true
    }
    try fixture.coordinator().run()
    XCTAssertEqual(launches, 2)
    XCTAssertGreaterThanOrEqual(fixture.launchTimes[1] - fixture.launchTimes[0], 1)
  }

  func testLaunchSuccessAloneDoesNotCountAsReadiness() {
    let fixture = RuntimeStartupFixture()
    fixture.onLaunch = { true }
    XCTAssertThrowsError(try fixture.coordinator().run()) {
      XCTAssertEqual($0 as? RuntimeStartupCoordinator.Failure, .startupTimedOut)
    }
    XCTAssertGreaterThan(fixture.launchTimes.count, 1)
    XCTAssertLessThanOrEqual(fixture.time, 4.3)
  }

  func testRegistrationFailureDoesNotLaunch() {
    let fixture = RuntimeStartupFixture()
    fixture.onRegister = { throw RuntimeStartupCoordinator.Failure.registration }
    XCTAssertThrowsError(try fixture.coordinator().run()) {
      XCTAssertEqual($0 as? RuntimeStartupCoordinator.Failure, .registration)
    }
    XCTAssertEqual(fixture.events, ["unregister", "register"])
  }

  func testConcurrentStartupRequestsShareOneHandover() {
    let inspected = DispatchSemaphore(value: 0)
    let release = DispatchSemaphore(value: 0)
    let completed = expectation(description: "both callers finish")
    completed.expectedFulfillmentCount = 2
    var inspections = 0
    var registrations = 0
    let coordinator = RuntimeStartupCoordinator(
      inspect: {
        inspections += 1
        if inspections == 1 {
          inspected.signal()
          _ = release.wait(timeout: .now() + 2)
        }
        return RuntimeStartupFixture.ready
      },
      stop: { XCTFail("must not stop matching runtime"); return false },
      unregister: { XCTFail("must not unregister matching runtime") },
      register: { registrations += 1 },
      launch: { XCTFail("must not launch another runtime"); return false },
      pause: { _ in }, log: { _ in })
    coordinator.start { result in
      if case .failure(let error) = result { XCTFail(error.localizedDescription) }
      completed.fulfill()
    }
    XCTAssertEqual(inspected.wait(timeout: .now() + 2), .success)
    coordinator.start { result in
      if case .failure(let error) = result { XCTFail(error.localizedDescription) }
      completed.fulfill()
    }
    release.signal()
    wait(for: [completed], timeout: 3)
    XCTAssertEqual(registrations, 1)
  }

  func testRetryAfterFailureStartsANewAttempt() {
    let fixture = RuntimeStartupFixture()
    let coordinator = fixture.coordinator()
    let failed = expectation(description: "first attempt times out")
    coordinator.start { result in
      if case .success = result { XCTFail("must verify readiness") }
      failed.fulfill()
    }
    wait(for: [failed], timeout: 2)
    fixture.onLaunch = { fixture.frames = [RuntimeStartupFixture.ready]; return true }
    let recovered = expectation(description: "retry succeeds")
    coordinator.start { result in
      if case .failure(let error) = result { XCTFail(error.localizedDescription) }
      recovered.fulfill()
    }
    wait(for: [recovered], timeout: 2)
    XCTAssertEqual(fixture.events.filter { $0 == "register" }.count, 2)
  }
}
