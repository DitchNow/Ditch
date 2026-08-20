import Cocoa
import FlutterMacOS
import XCTest

@testable import The_Ditch

class RunnerTests: XCTestCase {

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

}
