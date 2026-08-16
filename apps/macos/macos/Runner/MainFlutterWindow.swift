import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    let registrar = flutterViewController.registrar(forPlugin: "NativeComposerTextView")
    registrar.register(
      NativeComposerTextViewFactory(messenger: registrar.messenger),
      withId: "the_ditch/composer_text_view")

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
  private let textView = NSTextView()
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
    textView.font = NSFont.systemFont(ofSize: 18)
    textView.textColor = NSColor.labelColor
    textView.insertionPointColor = NSColor.labelColor
    textView.allowsUndo = true
    textView.isAutomaticQuoteSubstitutionEnabled = false
    textView.isAutomaticDashSubstitutionEnabled = false
    textView.string = initialText
    textView.isEditable = enabled
    textView.isSelectable = true
    textView.delegate = self

    scrollView.documentView = textView
    addSubview(scrollView)

    NSLayoutConstraint.activate([
      scrollView.leadingAnchor.constraint(equalTo: leadingAnchor),
      scrollView.trailingAnchor.constraint(equalTo: trailingAnchor),
      scrollView.topAnchor.constraint(equalTo: topAnchor),
      scrollView.bottomAnchor.constraint(equalTo: bottomAnchor),
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
        }
        result(nil)
      case "getText":
        result(self.textView.string)
      case "clearText":
        if !self.textView.string.isEmpty {
          self.isApplyingFlutterText = true
          self.textView.string = ""
          self.isApplyingFlutterText = false
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
    if isApplyingFlutterText {
      return
    }
    channel.invokeMethod("textChanged", arguments: textView.string)
  }
}
