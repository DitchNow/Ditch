import Foundation
import PeekabooAutomationKit
import PeekabooFoundation

/// Only fixed categories cross the ABI. Upstream messages/context can contain
/// captured text, file paths, typed values, or account information.
struct AutomationDiagnostic: Encodable {
    let ok = false
    let error: String
    let message: String
    let captureStage: String?
    let captureReason: String?

    init(_ failure: any Error) {
        var stage: String?
        var reason: String?
        let code: String
        if let action = failure as? DesktopActionFailure {
            code = action.outcome.dispatchState.mutationDispatched ? "outcome_unknown" : "actionRefused"
        } else if let native = failure as? AutomationFailure {
            code = native.rawValue
        } else if failure is DecodingError {
            code = "invalidArguments"
        } else if failure is CancellationError {
            code = "cancelled"
        } else if let ownership = failure as? ScreenCaptureKitOwnershipDiagnostic {
            code = "captureCoordinationFailed"
            stage = ownership.stage.rawValue
            reason = ownership.kind.rawValue
        } else if let observation = failure as? DesktopObservationError {
            switch observation {
            case .unsupportedTarget: code = "unsupportedTarget"
            case .targetNotFound: code = "targetNotFound"
            case .targetChanged: code = "staleSnapshot"
            case .ambiguousWindowTitle: code = "ambiguousTarget"
            }
        } else if let capture = failure as? CaptureError {
            switch capture {
            case .screenRecordingPermissionDenied, .permissionDeniedScreenRecording,
                 .accessibilityPermissionDenied, .appleScriptPermissionDenied:
                code = "permissionRequired"
            case .windowNotFound, .windowTitleNotFound, .appNotFound, .noWindowsFound,
                 .noFrontmostApplication: code = "targetNotFound"
            case .detectionTimedOut: code = "observationTimedOut"
            case .fileWriteError, .fileIOError: code = "captureStorageFailed"
            default: code = "captureFailed"
            }
        } else if let standard = failure as? any StandardizedError {
            switch standard.code {
            case .screenRecordingPermissionDenied, .accessibilityPermissionDenied,
                 .eventSynthesizingPermissionDenied: code = "permissionRequired"
            case .applicationNotFound, .windowNotFound, .elementNotFound, .menuNotFound:
                code = "targetNotFound"
            case .snapshotNotFound, .snapshotStale, .sessionNotFound: code = "staleSnapshot"
            case .captureFailed: code = "captureFailed"
            case .accessibilityIncomplete: code = "accessibilityIncomplete"
            case .timeout: code = "observationTimedOut"
            case .cancelled: code = "cancelled"
            case .fileIOError: code = "captureStorageFailed"
            default: code = "operationFailed"
            }
        } else {
            code = "operationFailed"
        }
        self.error = code
        self.captureStage = stage
        self.captureReason = reason
        self.message = switch code {
        case "protectedTarget": "Ditch blocks this target. This is not a missing macOS permission."
        case "permissionRequired": "The native operation requires a macOS permission. Check Ditch computer-use status."
        case "invalidArguments": "Invalid desktop arguments. Discover applications and windows with desktop_list_targets, then use a returned current-turn target_id. Legacy pid/window_id arguments are not supported. Follow the tool schema for field types."
        case "unsupportedTarget": "This target is not supported by the desktop adapter."
        case "targetNotFound", "ambiguousTarget": "List targets again and select an exact returned target."
        case "staleSnapshot": "No valid snapshot is available. Observe the target before requesting another action."
        case "captureCoordinationFailed": "Capture coordination failed. No new observation is available."
        case "captureFailed": "Screen capture failed. No new observation is available."
        case "captureStorageFailed": "The native host could not store the observation."
        case "accessibilityIncomplete": "The target did not return a usable Accessibility tree. This is not an app-level automation block. Use mode=visual and image coordinates."
        case "applicationGone": "The target process is no longer available. Discover the requested application again. This is not an automation policy block."
        case "processIdentityUnavailable": "The running application's process identity could not be verified. Rediscover once; if this persists, report processIdentityUnavailable. This is not a permission denial or an app-level automation block."
        case "targetNotDiscovered", "windowGone", "targetChanged": "Discover the application and windows again. Only current-turn target_id references are valid."
        case "targetNotFocused": "Focus the selected window with desktop_act(action=focus,target_id=...) then observe it again."
        case "targetObscured": "Another window covers this point. Focus the target, observe again, and use a visible point."
        case "windowRequired": "List windows using the application target_id and select a returned window target_id."
        case "discoveryLimit": "Target discovery limit reached for this turn. Stop retrying."
        case "observationTimedOut": "The native operation timed out. Its result is not verified."
        case "outcome_unknown": "Input may have been dispatched. Observe the target; do not automatically repeat the action."
        case "actionRefused": "Native input was refused. No successful action is confirmed."
        case "cancelled": "The operation was cancelled."
        default: "The native operation failed. Do not claim that it succeeded."
        }
    }
}
