import AppKit
import PeekabooAutomationKit

/// Discovery can succeed partially. Unavailable candidates never receive a
/// target_id; their fixed diagnostics explain why they cannot be acted upon.
struct AutomationDiscovery {
    private(set) var targets: [[String: Any]] = []
    private(set) var unavailable: [[String: Any]] = []

    mutating func append(_ candidate: [String: Any], register: () throws -> String) {
        var row = candidate
        do {
            row["target_id"] = try register()
            targets.append(row)
        } catch {
            let diagnostic = AutomationDiagnostic(error)
            row["error"] = diagnostic.error
            row["message"] = diagnostic.message
            unavailable.append(row)
        }
    }

    func data(emptyStatus: String) throws -> Data {
        let status = targets.isEmpty ? (unavailable.isEmpty ? emptyStatus : "unavailable")
            : (unavailable.isEmpty ? "ready" : "partial")
        let next = targets.isEmpty
            ? "No usable targets were returned. Inspect status and unavailable diagnostics. Rediscover the exact requested application once; do not invent target IDs or substitute another app."
            : "Use an application target_id to list its windows, then focus and observe a window target_id. Unavailable candidates cannot be acted upon."
        return try JSONSerialization.data(withJSONObject: ["targets": targets, "unavailable": unavailable,
                                                           "status": status, "next": next])
    }
}

extension PeekabooAdapter {
    static func publishAccessibility(_ result: ElementDetectionResult, manager: any SnapshotManagerProtocol) async throws -> String {
        let id = try await manager.createExplicitSnapshot()
        // Inspection's transient ID is not a snapshot authority. Publish its
        // immutable evidence under the native manager's newly owned reference.
        let published = ElementDetectionResult(snapshotId: id, screenshotPath: "", elements: result.elements, metadata: result.metadata)
        do { try await manager.storeDetectionResult(snapshotId: id, result: published) }
        catch { try? await manager.cleanSnapshot(snapshotId: id); throw error }
        return id
    }
    struct VisualObservation {
        let session: String
        let targetID: String
        let size: CGSize
        let created: Date
    }

    func discover(_ request: NativeRequest) throws -> Data {
        var discovery = AutomationDiscovery()
        if request.arguments.target_id != nil {
            let target = try registry.resolve(request.arguments.target_id, session: request.sessionId,
                                              requiredBundle: request.targetBundle)
            guard let app = NSRunningApplication(processIdentifier: target.pid) else { throw AutomationFailure.applicationGone }
            for window in AutomationTargetRegistry.windows(pid: target.pid).prefix(64) {
                guard let id = AutomationTargetRegistry.windowID(window), let bounds = AutomationTargetRegistry.bounds(window),
                      bounds.width > 1, bounds.height > 1 else { continue }
                discovery.append(["kind": "window", "title": window[kCGWindowName as String] as? String ?? "",
                                  "width": bounds.width, "height": bounds.height]) {
                    try registry.register(app, window: id, bounds: bounds, session: request.sessionId)
                }
            }
        } else {
            for app in NSWorkspace.shared.runningApplications where !app.isTerminated {
                let bundle = app.bundleIdentifier ?? ""
                guard (try? Self.validateApplication(bundleIdentifier: bundle, requiredBundle: request.targetBundle)) != nil else { continue }
                if let query = request.arguments.query, !query.isEmpty,
                   !(app.localizedName ?? "").localizedCaseInsensitiveContains(query), !bundle.localizedCaseInsensitiveContains(query) { continue }
                guard app.activationPolicy == .regular || !AutomationTargetRegistry.windows(pid: app.processIdentifier).isEmpty else { continue }
                guard discovery.targets.count + discovery.unavailable.count < 64 else { break }
                discovery.append(["kind": "application", "name": app.localizedName ?? "",
                                  "bundleIdentifier": bundle, "isActive": app.isActive]) {
                    try registry.register(app, session: request.sessionId)
                }
            }
        }
        return try discovery.data(emptyStatus: request.arguments.target_id == nil ? "noMatches" : "noVisibleWindows")
    }

    func observeVisual(_ request: NativeRequest) async throws -> Data {
        let target = try registry.resolve(request.arguments.target_id, session: request.sessionId, requiredBundle: request.targetBundle)
        guard let window = target.window, let targetID = request.arguments.target_id else { throw AutomationFailure.windowRequired }
        visualObservations = visualObservations.filter { $0.value.session != request.sessionId && Date().timeIntervalSince($0.value.created) < 60 }
        // Capture APIs return in-memory pixels. No AX traversal or snapshot writer
        // is involved in this foreground route.
        try await Task.sleep(for: .milliseconds(35))
        let capture = try await host.services.screenCapture.captureWindow(windowID: window, visualizerMode: .none, scale: .logical1x)
        _ = try registry.resolve(targetID, session: request.sessionId, requiredBundle: request.targetBundle)
        let (bytes, size) = try thumbnail(capture.imageData)
        let id = "s_" + UUID().uuidString.replacingOccurrences(of: "-", with: "").prefix(12)
        visualObservations[id] = .init(session: request.sessionId, targetID: targetID, size: size, created: Date())
        return try JSONSerialization.data(withJSONObject: ["snapshot_id": id, "target_id": targetID,
            "mode": "visual", "width": size.width, "height": size.height,
            "coordinate_space": "image_pixels_top_left", "accessibility_required": false,
            "image": ["mime_type": "image/png", "data": bytes.base64EncodedString()]])
    }
}
