import AppKit
import PeekabooAutomationKit

/// References belong to a single runtime turn. Neither model history nor a PID
/// alone establishes authority over a newly launched process.
@MainActor
final class AutomationTargetRegistry {
    struct Target {
        let session: String
        let pid: Int32
        let processStart: UInt64
        let bundle: String
        let window: UInt32?
        let bounds: CGRect?
    }
    private var targets: [String: Target] = [:]
    struct ProcessIdentity {
        let bundle: String
        let start: UInt64?
    }
    private let processIdentity: @MainActor (Int32) -> ProcessIdentity?

    init(processIdentity: @escaping @MainActor (Int32) -> ProcessIdentity? = AutomationTargetRegistry.readProcessIdentity) {
        self.processIdentity = processIdentity
    }

    static func readProcessIdentity(_ pid: Int32) -> ProcessIdentity? {
        guard let app = NSRunningApplication(processIdentifier: pid), !app.isTerminated else { return nil }
        // launchDate is optional even for live applications launched directly
        // (including Flutter). Use the same kernel identity as Peekaboo input.
        let start = SystemExactWindowMetadataProvider().processStartIdentity(for: pid)
        guard !app.isTerminated else { return nil }
        return ProcessIdentity(bundle: app.bundleIdentifier ?? "", start: start)
    }

    func clear(session: String?) {
        targets = targets.filter { session != nil && $0.value.session != session }
    }

    func register(_ app: NSRunningApplication, window: UInt32? = nil, bounds: CGRect? = nil,
                  session: String) throws -> String {
        try register(pid: app.processIdentifier, bundle: app.bundleIdentifier ?? "",
                     window: window, bounds: bounds, session: session)
    }

    func register(pid: Int32, bundle: String, window: UInt32? = nil, bounds: CGRect? = nil,
                  session: String) throws -> String {
        guard let identity = processIdentity(pid) else { throw AutomationFailure.applicationGone }
        guard identity.bundle == bundle else { throw AutomationFailure.targetChanged }
        guard let start = identity.start else { throw AutomationFailure.processIdentityUnavailable }
        return try register(Target(session: session, pid: pid, processStart: start,
            bundle: bundle, window: window, bounds: bounds))
    }

    func register(_ target: Target) throws -> String {
        if let existing = targets.first(where: {
            $0.value.session == target.session && $0.value.pid == target.pid && $0.value.processStart == target.processStart &&
            $0.value.bundle == target.bundle &&
            $0.value.window == target.window && $0.value.bounds == target.bounds
        }) { return existing.key }
        guard targets.count < 512 else { throw AutomationFailure.discoveryLimit }
        let id = "t_" + UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(12)
        targets[id] = target
        return id
    }

    func resolve(_ id: String?, session: String, requiredBundle: String?) throws -> Target {
        guard let id, let target = targets[id], target.session == session else {
            throw AutomationFailure.targetNotDiscovered
        }
        guard let identity = processIdentity(target.pid) else {
            targets.removeValue(forKey: id)
            throw AutomationFailure.applicationGone
        }
        guard let start = identity.start else { throw AutomationFailure.processIdentityUnavailable }
        guard start == target.processStart, identity.bundle == target.bundle else {
            targets.removeValue(forKey: id)
            throw AutomationFailure.targetChanged
        }
        try PeekabooAdapter.validateApplication(bundleIdentifier: target.bundle, requiredBundle: requiredBundle)
        if let window = target.window {
            guard let info = Self.windows(pid: target.pid).first(where: { Self.windowID($0) == window }) else {
                throw AutomationFailure.windowGone
            }
            guard Self.bounds(info) == target.bounds else { throw AutomationFailure.targetChanged }
        }
        return target
    }

    static func windows(pid: Int32) -> [[String: Any]] {
        (CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] ?? []).filter {
                ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid &&
                ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0 &&
                (($0[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 0) > 0
            }
    }
    static func windowID(_ info: [String: Any]) -> UInt32? {
        (info[kCGWindowNumber as String] as? NSNumber)?.uint32Value
    }
    static func bounds(_ info: [String: Any]) -> CGRect? {
        (info[kCGWindowBounds as String] as? NSDictionary).flatMap { CGRect(dictionaryRepresentation: $0) }
    }

    /// Global pointer input must land on the observed window, not an occluding app.
    static func requireVisible(_ target: Target, at point: CGPoint) throws {
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] ?? []
        try requireVisible(target, at: point, windows: windows)
    }

    static func requireVisible(_ target: Target, at point: CGPoint, windows: [[String: Any]]) throws {
        guard let bounds = target.bounds, bounds.contains(point), let window = target.window else {
            throw AutomationFailure.invalidArguments
        }
        for info in windows {
            guard ((info[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 0) > 0,
                  let rect = Self.bounds(info), rect.contains(point) else { continue }
            // Ditch's non-interactive overlay cannot redirect input.
            if let id = Self.windowID(info), AutomationVisualizer.overlayWindowIDs.contains(id) { continue }
            guard Self.windowID(info) == window else { throw AutomationFailure.targetObscured }
            return
        }
        throw AutomationFailure.targetObscured
    }
}
