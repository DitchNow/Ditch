import AppKit
import XCTest
import PeekabooAutomationKit
@testable import DitchAutomation

final class AutomationVisualInputTests: XCTestCase {
    @MainActor func testAccessibilityEvidenceSurvivesPublicationWithoutScreenshot() async throws {
        let manager = InMemorySnapshotManager()
        let result = ElementDetectionResult(snapshotId: "transient-inspection", screenshotPath: "",
            elements: .init(), metadata: .init(detectionTime: 0, elementCount: 0, method: "accessibility"))
        let id = try await PeekabooAdapter.publishAccessibility(result, manager: manager)
        XCTAssertNotEqual(id, result.snapshotId)
        let stored = try await manager.getDetectionResult(snapshotId: id)
        XCTAssertEqual(stored?.snapshotId, id)
        XCTAssertEqual(stored?.screenshotPath, "")
        try await manager.cleanSnapshot(snapshotId: id)
        let removed = try await manager.getDetectionResult(snapshotId: id)
        XCTAssertNil(removed)
    }
    @MainActor func testImageCoordinatesScaleToNegativeDisplayOrigin() throws {
        let bounds = CGRect(x: -1440, y: -200, width: 1440, height: 900)
        XCTAssertEqual(try PeekabooAdapter.imagePoint(x: 640, y: 400, size: CGSize(width: 1280, height: 800), bounds: bounds),
                       CGPoint(x: -720, y: 250))
        for x in [-1.0, 1280, .infinity, .nan] {
            XCTAssertThrowsError(try PeekabooAdapter.imagePoint(x: x, y: 0, size: CGSize(width: 1280, height: 800), bounds: bounds))
        }
    }

    @MainActor func testHostWindowsAreTargetsAndDoNotBypassOcclusion() throws {
        let bounds = CGRect(x: 0, y: 0, width: 200, height: 200)
        let target = AutomationTargetRegistry.Target(session: "turn", pid: getpid(), processStart: 123,
            bundle: "ai.theditch.app.staging", window: 7, bounds: bounds)
        func window(_ id: Int) -> [String: Any] {
            [kCGWindowNumber as String: id, kCGWindowOwnerPID as String: getpid(),
             kCGWindowBounds as String: bounds.dictionaryRepresentation, kCGWindowAlpha as String: 1]
        }
        XCTAssertNoThrow(try AutomationTargetRegistry.requireVisible(target, at: CGPoint(x: 50, y: 50), windows: [window(7)]))
        XCTAssertThrowsError(try AutomationTargetRegistry.requireVisible(target, at: CGPoint(x: 50, y: 50), windows: [window(8), window(7)]))
        XCTAssertThrowsError(try AutomationTargetRegistry.requireVisible(target, at: CGPoint(x: 250, y: 50), windows: [window(7)]))
    }

    @MainActor func testTargetReferencesCannotCrossTurns() throws {
        let registry = AutomationTargetRegistry()
        let target = try registry.register(.init(session: "one", pid: 42, processStart: 123,
            bundle: "ai.theditch.app.staging", window: nil, bounds: nil))
        XCTAssertThrowsError(try registry.resolve(target, session: "two", requiredBundle: nil))
        registry.clear(session: "one")
        XCTAssertThrowsError(try registry.resolve(target, session: "one", requiredBundle: nil))
        XCTAssertThrowsError(try registry.resolve("invented", session: "one", requiredBundle: nil))
    }

    @MainActor func testDisabledPointerFeedbackCreatesNoOverlay() {
        let visualizer = AutomationVisualizer()
        visualizer.pointerFeedback(enabled: false, session: "turn", action: "click", point: CGPoint(x: 20, y: 20))
        XCTAssertFalse(visualizer.isVisible)
        XCTAssertTrue(AutomationVisualizer.overlayWindowIDs.isEmpty)
    }
}
