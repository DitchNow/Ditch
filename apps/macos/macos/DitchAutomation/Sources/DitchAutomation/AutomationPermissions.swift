@preconcurrency import ApplicationServices
import CoreGraphics
import Foundation
import PeekabooAutomationKit

/// macOS permission state for the actual process hosting the embedded engine.
/// This type never changes Ditch's session permission or prompts during polling.
public struct AutomationPermissionState: Codable, Sendable, Equatable {
    public let accessibility: Bool
    public let screenRecording: Bool
    public let postEvent: Bool
    public let activeDesktop: Bool
}

@MainActor
public enum AutomationPermissions {
    public static func status() -> AutomationPermissionState {
        // Do not reuse a sticky positive cache when checking revoked OS access.
        AutomationPermissionState(
            accessibility: AXIsProcessTrusted(),
            screenRecording: CGPreflightScreenCaptureAccess(),
            postEvent: CGPreflightPostEventAccess(),
            activeDesktop: Self.activeDesktop()
        )
    }

    static func activeDesktop() -> Bool {
        guard let session = CGSessionCopyCurrentDictionary() as? [String: Any] else { return false }
        return session[kCGSessionOnConsoleKey as String] as? Bool == true
            && session["CGSSessionScreenIsLocked"] as? Bool != true
    }

    /// Only the first-party UI setup flow may call this; never an MCP tool.
    @discardableResult
    public static func request(_ permission: String) -> Bool {
        switch permission {
        case "accessibility":
            return AXIsProcessTrustedWithOptions(
                [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
            )
        case "screenRecording":
            return CGRequestScreenCaptureAccess()
        case "postEvent":
            return CGRequestPostEventAccess()
        default:
            return false
        }
    }
}
