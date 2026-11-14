import AppKit

@MainActor
final class AutomationOverlayView: NSView {
    let action: String
    let point: CGPoint?

    init(frame: NSRect, action: String, point: CGPoint?) {
        self.action = action
        self.point = point
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { nil }

    override func draw(_ dirtyRect: NSRect) {
        let accent = NSColor.controlAccentColor
        accent.withAlphaComponent(0.75).setStroke()
        let outline = NSBezierPath(roundedRect: bounds.insetBy(dx: 3, dy: 3), xRadius: 8, yRadius: 8)
        outline.lineWidth = 2
        outline.stroke()
        if let point, action == "click" {
            accent.withAlphaComponent(0.16).setFill()
            let ring = NSBezierPath(ovalIn: NSRect(x: point.x - 13, y: point.y - 13, width: 26, height: 26))
            ring.fill()
            ring.lineWidth = 2
            ring.stroke()
            accent.setFill()
            NSBezierPath(ovalIn: NSRect(x: point.x - 3, y: point.y - 3, width: 6, height: 6)).fill()
        } else {
            // Fixed icons only: observed labels, typed text and shortcuts never enter the overlay.
            let symbols = ["observe": "viewfinder", "type": "keyboard", "key": "command",
                           "scroll": "arrow.up.arrow.down", "focus": "macwindow", "click": "cursorarrow.click"]
            let chip = NSRect(x: max(6, bounds.midX - 18), y: 8, width: 36, height: 28)
            NSColor.windowBackgroundColor.withAlphaComponent(0.95).setFill()
            NSBezierPath(roundedRect: chip, xRadius: 8, yRadius: 8).fill()
            if let image = NSImage(systemSymbolName: symbols[action] ?? "viewfinder", accessibilityDescription: nil) {
                image.draw(in: chip.insetBy(dx: 9, dy: 5))
            }
        }
    }
}
