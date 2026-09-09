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
