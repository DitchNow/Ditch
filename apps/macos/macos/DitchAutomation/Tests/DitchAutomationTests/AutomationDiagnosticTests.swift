import Foundation
import PeekabooAutomationKit
import PeekabooFoundation
import XCTest
@testable import DitchAutomation

final class AutomationDiagnosticTests: XCTestCase {
    func testMalformedLegacyArgumentsAreDiagnosedWithoutEchoingInput() throws {
        let data = Data(#"{"sessionId":"turn","operationId":"op","tool":"desktop_observe","arguments":{"pid":"SECRET-80207","window_id":"1"}}"#.utf8)
        XCTAssertThrowsError(try JSONDecoder().decode(NativeRequest.self, from: data)) {
            let diagnostic = AutomationDiagnostic($0)
            XCTAssertEqual(diagnostic.error, "invalidArguments")
            XCTAssertTrue(diagnostic.message.contains("target_id"))
            XCTAssertFalse(diagnostic.message.contains("SECRET"))
        }
        XCTAssertEqual(AutomationDiagnostic(AutomationFailure.processIdentityUnavailable).error, "processIdentityUnavailable")
        XCTAssertNotEqual(AutomationDiagnostic(AutomationFailure.applicationGone).message,
                          AutomationDiagnostic(AutomationFailure.protectedTarget).message)
    }
    func testFailureCategoriesDoNotConflatePermissionsTargetsAndCapture() {
        XCTAssertEqual(AutomationDiagnostic(AutomationFailure.protectedTarget).error, "protectedTarget")
        XCTAssertEqual(AutomationDiagnostic(PeekabooError.permissionDeniedAccessibility).error, "permissionRequired")
        XCTAssertEqual(AutomationDiagnostic(DesktopObservationError.targetNotFound("private target")).error, "targetNotFound")
        XCTAssertEqual(AutomationDiagnostic(DesktopObservationError.targetChanged("private target")).error, "staleSnapshot")
        XCTAssertEqual(AutomationDiagnostic(CaptureError.captureFailed("private capture")).error, "captureFailed")
        XCTAssertEqual(AutomationDiagnostic(PeekabooError.accessibilityIncomplete("private content")).error, "accessibilityIncomplete")
        XCTAssertEqual(AutomationDiagnostic(CaptureError.detectionTimedOut(3)).error, "observationTimedOut")
        XCTAssertEqual(AutomationDiagnostic(CancellationError()).error, "cancelled")
    }

    func testDiagnosticNeverSerializesObservedTextPathsOrUnderlyingMessages() throws {
        let marker = "SECRET-CAPTURED-CONTENT"
        let errors: [any Error] = [
            DesktopObservationError.ambiguousWindowTitle(marker, candidates: marker),
            CaptureError.fileWriteError(marker, NSError(domain: marker, code: 7)),
            PeekabooError.captureFailed(marker),
            NSError(domain: marker, code: 8, userInfo: [NSLocalizedDescriptionKey: marker]),
            ScreenCaptureKitOwnershipDiagnostic(kind: .ownedByAnotherProcess, stage: .admission,
                message: marker, path: marker),
        ]
        for error in errors {
            let bytes = try JSONEncoder().encode(AutomationDiagnostic(error))
            let text = String(decoding: bytes, as: UTF8.self)
            XCTAssertFalse(text.contains(marker))
            let value = try XCTUnwrap(JSONSerialization.jsonObject(with: bytes) as? [String: Any])
            XCTAssertEqual(value["ok"] as? Bool, false)
            XCTAssertNotNil(value["message"] as? String)
        }
    }

    func testCaptureCoordinationRetainsOnlyTypedStageAndReason() {
        let result = AutomationDiagnostic(ScreenCaptureKitOwnershipDiagnostic(
            kind: .uncoordinatedHosts, stage: .preparation, message: "private diagnostic"))
        XCTAssertEqual(result.error, "captureCoordinationFailed")
        XCTAssertEqual(result.captureStage, "preparation")
        XCTAssertEqual(result.captureReason, "uncoordinatedHosts")
    }
}
