import AppKit
import PeekabooAutomationKit
import PeekabooFoundation

/// Scoped by the native request, never by model-provided tool arguments.
@MainActor
final class DitchAutomationFeedbackClient: AutomationFeedbackClient {
    let visualizer: AutomationVisualizer
    var session: String?
    init(visualizer: AutomationVisualizer) { self.visualizer = visualizer }

    func showMouseMovement(from: CGPoint, to: CGPoint, duration: TimeInterval, target: VisualizerTargetWindow?) async -> Bool {
        guard let session else { return false }
        visualizer.trackPointer(session: session, duration: duration, dragging: false)
        return true
    }

    func showSwipeGesture(from: CGPoint, to: CGPoint, duration: TimeInterval, target: VisualizerTargetWindow?) async -> Bool {
        guard let session else { return false }
        visualizer.trackPointer(session: session, duration: duration, dragging: true)
        return true
    }
}

@MainActor
final class AutomationPointerView: NSView {
    let action: String
    init(action: String) { self.action = action; super.init(frame: NSRect(x: 0, y: 0, width: 72, height: 72)) }
    required init?(coder: NSCoder) { nil }
    override func draw(_ dirtyRect: NSRect) {
        let color: NSColor = action == "failed" ? .systemRed : .systemTeal
        color.withAlphaComponent(0.18).setFill()
        NSBezierPath(ovalIn: bounds.insetBy(dx: 7, dy: 7)).fill()
        color.setStroke()
        let ring = NSBezierPath(ovalIn: bounds.insetBy(dx: 9, dy: 9))
        ring.lineWidth = action.contains("click") ? 4 : 2
        ring.stroke()
        let symbol = action == "failed" ? "exclamationmark" : action == "type" ? "keyboard" : action == "key" ? "command" : "cursorarrow"
        if let image = NSImage(systemSymbolName: symbol, accessibilityDescription: nil) {
            image.withSymbolConfiguration(.init(paletteColors: [color]))?.draw(in: bounds.insetBy(dx: 23, dy: 21))
        }
    }
}
