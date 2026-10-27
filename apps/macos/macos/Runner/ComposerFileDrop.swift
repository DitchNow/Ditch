import Cocoa
import FlutterMacOS

/// Window routing covers Flutter controls; NSTextView routes file drags here too.
class ComposerDropWindow: NSWindow, NSDraggingDestination {
  var composerFileDrop: ComposerFileDrop?

  func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
    composerFileDrop?.update(sender) ?? []
  }
  func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
    composerFileDrop?.update(sender) ?? []
  }
  func draggingExited(_ sender: NSDraggingInfo?) {
    composerFileDrop?.exit()
  }
  func prepareForDragOperation(_ sender: NSDraggingInfo) -> Bool {
    composerFileDrop?.update(sender) == .copy
  }
  func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    composerFileDrop?.drop(sender) ?? false
  }
  func draggingEnded(_ sender: NSDraggingInfo) { composerFileDrop?.clear() }
  func wantsPeriodicDraggingUpdates() -> Bool { true }
  func concludeDragOperation(_ sender: NSDraggingInfo?) {
    composerFileDrop?.clear()
  }
}

final class ComposerFileDrop {
  struct Region {
    let id: Int
    let rect: NSRect
    let enabled: Bool
    let remote: Bool
    let background: NSColor
    let foreground: NSColor
    let accent: NSColor
  }
  private weak var view: NSView?
  private let channel: FlutterMethodChannel
  private var regions: [Int: Region] = [:]
  private var active: Int?
  private var lastLocation: NSPoint?
  private var lastSequence: Int?
  private var exitRevision = 0
  private let overlay = ComposerDropOverlay()

  init(view: NSView, messenger: FlutterBinaryMessenger) {
    self.view = view
    channel = FlutterMethodChannel(name: "the_ditch/composer_file_drop", binaryMessenger: messenger)
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self, let args = call.arguments as? [String: Any],
        let id = args["id"] as? Int else { result(nil); return }
      switch call.method {
      case "register":
        guard let x = args["x"] as? Double, let y = args["y"] as? Double,
          let width = args["width"] as? Double, let height = args["height"] as? Double
        else { result(nil); return }
        self.regions[id] = Region(id: id,
          rect: NSRect(x: x, y: y, width: width, height: height),
          enabled: args["enabled"] as? Bool ?? false,
          remote: args["remote"] as? Bool ?? false,
          background: Self.color(args["background"], fallback: .windowBackgroundColor),
          foreground: Self.color(args["foreground"], fallback: .labelColor),
          accent: Self.color(args["accent"], fallback: .controlAccentColor))
        if self.active == id, let location = self.lastLocation {
          if let region = self.region(at: location), let view = self.view {
            self.present(region, in: view)
          } else {
            self.clear()
          }
        }
      case "unregister":
        self.regions.removeValue(forKey: id)
        if self.active == id { self.clear() }
      default:
        result(FlutterMethodNotImplemented)
        return
      }
      result(nil)
    }
  }

  static func filePaths(from pasteboard: NSPasteboard) -> [String] {
    let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [
      .urlReadingFileURLsOnly: true,
    ]) as? [URL] ?? []
    return normalizedFilePaths(urls)
  }

  static func normalizedFilePaths(_ urls: [URL]) -> [String] {
    var seen = Set<String>()
    return urls.filter(\.isFileURL).map { $0.standardizedFileURL.path }
      .filter { seen.insert($0).inserted }
  }

  private static func color(_ value: Any?, fallback: NSColor) -> NSColor {
    guard let number = value as? NSNumber else { return fallback }
    let argb = number.uint32Value
    return NSColor(srgbRed: CGFloat((argb >> 16) & 0xff) / 255,
      green: CGFloat((argb >> 8) & 0xff) / 255,
      blue: CGFloat(argb & 0xff) / 255, alpha: CGFloat((argb >> 24) & 0xff) / 255)
  }

  /// Flutter coordinates and AppKit points are logical units, including Retina.
  static func appKitRect(_ rect: NSRect, in bounds: NSRect, flipped: Bool) -> NSRect {
    NSRect(x: bounds.minX + rect.minX,
      y: flipped ? bounds.minY + rect.minY : bounds.maxY - rect.maxY,
      width: rect.width, height: rect.height)
  }

  private func region(at location: NSPoint) -> Region? {
    guard let view else { return nil }
    let point = view.convert(location, from: nil)
    return regions.values.filter { $0.enabled }.sorted { $0.id > $1.id }.first {
      Self.appKitRect($0.rect, in: view.bounds, flipped: view.isFlipped).contains(point)
    }
  }

  func update(_ sender: NSDraggingInfo) -> NSDragOperation {
    exitRevision += 1
    lastLocation = sender.draggingLocation
    guard !Self.filePaths(from: sender.draggingPasteboard).isEmpty,
      sender.draggingSourceOperationMask.contains(.copy),
      let region = region(at: sender.draggingLocation), let view
    else { clear(); return [] }
    present(region, in: view)
    return .copy
  }

  private func present(_ region: Region, in view: NSView) {
    active = region.id
    overlay.message = region.remote
      ? "Local file inclusion is unavailable for SSH projects" : "Drop to include"
    overlay.background = region.background
    overlay.foreground = region.foreground
    overlay.accent = region.accent
    overlay.setAccessibilityLabel(overlay.message)
    // Install above the platform-view hierarchy. hitTest always passes through.
    let root = view.window?.contentView ?? view
    let rect = Self.appKitRect(region.rect, in: view.bounds, flipped: view.isFlipped)
    overlay.frame = root.convert(rect, from: view)
    if root.subviews.last !== overlay {
      root.addSubview(overlay, positioned: .above, relativeTo: nil)
    }
    overlay.needsDisplay = true
  }

  func drop(_ sender: NSDraggingInfo) -> Bool {
    defer { clear() }
    guard lastSequence != sender.draggingSequenceNumber,
      update(sender) == .copy, let id = active else { return false }
    lastSequence = sender.draggingSequenceNumber
    channel.invokeMethod("drop", arguments: [
      "id": id, "paths": Self.filePaths(from: sender.draggingPasteboard),
    ])
    return true
  }

  func exit() {
    // A window/editor destination handoff must not flash the overlay off.
    exitRevision += 1
    let revision = exitRevision
    DispatchQueue.main.async { [weak self] in
      guard let self, self.exitRevision == revision else { return }
      self.clear()
    }
  }

  func clear() {
    active = nil
    overlay.removeFromSuperview()
  }
}

final class ComposerDropOverlay: NSView {
  var message = "Drop to include"
  var background = NSColor.windowBackgroundColor
  var foreground = NSColor.labelColor
  var accent = NSColor.controlAccentColor
  override func hitTest(_ point: NSPoint) -> NSView? { nil }
  override func draw(_ dirtyRect: NSRect) {
    let shape = NSBezierPath(roundedRect: bounds.insetBy(dx: 2, dy: 2), xRadius: 10, yRadius: 10)
    background.withAlphaComponent(0.96).setFill()
    shape.fill()
    accent.setStroke()
    shape.lineWidth = 2
    shape.stroke()
    let style = NSMutableParagraphStyle()
    style.alignment = .center
    let attributes: [NSAttributedString.Key: Any] = [
      .font: NSFont.systemFont(ofSize: 15, weight: .semibold),
      .foregroundColor: foreground, .paragraphStyle: style,
    ]
    let text = NSAttributedString(string: message, attributes: attributes)
    text.draw(in: NSRect(x: 12, y: bounds.midY - 20, width: bounds.width - 24, height: 40))
  }
}
