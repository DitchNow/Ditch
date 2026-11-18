import AppKit
import QuartzCore

/// Local presentation only. No distributed notifications, files, or model-visible events.
@MainActor
final class AutomationVisualizer {
    static private(set) var overlayWindowIDs = Set<UInt32>()
    private var window: AutomationOverlayWindow?
    private var owner: String?
    private var expiry: Task<Void, Never>?
    private var generation = 0
    var isVisible: Bool { window != nil }

    static func appKitRect(_ rect: CGRect, primaryTop: CGFloat) -> CGRect {
        CGRect(x: rect.minX, y: primaryTop - rect.maxY, width: rect.width, height: rect.height)
    }

    /// Suppress feedback for hidden, moved, minimized or even partly covered targets.
    /// Never expose a background app by floating its outline above the foreground app.
    static func unobscuredTarget(_ id: UInt32, pid: Int32, bounds: CGRect,
                                 windows: [[String: Any]]) -> Bool {
        guard bounds.width > 0, bounds.height > 0,
              [bounds.minX, bounds.minY, bounds.width, bounds.height].allSatisfy({ $0.isFinite }) else { return false }
        for item in windows {
            guard let rectData = item[kCGWindowBounds as String] as? NSDictionary,
                  let rect = CGRect(dictionaryRepresentation: rectData),
                  (item[kCGWindowAlpha as String] as? Double ?? 1) > 0 else { continue }
            if (item[kCGWindowNumber as String] as? NSNumber)?.uint32Value == id {
                return (item[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid && rect == bounds
            }
            if rect.intersects(bounds) { return false }
        }
        return false
    }

    func clear(session: String? = nil) {
        guard session == nil || owner == session else { return }
        generation += 1
        expiry?.cancel()
        expiry = nil
        if let window { Self.overlayWindowIDs.remove(UInt32(window.windowNumber)) }
        window?.orderOut(nil)
        window?.close()
        window = nil
        owner = nil
        CATransaction.flush()
    }

    func show(enabled: Bool, session: String, action: String, windowID: UInt32,
              pid: Int32, bounds: CGRect, point: CGPoint? = nil) {
        clear()
        guard enabled, !Task.isCancelled,
              let top = NSScreen.screens.first?.frame.maxY,
              let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
                as? [[String: Any]],
              Self.unobscuredTarget(windowID, pid: pid, bounds: bounds, windows: windows) else { return }
        let frame = Self.appKitRect(bounds, primaryTop: top)
        let overlay = AutomationOverlayWindow(frame: frame)
        let localPoint = point.flatMap { p in
            bounds.contains(p) ? CGPoint(x: p.x - bounds.minX, y: bounds.maxY - p.y) : nil
        }
        overlay.contentView = AutomationOverlayView(frame: CGRect(origin: .zero, size: frame.size),
                                                    action: action, point: localPoint)
        owner = session
        window = overlay
        Self.overlayWindowIDs.insert(UInt32(overlay.windowNumber))
        overlay.orderFrontRegardless()
        let token = generation
        expiry = Task { @MainActor [weak self, weak overlay] in
            // The pulse indicates dispatch/observation, never verified success.
            try? await Task.sleep(for: .milliseconds(350))
            guard !Task.isCancelled, let self, self.generation == token else { return }
            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                await NSAnimationContext.runAnimationGroup { context in
                    context.duration = 0.18
                    overlay?.animator().alphaValue = 0
                }
            }
            guard !Task.isCancelled, self.generation == token else { return }
            self.clear(session: session)
        }
    }

    private func pointerWindow(session: String, action: String, point: CGPoint) -> AutomationOverlayWindow? {
        clear()
        guard let top = NSScreen.screens.first?.frame.maxY else { return nil }
        let overlay = AutomationOverlayWindow(frame: CGRect(x: point.x - 36, y: top - point.y - 36, width: 72, height: 72))
        overlay.contentView = AutomationPointerView(action: action)
        owner = session
        window = overlay
        Self.overlayWindowIDs.insert(UInt32(overlay.windowNumber))
        overlay.orderFrontRegardless()
        return overlay
    }

    /// Sample the real global pointer during native dispatch. Never replay an
    /// imagined path after a failed action or move the physical pointer here.
    func trackPointer(session: String, duration: TimeInterval, dragging: Bool) {
        guard let point = CGEvent(source: nil)?.location,
              let overlay = pointerWindow(session: session, action: dragging ? "drag" : "move", point: point),
              let top = NSScreen.screens.first?.frame.maxY else { return }
        let token = generation
        expiry = Task { @MainActor [weak self, weak overlay] in
            let end = Date().addingTimeInterval(min(2, max(0.1, duration)) + 0.25)
            while Date() < end, !Task.isCancelled {
                guard let self, self.generation == token, let overlay else { return }
                if let point = CGEvent(source: nil)?.location {
                    overlay.setFrameOrigin(CGPoint(x: point.x - 36, y: top - point.y - 36))
                }
                try? await Task.sleep(for: .milliseconds(16))
            }
            guard !Task.isCancelled, let self, self.generation == token else { return }
            self.clear(session: session)
        }
    }

    func pointerFeedback(enabled: Bool, session: String, action: String, point: CGPoint?) {
        guard enabled, !Task.isCancelled, let point = point ?? CGEvent(source: nil)?.location,
              pointerWindow(session: session, action: action, point: point) != nil else { return }
        let token = generation
        expiry = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(750))
            guard !Task.isCancelled, let self, self.generation == token else { return }
            self.clear(session: session)
        }
    }
}
