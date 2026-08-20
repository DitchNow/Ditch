import Cocoa
import FlutterMacOS
import XCTest

@testable import The_Ditch

class RunnerTests: XCTestCase {

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
