import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)
    self.titleVisibility = .hidden
    self.titlebarAppearsTransparent = true
    self.styleMask.insert(.fullSizeContentView)
    self.isMovableByWindowBackground = false
    self.minSize = NSSize(width: 860, height: 620)
    self.backgroundColor = .windowBackgroundColor
    if #available(macOS 11.0, *) {
      self.toolbarStyle = .unified
      self.titlebarSeparatorStyle = .none
    }

    RegisterGeneratedPlugins(registry: flutterViewController)
    let registrar = flutterViewController.registrar(forPlugin: "NativeComposerTextView")
    registrar.register(
      NativeComposerTextViewFactory(messenger: registrar.messenger),
      withId: "the_ditch/composer_text_view")
    (NSApp.delegate as? AppDelegate)?.configureApplicationChannel(messenger: registrar.messenger)
    (NSApp.delegate as? AppDelegate)?.configureProjectPickerChannel(messenger: registrar.messenger)

    super.awakeFromNib()

    // Keep the native window identity aligned with the bundle's visible name.
    // AppKit may otherwise retain the nib or restored-state title across a
    // product rename even though Flutter and Info.plist already use the new name.
    self.title =
      Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String
      ?? "Ditch"
  }
}

class NativeComposerTextViewFactory: NSObject, FlutterPlatformViewFactory {
  private let messenger: FlutterBinaryMessenger

  init(messenger: FlutterBinaryMessenger) {
    self.messenger = messenger
    super.init()
  }

  func createArgsCodec() -> (FlutterMessageCodec & NSObjectProtocol)? {
    return FlutterStandardMessageCodec.sharedInstance()
  }

  func create(
    withViewIdentifier viewId: Int64,
    arguments args: Any?
  ) -> NSView {
    return NativeComposerTextView(
      frame: .zero,
      viewId: viewId,
      args: args,
      messenger: messenger)
  }
}

class NativeComposerTextView: NSView, NSTextViewDelegate {
  private let channel: FlutterMethodChannel
  private let platformViewsChannel: FlutterMethodChannel
  private let scrollView = NSScrollView()
  private let textView = ComposerTextView()
  private let placeholderLabel = ComposerPlaceholderLabel(labelWithString: "")
  private var isApplyingFlutterText = false
  private var lastReportedContentHeight: CGFloat = 0

