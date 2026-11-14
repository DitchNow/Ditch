import AppKit
import Foundation
import ImageIO
import PeekabooAutomationKit
import PeekabooFoundation
import UniformTypeIdentifiers

struct DesktopArguments: Decodable, Sendable {
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
}

enum AutomationFailure: String, Error {
    case permissionRequired, invalidArguments, staleSnapshot, protectedTarget, busy, unavailable
}

@MainActor
final class PeekabooAdapter {
    struct Snapshot {
        let session: String
        let nativeID: String
        let context: WindowContext
        let imageSize: CGSize
        let capturedAt: Date
        let artifact: URL
    }

    private let host: AutomationHost
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
        if request.tool == "desktop_status" {
            return try self.encode(self.host.permissions())
        }
        let permissions = self.host.permissions()
        guard permissions.activeDesktop else { throw AutomationFailure.unavailable }
        guard permissions.accessibility else { throw AutomationFailure.permissionRequired }
        switch request.tool {
        case "desktop_list_targets":
            if let pid = request.arguments.pid {
                try self.validateTarget(pid)
                let result = try await self.host.services.applications.listWindows(for: "PID:\(pid)", timeout: 2)
                return try self.encode(Array(result.data.windows.prefix(64)))
            }
            let result = try await self.host.services.applications.listApplications()
            let apps = result.data.applications.filter { (try? self.validateTarget($0.processIdentifier)) != nil }
            return try self.encode(Array(apps.prefix(64)))
        case "desktop_observe":
            guard permissions.screenRecording else { throw AutomationFailure.permissionRequired }
            return try await self.observe(request)
        case "desktop_act":
            return try await self.act(request)
        default:
            throw AutomationFailure.invalidArguments
        }
    }

    func invalidate(session: String? = nil) async {
        let keys = self.snapshots.filter { session == nil || $0.value.session == session }.map(\.key)
        for key in keys {
            if let snapshot = self.snapshots.removeValue(forKey: key) {
                try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID)
                try? FileManager.default.removeItem(at: snapshot.artifact)
            }
        }
    }

    private func validateTarget(_ pid: Int32) throws {
        guard pid > 0, let app = NSRunningApplication(processIdentifier: pid) else {
            throw AutomationFailure.staleSnapshot
        }
        let bundle = app.bundleIdentifier ?? ""
        // Permission changes remain first-party user actions, even under Full Access.
        guard pid != ProcessInfo.processInfo.processIdentifier,
              !bundle.hasPrefix("ai.theditch."),
              !["com.apple.systempreferences", "com.apple.SecurityAgent", "com.apple.loginwindow"].contains(bundle)
        else { throw AutomationFailure.protectedTarget }
    }

    private func observe(_ request: NativeRequest) async throws -> Data {
        guard let pid = request.arguments.pid, let window = request.arguments.window_id else {
            throw AutomationFailure.invalidArguments
        }
        try self.validateTarget(pid)
        await self.invalidate(session: request.sessionId)
        if self.snapshots.count >= 8 { await self.invalidate() }
        let artifact = self.captures.appendingPathComponent("capture-\(UUID().uuidString).png")
        var retained = false
        defer { if !retained { try? FileManager.default.removeItem(at: artifact) } }
        let result = try await self.host.services.desktopObservation.observe(.init(
            target: .pid(pid, window: .id(window)),
            capture: .init(scale: .logical1x, focus: .background),
            detection: .init(mode: .accessibility, allowWebFocusFallback: false,
                             traversalBudget: .init(maxDepth: 10, maxElementCount: 200, maxChildrenPerNode: 80),
                             requiresFreshAccessibilityTree: true),
            output: .init(path: artifact.path, saveRawScreenshot: true, saveSnapshot: true, includeImageData: true),
            timeout: .init(overall: 8, detection: 3)
        ))
        try Task.checkCancellation()
        guard let context = result.target.detectionContext,
              context.applicationProcessId == pid,
              context.windowID == Int(window),
              context.windowMutationIdentity != nil,
              let nativeID = result.files.publishedSnapshotID
        else { throw AutomationFailure.staleSnapshot }
        let (image, imageSize) = try self.thumbnail(result.capture.imageData)
        let id = UUID().uuidString
        self.snapshots[id] = Snapshot(session: request.sessionId, nativeID: nativeID, context: context,
                                      imageSize: imageSize, capturedAt: Date(), artifact: artifact)
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(30))
            guard let self, let snapshot = self.snapshots.removeValue(forKey: id) else { return }
            try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID)
            try? FileManager.default.removeItem(at: snapshot.artifact)
        }
        retained = true
        var elements: [[String: Any]] = []
        var byteCount = 0
        for element in result.elements?.elements.all ?? [] {
            let value: [String: Any] = ["id": element.id, "role": String(describing: element.type),
                                       "label": String((element.label ?? "").prefix(512)),
                                       "value": String((element.value ?? "").prefix(512)),
                                       "enabled": element.isEnabled]
            let count = try JSONSerialization.data(withJSONObject: value).count
            guard byteCount + count <= 32 * 1024 else { break }
            byteCount += count
            elements.append(value)
        }
        var output: [String: Any] = ["snapshot_id": id, "pid": pid, "window_id": window,
                                     "width": imageSize.width, "height": imageSize.height,
                                     "elements": elements,
                                     "truncated": elements.count < (result.elements?.elements.all.count ?? 0)]
        if request.arguments.include_image == true {
            output["image"] = ["mime_type": "image/png", "data": image.base64EncodedString()]
        }
        return try JSONSerialization.data(withJSONObject: output)
    }

    private func act(_ request: NativeRequest) async throws -> Data {
        let args = request.arguments
        guard let id = args.snapshot_id, let snapshot = self.snapshots[id], snapshot.session == request.sessionId
        else { throw AutomationFailure.staleSnapshot }
        self.snapshots.removeValue(forKey: id)
        defer {
            try? FileManager.default.removeItem(at: snapshot.artifact)
            Task { @MainActor in try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID) }
        }
        guard Date().timeIntervalSince(snapshot.capturedAt) <= 30,
              let identity = snapshot.context.windowMutationIdentity,
              let bounds = snapshot.context.windowBounds,
              let pid = snapshot.context.applicationProcessId
        else { throw AutomationFailure.staleSnapshot }
        try self.validateTarget(pid)
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
        switch args.action {
        case "click":
            let target: ClickTarget
            if let element = args.element_id {
                target = .elementId(element)
            } else if let x = args.x, let y = args.y, x.isFinite, y.isFinite,
                      x >= 0, y >= 0, x < snapshot.imageSize.width, y < snapshot.imageSize.height {
                target = .coordinates(CGPoint(x: bounds.minX + x * bounds.width / snapshot.imageSize.width,
                                              y: bounds.minY + y * bounds.height / snapshot.imageSize.height))
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
        try? await self.host.services.snapshots.cleanSnapshot(snapshotId: snapshot.nativeID)
        return try JSONSerialization.data(withJSONObject: ["outcome": outcome.state.rawValue, "verification_required": true, "snapshot_consumed": true])
    }

    private func thumbnail(_ data: Data) throws -> (Data, CGSize) {
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
