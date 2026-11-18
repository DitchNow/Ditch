import AppKit
import PeekabooAutomationKit
import PeekabooFoundation

extension PeekabooAdapter {
    static func imagePoint(x: Double?, y: Double?, size: CGSize, bounds: CGRect) throws -> CGPoint {
        guard let x, let y, x.isFinite, y.isFinite, size.width > 0, size.height > 0,
              x >= 0, y >= 0, x < size.width, y < size.height else { throw AutomationFailure.invalidArguments }
        return CGPoint(x: bounds.minX + x * bounds.width / size.width,
                       y: bounds.minY + y * bounds.height / size.height)
    }

    func focusTarget(_ request: NativeRequest) async throws -> Data {
        let target = try registry.resolve(request.arguments.target_id, session: request.sessionId, requiredBundle: request.targetBundle)
        guard let window = target.window else { throw AutomationFailure.windowRequired }
        // Activating an application does not require a complete AX tree. If its
        // selected window is not frontmost, use Peekaboo's exact-window focus.
        guard let app = NSRunningApplication(processIdentifier: target.pid), app.activate(options: [])
        else { throw AutomationFailure.targetNotFocused }
        try await Task.sleep(for: .milliseconds(150))
        if (try? Self.requireForeground(target)) == nil {
            _ = try accepted(await host.services.windows.focusWindowResult(target: .windowId(Int(window))))
        }
        _ = try registry.resolve(request.arguments.target_id, session: request.sessionId, requiredBundle: request.targetBundle)
        try Self.requireForeground(target)
        return try JSONSerialization.data(withJSONObject: ["outcome": "dispatchedUnverified", "verification_required": true,
            "next": "Observe this window before input."])
    }

    static func requireForeground(_ target: AutomationTargetRegistry.Target) throws {
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == target.pid,
              AutomationTargetRegistry.windows(pid: target.pid).first.flatMap(AutomationTargetRegistry.windowID) == target.window
        else { throw AutomationFailure.targetNotFocused }
    }

    func accepted<T: Sendable>(_ result: UIAutomationActionResult<T>) throws -> DesktopActionOutcome {
        try UIAutomationActionResultSemantics.requireAcceptedOutcome(result,
            policy: .init(acceptsDispatchedUnverified: true), operation: "Ditch foreground input")
    }

    func actVisual(_ request: NativeRequest) async throws -> Data {
        guard host.permissions().postEvent else { throw AutomationFailure.permissionRequired }
        let args = request.arguments
        guard let id = args.snapshot_id, let observation = visualObservations[id],
              observation.session == request.sessionId, Date().timeIntervalSince(observation.created) < 60
        else { throw AutomationFailure.staleSnapshot }
        // Consume before dispatch, including failures; a lost reply must never replay input.
        visualObservations.removeValue(forKey: id)
        let target = try registry.resolve(observation.targetID, session: request.sessionId, requiredBundle: request.targetBundle)
        guard let bounds = target.bounds else { throw AutomationFailure.windowRequired }
        try Self.requireForeground(target)
        try Task.checkCancellation()
        host.feedback.session = request.visualizer == true ? request.sessionId : nil
        defer { host.feedback.session = nil }
        let input = host.foregroundInput
        var point: CGPoint?
        func check() throws {
            try Task.checkCancellation()
            _ = try registry.resolve(observation.targetID, session: request.sessionId, requiredBundle: request.targetBundle)
            try Self.requireForeground(target)
        }
        func move(_ p: CGPoint) async throws {
            try AutomationTargetRegistry.requireVisible(target, at: p)
            _ = try accepted(await input.moveMouseWithOutcome(to: p, duration: 350, steps: 24, profile: .linear))
            try check()
            try AutomationTargetRegistry.requireVisible(target, at: p)
        }
        let outcome: DesktopActionOutcome
        var inputAttempted = false
        do {
            switch args.action {
            case "click", "double_click", "right_click", "move", "scroll", "drag":
                let p = try Self.imagePoint(x: args.x, y: args.y, size: observation.size, bounds: bounds)
                point = p
                // Validate every parameter before moving the pointer.
                let destination = args.action == "drag" ? try Self.imagePoint(x: args.to_x, y: args.to_y, size: observation.size, bounds: bounds) : nil
                if let destination { try AutomationTargetRegistry.requireVisible(target, at: destination) }
                if args.action == "scroll" {
                    guard args.direction.flatMap(ScrollDirection.init(rawValue:)) != nil,
                          let amount = args.amount, (1...10).contains(amount) else { throw AutomationFailure.invalidArguments }
                }
                try await move(p)
                if args.action == "drag", let window = target.window {
                    let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
                        as? [[String: Any]] ?? []
                    let withoutOverlays = windows.filter {
                        !AutomationVisualizer.overlayWindowIDs.contains(AutomationTargetRegistry.windowID($0) ?? 0)
                    }
                    guard AutomationVisualizer.unobscuredTarget(window, pid: target.pid, bounds: bounds, windows: withoutOverlays)
                    else { throw AutomationFailure.targetObscured }
                }
                inputAttempted = args.action != "move"
                switch args.action {
                case "move":
                    return try JSONSerialization.data(withJSONObject: ["outcome": "dispatchedUnverified", "verification_required": true, "snapshot_consumed": true])
                case "drag":
                    outcome = try accepted(await input.dragWithOutcome(.init(from: p, to: destination!, duration: 500,
                        steps: 32, modifiers: nil, profile: .linear)))
                case "scroll":
                    outcome = try accepted(await input.scrollWithOutcome(.init(direction: ScrollDirection(rawValue: args.direction!)!,
                        amount: args.amount!, foreground: true)))
                default:
                    let kind: ClickType = args.action == "double_click" ? .double : args.action == "right_click" ? .right : .single
                    outcome = try accepted(await input.clickWithOutcome(target: .coordinates(p), clickType: kind, snapshotId: nil))
                }
            case "type":
                guard let text = args.text, !text.isEmpty, text.count <= 256 else { throw AutomationFailure.invalidArguments }
                inputAttempted = true
                outcome = try accepted(await input.typeActionsWithOutcome([.text(text)], cadence: .fixed(milliseconds: 10), snapshotId: nil))
            case "key":
                guard let keys = args.keys, !keys.isEmpty, keys.count <= 80 else { throw AutomationFailure.invalidArguments }
                inputAttempted = true
                outcome = try accepted(await input.hotkeyWithOutcome(keys: keys, holdDuration: 0))
            default: throw AutomationFailure.invalidArguments
            }
            try Task.checkCancellation()
        } catch {
            if !(error is CancellationError) {
                host.visualizer.pointerFeedback(enabled: request.visualizer == true, session: request.sessionId, action: "failed", point: point)
            }
            if inputAttempted && !(error is DesktopActionFailure) && !(error is CancellationError) {
                throw AutomationFailure.outcomeUnknown
            }
            throw error
        }
        host.visualizer.pointerFeedback(enabled: request.visualizer == true, session: request.sessionId, action: args.action ?? "", point: point)
        return try JSONSerialization.data(withJSONObject: ["outcome": outcome.state.rawValue, "verification_required": true,
            "snapshot_consumed": true, "delivery": "foreground", "next": "Observe once to verify the result before another action."])
    }
}
