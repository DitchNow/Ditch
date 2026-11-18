import AppKit
import XCTest
@testable import DitchAutomation

final class AutomationTargetPolicyTests: XCTestCase {
    @MainActor func testVerificationScopeDoesNotAdmitOtherApps() {
        XCTAssertNoThrow(try PeekabooAdapter.validateApplication(bundleIdentifier: "company.thebrowser.Browser",
                                                                requiredBundle: "company.thebrowser.Browser"))
        for bundle in ["com.apple.Terminal", "ai.theditch.app", "ai.theditch.app.staging", ""] {
            XCTAssertThrowsError(try PeekabooAdapter.validateApplication(bundleIdentifier: bundle,
                                                                        requiredBundle: "company.thebrowser.Browser")) {
                XCTAssertEqual($0 as? AutomationFailure, .protectedTarget)
            }
        }
    }
    @MainActor func testDitchEditionsAreOrdinaryTargets() {
        for bundle in ["ai.theditch.app", "ai.theditch.app.staging", "ai.theditch.runtime", "ai.theditch.runtime.staging", "com.apple.TextEdit", ""] {
            XCTAssertNoThrow(try PeekabooAdapter.validateApplication(bundleIdentifier: bundle))
        }
    }

    @MainActor func testAutomationHostIsNotRejectedForItsOwnProcessIdentity() throws {
        let pid = ProcessInfo.processInfo.processIdentifier
        guard NSRunningApplication(processIdentifier: pid) != nil else {
            throw XCTSkip("Requires a test host registered with the macOS application server")
        }
        XCTAssertNoThrow(try PeekabooAdapter.validateTarget(pid))
    }

    @MainActor func testSystemPermissionAndAuthenticationAppsRemainProtected() {
        for bundle in ["com.apple.systempreferences", "com.apple.SecurityAgent", "com.apple.loginwindow"] {
            XCTAssertThrowsError(try PeekabooAdapter.validateApplication(bundleIdentifier: bundle)) {
                XCTAssertEqual($0 as? AutomationFailure, .protectedTarget)
            }
        }
    }

    @MainActor func testInvalidProcessIsStillRejected() {
        for pid in [Int32(0), Int32(-1)] {
            XCTAssertThrowsError(try PeekabooAdapter.validateTarget(pid)) {
                XCTAssertEqual($0 as? AutomationFailure, .staleSnapshot)
            }
        }
    }
}
