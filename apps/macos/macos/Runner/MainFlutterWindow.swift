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
  private let scrollView = NSScrollView()
  private let textView = ComposerTextView()
  private let placeholderLabel = NSTextField(labelWithString: "")
  private var isApplyingFlutterText = false

  init(
    frame: NSRect,
    viewId: Int64,
    args: Any?,
    messenger: FlutterBinaryMessenger
  ) {
    channel = FlutterMethodChannel(
      name: "the_ditch/composer_text_view/\(viewId)",
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
    textView.font = NSFont.systemFont(ofSize: fontSize)
    textView.textContainerInset = NSSize(width: 2, height: 6)
    textView.textColor = NSColor.labelColor
    textView.insertionPointColor = NSColor.labelColor
    textView.allowsUndo = true
    textView.isAutomaticQuoteSubstitutionEnabled = false
    textView.isAutomaticDashSubstitutionEnabled = false
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
    if escapeEnabled {
      textView.onEscape = { [weak self] in
        self?.channel.invokeMethod("escapePressed", arguments: nil)
      }
    }

    scrollView.documentView = textView
    addSubview(scrollView)

    placeholderLabel.stringValue = placeholder
    placeholderLabel.textColor = .placeholderTextColor
    placeholderLabel.font = NSFont.systemFont(ofSize: fontSize)
    placeholderLabel.translatesAutoresizingMaskIntoConstraints = false
    placeholderLabel.isHidden = !initialText.isEmpty
    addSubview(placeholderLabel)

    NSLayoutConstraint.activate([
      scrollView.leadingAnchor.constraint(equalTo: leadingAnchor),
      scrollView.trailingAnchor.constraint(equalTo: trailingAnchor),
      scrollView.topAnchor.constraint(equalTo: topAnchor),
      scrollView.bottomAnchor.constraint(equalTo: bottomAnchor),
      placeholderLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
      placeholderLabel.topAnchor.constraint(equalTo: topAnchor, constant: 7),
    ])

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
        }
        result(nil)
      case "setEnabled":
        self.textView.isEditable = (call.arguments as? Bool) ?? true
        result(nil)
      case "focus":
        self.window?.makeFirstResponder(self.textView)
        result(nil)
      case "blur":
        if self.window?.firstResponder == self.textView {
          self.window?.makeFirstResponder(nil)
        }
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }

  required init?(coder: NSCoder) {
    fatalError("init(coder:) has not been implemented")
  }

  func textDidChange(_ notification: Notification) {
    placeholderLabel.isHidden = !textView.string.isEmpty
    if isApplyingFlutterText {
      return
    }
    channel.invokeMethod("textChanged", arguments: textView.string)
  }
}

final class ComposerTextView: NSTextView {
  var onEnlarge: (() -> Void)?
  var onEscape: (() -> Void)?
  var onSubmit: (() -> Void)?

  override func doCommand(by selector: Selector) {
    if selector == #selector(insertNewline(_:)) {
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
    let modifiers = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
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
    return super.performKeyEquivalent(with: event)
  }
}
