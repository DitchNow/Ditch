import AppKit
import Foundation
import ImageIO
import PeekabooAutomationKit
import PeekabooFoundation
import UniformTypeIdentifiers

struct DesktopArguments: Decodable, Sendable {
    let target_id: String?
    let query: String?
    let mode: String?
    let to_x: Double?
    let to_y: Double?
    let pid: Int32?
    let window_id: UInt32?
    let include_image: Bool?
    let snapshot_id: String?
    let action: String?
    let element_id: String?
    let x: Double?
    let y: Double?
    let text: String?
    let keys: String?
    let direction: String?
    let amount: Int?
}

struct NativeRequest: Decodable, Sendable {
    let sessionId: String
    let operationId: String
    let tool: String
    let arguments: DesktopArguments
    let visualizer: Bool?
    let targetBundle: String?
}

enum AutomationFailure: String, Error {
    case outcomeUnknown = "outcome_unknown"
    case permissionRequired, invalidArguments, staleSnapshot, protectedTarget, busy, unavailable
    case applicationGone, processIdentityUnavailable, discoveryLimit, targetNotDiscovered, windowGone, targetChanged, targetObscured, windowRequired, targetNotFocused
}

@MainActor
final class PeekabooAdapter {
    struct Snapshot {
        let session: String
        let nativeID: String
        let context: WindowContext
        let imageSize: CGSize
        let capturedAt: Date
        let elementBounds: [String: CGRect]
    }

    let host: AutomationHost
    let registry = AutomationTargetRegistry()
    var visualObservations: [String: VisualObservation] = [:]
    private let captures: URL
    private var snapshots: [String: Snapshot] = [:]

    init(host: AutomationHost, captures: URL) throws {
        self.host = host
        self.captures = captures
        try FileManager.default.createDirectory(at: captures, withIntermediateDirectories: true,
                                               attributes: [.posixPermissions: 0o700])
        let attributes = try FileManager.default.attributesOfItem(atPath: captures.path)
        guard attributes[.type] as? FileAttributeType == .typeDirectory,
              (attributes[.ownerAccountID] as? NSNumber)?.uint32Value == getuid() else {
            throw AutomationFailure.unavailable
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: captures.path)
        // This environment's previous process owns no live snapshots after a
        // restart. Remove only our own capture files, never walk subdirectories.
        for file in try FileManager.default.contentsOfDirectory(at: captures, includingPropertiesForKeys: nil)
            where file.lastPathComponent.hasPrefix("capture-") && file.pathExtension == "png" {
            try? FileManager.default.removeItem(at: file)
        }
    }

    func invoke(_ request: NativeRequest) async throws -> Data {
        try Task.checkCancellation()
        // Remove feedback before any fresh observation or input dispatch.
        self.host.visualizer.clear()
        if request.tool == "desktop_status" {
            return try self.encode(self.host.permissions())
        }
        // Legacy PIDs/window IDs are not current-turn target authority.
        guard request.arguments.pid == nil, request.arguments.window_id == nil else {
            throw AutomationFailure.invalidArguments
        }
        let permissions = self.host.permissions()
        guard permissions.activeDesktop else { throw AutomationFailure.unavailable }
        guard permissions.accessibility else { throw AutomationFailure.permissionRequired }
        switch request.tool {
        case "desktop_list_targets":
            return try self.discover(request)
        case "desktop_observe":
            guard request.arguments.mode == nil || ["visual", "accessibility"].contains(request.arguments.mode!) else { throw AutomationFailure.invalidArguments }
            if request.arguments.mode != "accessibility" || request.arguments.include_image == true {
                guard permissions.screenRecording else { throw AutomationFailure.permissionRequired }
            }
            await clearSnapshots(session: request.sessionId)
            visualObservations = visualObservations.filter { $0.value.session != request.sessionId }
            if request.arguments.mode != "accessibility" { return try await self.observeVisual(request) }
            return try await self.observe(request)
        case "desktop_act":
            if request.arguments.action == "focus" && request.arguments.snapshot_id == nil {
                return try await self.focusTarget(request)
            }
            if let id = request.arguments.snapshot_id, self.visualObservations[id] != nil {
                return try await self.actVisual(request)
            }
            return try await self.act(request)
        default:
            throw AutomationFailure.invalidArguments
        }
    }

    func clearFeedback() { self.host.visualizer.clear() }

    func invalidate(session: String? = nil) async {
        self.registry.clear(session: session)
        self.visualObservations = self.visualObservations.filter { session != nil && $0.value.session != session }
        await self.clearSnapshots(session: session)
    }

