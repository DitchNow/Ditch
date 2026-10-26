import Cocoa
import FlutterMacOS

class MainFlutterWindow: ComposerDropWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)
    self.titleVisibility = .hidden
    self.titlebarAppearsTransparent = true
    // Leave title-bar gestures with AppKit (including the system double-click action).
    self.styleMask.remove(.fullSizeContentView)
    self.isMovableByWindowBackground = false
    self.minSize = NSSize(width: 860, height: 620)
    self.backgroundColor = .windowBackgroundColor
    if #available(macOS 11.0, *) {
      self.toolbarStyle = .unified
      self.titlebarSeparatorStyle = .none
    }

    RegisterGeneratedPlugins(registry: flutterViewController)
    let registrar = flutterViewController.registrar(forPlugin: "NativeComposerTextView")
    composerFileDrop = ComposerFileDrop(view: flutterViewController.view, messenger: registrar.messenger)
    registerForDraggedTypes([.fileURL])
    registrar.register(
      NativeComposerTextViewFactory(messenger: registrar.messenger),
      withId: "the_ditch/composer_text_view")
    (NSApp.delegate as? AppDelegate)?.configureApplicationChannel(messenger: registrar.messenger)
    (NSApp.delegate as? AppDelegate)?.configureProjectPickerChannel(messenger: registrar.messenger)

    super.awakeFromNib()

    // AppKit owns persistence and display constraints; bundle identity isolates staging.
    let frameName = NSWindow.FrameAutosaveName(
      "\(Bundle.main.bundleIdentifier ?? "Ditch").mainWindow")
    if !self.setFrameUsingName(frameName) {
      self.setContentSize(NSSize(width: 1280, height: 820))
      self.setFrame(self.constrainFrameRect(self.frame, to: self.screen), display: false)
      self.center()
    }
    self.setFrameAutosaveName(frameName)

    // Keep the native window identity aligned with the bundle's visible name.
    // AppKit may otherwise retain the nib or restored-state title across a
    // product rename even though Flutter and Info.plist already use the new name.
    self.title =
      Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String
      ?? "Ditch"
  }
}