  init(
    frame: NSRect,
    viewId: Int64,
    args: Any?,
    messenger: FlutterBinaryMessenger
  ) {
    channel = FlutterMethodChannel(
      name: "the_ditch/composer_text_view/\(viewId)",
      binaryMessenger: messenger)
    platformViewsChannel = FlutterMethodChannel(
      name: "flutter/platform_views",
      binaryMessenger: messenger)
    super.init(frame: frame)

    let arguments = args as? [String: Any]
    let initialText = arguments?["text"] as? String ?? ""
    let enabled = arguments?["enabled"] as? Bool ?? true
    let fontSize = arguments?["fontSize"] as? Double ?? 14
    let escapeEnabled = arguments?["escapeEnabled"] as? Bool ?? false
    let placeholder = arguments?["placeholder"] as? String ?? ""

    wantsLayer = true
    layer?.backgroundColor = NSColor.clear.cgColor

    scrollView.translatesAutoresizingMaskIntoConstraints = false
    scrollView.hasVerticalScroller = true
    scrollView.autohidesScrollers = true
    scrollView.drawsBackground = false
    scrollView.borderType = .noBorder

    textView.minSize = NSSize(width: 0, height: 0)
    textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
    textView.isVerticallyResizable = true
    textView.isHorizontallyResizable = false
    textView.autoresizingMask = [.width]
    textView.textContainer?.containerSize = NSSize(width: frame.width, height: CGFloat.greatestFiniteMagnitude)
    textView.textContainer?.widthTracksTextView = true
    textView.drawsBackground = false
    textView.font = NSFont(name: "Avenir Next", size: fontSize) ?? NSFont.systemFont(ofSize: fontSize)
    let textContainerInset = NSSize(width: 2, height: 6)
    textView.textContainerInset = textContainerInset
    textView.textColor = Self.color(arguments?["textColor"], fallback: NSColor.labelColor)
    textView.insertionPointColor = Self.color(arguments?["caretColor"], fallback: NSColor.labelColor)
    ComposerTextView.configureAsPlainText(textView)
    textView.allowsUndo = true
    textView.isAutomaticQuoteSubstitutionEnabled = false
    textView.isAutomaticDashSubstitutionEnabled = false
    textView.isAutomaticLinkDetectionEnabled = false
    textView.isAutomaticDataDetectionEnabled = false
    textView.string = initialText
    textView.isEditable = enabled
    textView.isSelectable = true
    textView.delegate = self
    textView.setAccessibilityLabel("Agent prompt")
    textView.onEnlarge = { [weak self] in
      self?.channel.invokeMethod("enlargeRequested", arguments: nil)
    }
    textView.onSubmit = { [weak self] in
      self?.channel.invokeMethod("submitRequested", arguments: nil)
    }
    textView.onFocusClaimed = { [weak self] in
      guard let self else { return }
      // AppKit owns the real first responder, while Flutter separately tracks
      // logical focus. Notify Flutter through its platform-view protocol so the
      // AppKitView focus node can evict stale focus from widgets such as xterm.
      self.platformViewsChannel.invokeMethod(
        "viewFocused",
        arguments: NSNumber(value: viewId))
      self.channel.invokeMethod("focusChanged", arguments: true)
    }
    textView.onFocusLost = { [weak self] in
      self?.channel.invokeMethod("focusChanged", arguments: false)
    }
    if escapeEnabled {
      textView.onEscape = { [weak self] in
        self?.channel.invokeMethod("escapePressed", arguments: nil)
      }
    }

    scrollView.documentView = textView
    addSubview(scrollView)

    placeholderLabel.stringValue = placeholder
    placeholderLabel.textColor = Self.color(
      arguments?["placeholderColor"], fallback: NSColor.placeholderTextColor)
    placeholderLabel.font = NSFont(name: "Avenir Next", size: fontSize)
      ?? NSFont.systemFont(ofSize: fontSize)
    placeholderLabel.translatesAutoresizingMaskIntoConstraints = false
    placeholderLabel.isHidden = !initialText.isEmpty
    addSubview(placeholderLabel)

    // Match the placeholder to TextKit's real text origin. NSTextView adds its
    // text-container inset and line-fragment padding before drawing the caret;
    // using unrelated constants puts the caret through the first glyph.
    let lineFragmentPadding = textView.textContainer?.lineFragmentPadding ?? 0
    let placeholderLeading = textContainerInset.width + lineFragmentPadding

    NSLayoutConstraint.activate([
      scrollView.leadingAnchor.constraint(equalTo: leadingAnchor),
      scrollView.trailingAnchor.constraint(equalTo: trailingAnchor),
      scrollView.topAnchor.constraint(equalTo: topAnchor),
      scrollView.bottomAnchor.constraint(equalTo: bottomAnchor),
      placeholderLabel.leadingAnchor.constraint(
        equalTo: leadingAnchor,
        constant: placeholderLeading),
      placeholderLabel.topAnchor.constraint(
        equalTo: topAnchor,
        constant: textContainerInset.height),
    ])

    applyPlainTextAppearance(
      font: textView.font ?? NSFont.systemFont(ofSize: fontSize),
      color: textView.textColor ?? NSColor.labelColor)

    DispatchQueue.main.async { [weak self] in
      self?.reportContentHeight(force: true)
    }

    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else {
        result(nil)
        return
      }

      switch call.method {
      case "setText":
        let text = call.arguments as? String ?? ""
        if self.textView.string != text {
          self.isApplyingFlutterText = true
          self.textView.string = text
          self.isApplyingFlutterText = false
          self.placeholderLabel.isHidden = !text.isEmpty
          self.applyPlainTextAppearance()
          self.reportContentHeight()
        }
        result(nil)
      case "getText":
        result(self.textView.string)
      case "clearText":
        if !self.textView.string.isEmpty {
          self.isApplyingFlutterText = true
          self.textView.string = ""
          self.isApplyingFlutterText = false
          self.placeholderLabel.isHidden = false
          self.applyPlainTextAppearance()
          self.reportContentHeight()
        }
        result(nil)
      case "setEnabled":
        self.textView.isEditable = (call.arguments as? Bool) ?? true
        result(nil)
      case "setAppearance":
        let appearance = call.arguments as? [String: Any]
        self.textView.textColor = Self.color(
          appearance?["textColor"], fallback: NSColor.labelColor)
        self.textView.insertionPointColor = Self.color(
          appearance?["caretColor"], fallback: NSColor.labelColor)
        self.placeholderLabel.textColor = Self.color(
          appearance?["placeholderColor"], fallback: NSColor.placeholderTextColor)
        let fontName = appearance?["fontName"] as? String ?? "Avenir Next"
        self.textView.font = NSFont(name: fontName, size: fontSize)
          ?? NSFont.systemFont(ofSize: fontSize)
        self.placeholderLabel.font = self.textView.font
        self.applyPlainTextAppearance()
        self.reportContentHeight()
        result(nil)
      case "focus":
        let wasFirstResponder = self.window?.firstResponder === self.textView
        let accepted = self.window?.makeFirstResponder(self.textView) ?? false
        if accepted && wasFirstResponder {
          self.textView.reassertFocusClaim()
        }
        result(accepted)
      case "blur":
        if self.window?.firstResponder == self.textView {
          self.window?.makeFirstResponder(nil)
        }
        result(nil)
      case "hasFocus":
        result(self.window?.firstResponder == self.textView)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  required init?(coder: NSCoder) {
    fatalError("init(coder:) has not been implemented")
  }

  private static func color(_ value: Any?, fallback: NSColor) -> NSColor {
    guard let number = value as? NSNumber else { return fallback }
    let argb = number.uint32Value
    return NSColor(
      calibratedRed: CGFloat((argb >> 16) & 0xff) / 255,
      green: CGFloat((argb >> 8) & 0xff) / 255,
      blue: CGFloat(argb & 0xff) / 255,
      alpha: CGFloat((argb >> 24) & 0xff) / 255)
  }

  override func layout() {
    super.layout()
    alignTextViewToViewport()
    reportContentHeight()
  }

  override func mouseDown(with event: NSEvent) {
    guard textView.isEditable else {
      super.mouseDown(with: event)
      return
    }
    // The scroll view normally routes clicks directly to the text view. Keep a
    // fallback for any uncovered editor space so the complete platform-view
    // surface behaves as one native text input and NSTextView still computes
    // the insertion point from the original mouse location.
    textView.mouseDown(with: event)
  }

  private func alignTextViewToViewport() {
    let viewport = scrollView.contentView.bounds.size
    guard viewport.width > 0, viewport.height > 0 else { return }
    var frame = textView.frame
    if frame.width != viewport.width {
      frame.size.width = viewport.width
      textView.frame = frame
    }
    frame.size.height = max(measuredContentHeight(), viewport.height)
    if frame != textView.frame {
      textView.frame = frame
    }
  }

  private func measuredContentHeight() -> CGFloat {
    guard let layoutManager = textView.layoutManager,
      let textContainer = textView.textContainer
    else { return textView.font?.boundingRectForFont.height ?? 0 }
    layoutManager.ensureLayout(for: textContainer)
    let usedHeight = layoutManager.usedRect(for: textContainer).height
    return ceil(max(textView.font?.boundingRectForFont.height ?? 0, usedHeight)
      + (textView.textContainerInset.height * 2))
  }

  private func applyPlainTextAppearance(font: NSFont? = nil, color: NSColor? = nil) {
    let resolvedFont = font ?? textView.font ?? NSFont.systemFont(ofSize: NSFont.systemFontSize)
    let resolvedColor = color ?? textView.textColor ?? NSColor.labelColor
    textView.font = resolvedFont
    textView.textColor = resolvedColor
    textView.typingAttributes = [
      .font: resolvedFont,
      .foregroundColor: resolvedColor,
    ]
    guard let storage = textView.textStorage, storage.length > 0 else { return }
    storage.setAttributes(
      [.font: resolvedFont, .foregroundColor: resolvedColor],
      range: NSRange(location: 0, length: storage.length))
  }

  private func reportContentHeight(force: Bool = false) {
    let height = measuredContentHeight()
    guard force || abs(height - lastReportedContentHeight) >= 1 else { return }
    lastReportedContentHeight = height
    channel.invokeMethod("contentHeightChanged", arguments: Double(height))
  }

  func textDidChange(_ notification: Notification) {
    placeholderLabel.isHidden = !textView.string.isEmpty
    reportContentHeight()
    if isApplyingFlutterText {
      return
    }
    channel.invokeMethod("textChanged", arguments: textView.string)
  }

}

final class ComposerPlaceholderLabel: NSTextField {
  override func hitTest(_ point: NSPoint) -> NSView? {
    // This label is visual decoration. The NSTextView underneath must receive
    // clicks so the editor focuses and places its caret at the clicked point.
    nil
  }
}

final class ComposerTextView: NSTextView {
  var onEnlarge: (() -> Void)?
  var onEscape: (() -> Void)?
  var onFocusClaimed: (() -> Void)?
  var onFocusLost: (() -> Void)?
  var onSubmit: (() -> Void)?

  static func configureAsPlainText(_ textView: NSTextView) {
    textView.isRichText = false
    textView.importsGraphics = false
  }

  static func plainText(from pasteboard: NSPasteboard) -> String? {
    pasteboard.string(forType: .string)
  }

  override func becomeFirstResponder() -> Bool {
    let accepted = super.becomeFirstResponder()
    if accepted {
      onFocusClaimed?()
    }
    return accepted
  }

  override func resignFirstResponder() -> Bool {
    let accepted = super.resignFirstResponder()
    if accepted {
      onFocusLost?()
    }
    return accepted
  }

  func reassertFocusClaim() {
    if window?.firstResponder === self {
      onFocusClaimed?()
    }
  }

  override func mouseDown(with event: NSEvent) {
    let wasFirstResponder = window?.firstResponder === self
    let accepted = window?.makeFirstResponder(self) ?? false
    if accepted && wasFirstResponder {
      reassertFocusClaim()
    }
    super.mouseDown(with: event)
  }

  override func paste(_ sender: Any?) {
    guard let plainText = Self.plainText(from: .general) else { return }
    insertText(plainText, replacementRange: selectedRange())
  }

  override func keyDown(with event: NSEvent) {
    guard event.keyCode == 123 || event.keyCode == 124 else {
      super.keyDown(with: event)
      return
    }
    let rawModifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
    let modifiers = rawModifiers.subtracting([.capsLock, .numericPad, .function])
    // Keep Option/Control navigation and input-method behavior native. The
    // explicit paths below make character and macOS line navigation reliable
    // when this NSTextView is hosted inside a Flutter platform view.
    guard !modifiers.contains(.option), !modifiers.contains(.control) else {
      super.keyDown(with: event)
      return
    }
    let selecting = modifiers.contains(.shift)
    if modifiers.contains(.command) {
      if event.keyCode == 123 {
        selecting ? moveToBeginningOfLineAndModifySelection(nil) : moveToBeginningOfLine(nil)
      } else {
        selecting ? moveToEndOfLineAndModifySelection(nil) : moveToEndOfLine(nil)
      }
    } else if event.keyCode == 123 {
      selecting ? moveLeftAndModifySelection(nil) : moveLeft(nil)
    } else {
      selecting ? moveRightAndModifySelection(nil) : moveRight(nil)
    }
  }

  override func doCommand(by selector: Selector) {
    if selector == #selector(insertNewline(_:)) {
      if hasMarkedText() {
        super.doCommand(by: selector)
        return
      }
      let modifiers = NSApp.currentEvent?.modifierFlags.intersection(.deviceIndependentFlagsMask)
      if modifiers?.contains(.shift) == true {
        super.doCommand(by: selector)
      } else {
        onSubmit?()
      }
      return
    }
    super.doCommand(by: selector)
  }

  override func performKeyEquivalent(with event: NSEvent) -> Bool {
    let rawModifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
    let modifiers = rawModifiers.subtracting([.capsLock, .numericPad, .function])
    if modifiers == [.command, .shift],
      event.charactersIgnoringModifiers?.lowercased() == "f"
    {
      onEnlarge?()
      return true
    }
    if event.keyCode == 53, let onEscape {
      onEscape()
      return true
    }
    guard window?.firstResponder == self,
      modifiers.contains(.command),
      let character = event.charactersIgnoringModifiers?.lowercased()
    else {
      return super.performKeyEquivalent(with: event)
    }
    switch character {
    case "v":
      paste(nil)
      return true
    case "c":
      copy(nil)
      return true
    case "x":
      cut(nil)
      return true
    case "a":
      selectAll(nil)
      return true
    case "z":
      if modifiers.contains(.shift) {
        undoManager?.redo()
      } else {
        undoManager?.undo()
      }
      return true
    default:
      break
    }
    return super.performKeyEquivalent(with: event)
  }
}
