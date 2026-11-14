import AppKit
import XCTest
@testable import DitchAutomation

final class AutomationVisualizerTests: XCTestCase {
    @MainActor func testOverlayCannotStealInput() {
        _ = NSApplication.shared
        let window = AutomationOverlayWindow(frame: CGRect(x: 0, y: 0, width: 200, height: 100))
        XCTAssertFalse(window.canBecomeKey)
        XCTAssertFalse(window.canBecomeMain)
        XCTAssertTrue(window.ignoresMouseEvents)
        XCTAssertEqual(window.sharingType, .none)
        XCTAssertFalse(window.isVisible)
        window.close()
    }

    @MainActor func testCoordinatesPreserveDisplaysAboveAndLeftOfPrimary() {
        let rect = CGRect(x: -1200, y: -800, width: 900, height: 600)
        XCTAssertEqual(AutomationVisualizer.appKitRect(rect, primaryTop: 1080),
                       CGRect(x: -1200, y: 1280, width: 900, height: 600))
    }

    @MainActor func testCoveredMovedAndWrongProcessTargetsNeverShow() {
        let bounds = CGRect(x: 100, y: 100, width: 400, height: 300)
        func window(_ id: Int, _ pid: Int, _ rect: CGRect) -> [String: Any] {
            [kCGWindowNumber as String: id, kCGWindowOwnerPID as String: pid,
             kCGWindowBounds as String: rect.dictionaryRepresentation, kCGWindowAlpha as String: 1.0]
        }
        let target = window(7, 42, bounds)
        XCTAssertTrue(AutomationVisualizer.unobscuredTarget(7, pid: 42, bounds: bounds, windows: [target]))
        XCTAssertFalse(AutomationVisualizer.unobscuredTarget(7, pid: 43, bounds: bounds, windows: [target]))
        XCTAssertFalse(AutomationVisualizer.unobscuredTarget(7, pid: 42, bounds: bounds.offsetBy(dx: 5, dy: 0), windows: [target]))
        XCTAssertFalse(AutomationVisualizer.unobscuredTarget(7, pid: 42, bounds: bounds,
                        windows: [window(8, 43, bounds.insetBy(dx: 20, dy: 20)), target]))
        XCTAssertFalse(AutomationVisualizer.unobscuredTarget(7, pid: 42, bounds: bounds, windows: []))
    }

    @MainActor func testDisabledFeedbackAndRepeatedCleanupCreateNoWindows() {
        let visualizer = AutomationVisualizer()
        visualizer.show(enabled: false, session: "turn", action: "click", windowID: 1, pid: 1,
                        bounds: CGRect(x: 0, y: 0, width: 200, height: 200))
        XCTAssertFalse(visualizer.isVisible)
        visualizer.clear(session: "other")
        visualizer.clear()
        visualizer.clear()
        XCTAssertFalse(visualizer.isVisible)
    }
}