    private func clearSnapshots(session: String? = nil) async {
        self.host.visualizer.clear(session: session)
        let keys = self.snapshots.filter { session == nil || $0.value.session == session }.map(\.key)
        for key in keys {
            if let snapshot = self.snapshots.removeValue(forKey: key) {
                try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID)
            }
        }
    }

    static func validateTarget(_ pid: Int32, requiredBundle: String? = nil) throws {
        guard pid > 0, let app = NSRunningApplication(processIdentifier: pid) else {
            throw AutomationFailure.staleSnapshot
        }
        try Self.validateApplication(bundleIdentifier: app.bundleIdentifier ?? "", requiredBundle: requiredBundle)
    }

    static func validateApplication(bundleIdentifier: String, requiredBundle: String? = nil) throws {
        if let requiredBundle, bundleIdentifier != requiredBundle { throw AutomationFailure.protectedTarget }
        // Full Access eligibility is enforced by the runtime. Ditch windows,
        // including the host's own windows, are ordinary automation targets.
        // Keep OS permission and authentication surfaces out of this adapter.
        guard !["com.apple.systempreferences", "com.apple.SecurityAgent", "com.apple.loginwindow"].contains(bundleIdentifier)
        else { throw AutomationFailure.protectedTarget }
    }

    private func observe(_ request: NativeRequest) async throws -> Data {
        let target = try registry.resolve(request.arguments.target_id, session: request.sessionId, requiredBundle: request.targetBundle)
        guard let window = target.window else { throw AutomationFailure.windowRequired }
        let resolved = try await ObservationTargetResolver(applications: host.services.applications)
            .resolve(.windowID(window), snapshot: .init())
        guard let initialContext = resolved.detectionContext else { throw AutomationFailure.staleSnapshot }
        let result = try await host.services.automation.inspectAccessibilityTree(windowContext: initialContext)
        try Task.checkCancellation()
        _ = try registry.resolve(request.arguments.target_id, session: request.sessionId, requiredBundle: request.targetBundle)
        guard let context = result.metadata.windowContext, context.windowMutationIdentity != nil,
              context.applicationProcessId == target.pid, context.windowID == Int(window),
              let bounds = context.windowBounds else { throw PeekabooError.accessibilityIncomplete("Exact window Accessibility context is unavailable") }
        let nativeID = try await Self.publishAccessibility(result, manager: host.services.snapshots)
        let id = UUID().uuidString
        var size = bounds.size
        var output: [String: Any] = ["snapshot_id": id, "target_id": request.arguments.target_id!,
            "mode": "accessibility"]
        if request.arguments.include_image == true {
            do {
                let capture = try await host.services.screenCapture.captureWindow(windowID: window, visualizerMode: .none, scale: .logical1x)
                let (bytes, imageSize) = try thumbnail(capture.imageData)
                size = imageSize
                output["image"] = ["mime_type": "image/png", "data": bytes.base64EncodedString()]
            } catch {
                try? await host.services.snapshots.cleanSnapshot(snapshotId: nativeID)
                throw error
            }
        }
        snapshots[id] = Snapshot(session: request.sessionId, nativeID: nativeID, context: context,
            imageSize: size, capturedAt: Date(),
            elementBounds: Dictionary(result.elements.all.prefix(200).map { ($0.id, $0.bounds) },
                                      uniquingKeysWith: { first, _ in first }))
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(30))
            guard let self, let snapshot = self.snapshots.removeValue(forKey: id) else { return }
            try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID)
        }
        var elements: [[String: Any]] = []
        var byteCount = 0
        for element in result.elements.all.prefix(200) {
            let value: [String: Any] = ["id": element.id, "role": String(describing: element.type),
                "label": String((element.label ?? "").prefix(512)),
                "value": String((element.value ?? "").prefix(512)), "enabled": element.isEnabled]
            let count = try JSONSerialization.data(withJSONObject: value).count
            guard byteCount + count <= 32 * 1024 else { break }
            byteCount += count
            elements.append(value)
        }
        output["width"] = size.width
        output["height"] = size.height
        output["elements"] = elements
        output["truncated"] = elements.count < result.elements.all.count
        return try JSONSerialization.data(withJSONObject: output)
    }

    private func act(_ request: NativeRequest) async throws -> Data {
        let args = request.arguments
        guard let id = args.snapshot_id, let snapshot = self.snapshots[id], snapshot.session == request.sessionId
        else { throw AutomationFailure.staleSnapshot }
        self.snapshots.removeValue(forKey: id)
        defer {
            Task { @MainActor in try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID) }
        }
        guard Date().timeIntervalSince(snapshot.capturedAt) <= 30,
              let identity = snapshot.context.windowMutationIdentity,
              let bounds = snapshot.context.windowBounds,
              let pid = snapshot.context.applicationProcessId
        else { throw AutomationFailure.staleSnapshot }
        try Self.validateTarget(pid, requiredBundle: request.targetBundle)
        try Task.checkCancellation()
        guard let automation = self.host.services.automation as? any UIAutomationActionOutcomeProviding else {
            throw AutomationFailure.unavailable
        }
        // Each snapshot is single-use. Upstream validates the capture-owned process
        // generation, window and receiver again immediately before delivery.
        let expected = DesktopTargetIdentity(exactWindow: try .init(identity: identity, bounds: bounds))
        func accepted<T: Sendable>(_ result: UIAutomationActionResult<T>) throws -> DesktopActionOutcome {
            try UIAutomationActionResultSemantics.requireAcceptedOutcome(result,
                policy: .init(acceptsDispatchedUnverified: true), targetRequirement: .compatible(expected),
                operation: "Ditch desktop action")
        }
        let outcome: DesktopActionOutcome
        var clickPoint: CGPoint?
        switch args.action {
        case "click":
            let target: ClickTarget
            if let element = args.element_id {
                target = .elementId(element)
                if let rect = snapshot.elementBounds[element] {
                    clickPoint = CGPoint(x: rect.midX, y: rect.midY)
                }
            } else if let x = args.x, let y = args.y, x.isFinite, y.isFinite,
                      x >= 0, y >= 0, x < snapshot.imageSize.width, y < snapshot.imageSize.height {
                let point = CGPoint(x: bounds.minX + x * bounds.width / snapshot.imageSize.width,
                                              y: bounds.minY + y * bounds.height / snapshot.imageSize.height)
                target = .coordinates(point)
                clickPoint = point
            } else { throw AutomationFailure.invalidArguments }
            outcome = try accepted(await automation.clickWithOutcome(target: target, clickType: .single,
                snapshotId: snapshot.nativeID, expectedWindowIdentity: identity, expectedWindowBounds: bounds))
        case "type":
            guard let text = args.text, !text.isEmpty, text.utf8.count <= 4096,
                  let receiver = snapshot.context.focusedElement else { throw AutomationFailure.invalidArguments }
            outcome = try accepted(await automation.typeActionsWithOutcome([.text(text)], cadence: .fixed(milliseconds: 20),
                snapshotId: snapshot.nativeID,
                target: .init(windowIdentity: identity, windowBounds: bounds, focusedElement: receiver)))
        case "key":
            guard let keys = args.keys, !keys.isEmpty, keys.count <= 80,
                  let receiver = snapshot.context.focusedElement else { throw AutomationFailure.invalidArguments }
            outcome = try accepted(await automation.hotkeyWithOutcome(keys: keys, holdDuration: 0,
                target: .init(windowIdentity: identity, windowBounds: bounds, focusedElement: receiver)))
        case "scroll":
            guard let direction = args.direction.flatMap(ScrollDirection.init(rawValue:)),
                  let amount = args.amount, (1...10).contains(amount), let element = args.element_id
            else { throw AutomationFailure.invalidArguments }
            outcome = try accepted(await automation.scrollWithOutcome(.init(direction: direction, amount: amount,
                target: element, snapshotId: snapshot.nativeID,
                expectedWindow: try .init(identity: identity, bounds: bounds), foreground: false)))
        case "focus":
            outcome = try accepted(await self.host.services.windows.focusWindowResult(
                target: .windowId(identity.windowID), expectedIdentity: identity))
        default:
            throw AutomationFailure.invalidArguments
        }
        try Task.checkCancellation()
        if let windowID = UInt32(exactly: identity.windowID) {
            self.host.visualizer.show(enabled: request.visualizer == true, session: request.sessionId,
                                      action: args.action ?? "", windowID: windowID,
                                      pid: pid, bounds: bounds, point: clickPoint)
        }
        try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID)
        return try JSONSerialization.data(withJSONObject: ["outcome": outcome.state.rawValue, "verification_required": true, "snapshot_consumed": true])
    }

    func thumbnail(_ data: Data) throws -> (Data, CGSize) {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceThumbnailMaxPixelSize: 1280,
              ] as CFDictionary)
        else { throw AutomationFailure.unavailable }
        let bytes = NSMutableData()
        guard let output = CGImageDestinationCreateWithData(bytes, UTType.png.identifier as CFString, 1, nil) else {
            throw AutomationFailure.unavailable
        }
        CGImageDestinationAddImage(output, image, nil)
        guard CGImageDestinationFinalize(output), bytes.length <= 2 * 1024 * 1024 else {
            throw AutomationFailure.unavailable
        }
        return (bytes as Data, CGSize(width: image.width, height: image.height))
    }

    private func encode<T: Encodable>(_ value: T) throws -> Data {
        let data = try JSONEncoder().encode(value)
        guard data.count <= 3 * 1024 * 1024 else { throw AutomationFailure.unavailable }
        return data
    }
}
