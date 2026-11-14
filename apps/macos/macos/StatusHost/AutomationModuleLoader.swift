import Cocoa
import Darwin

private typealias AutomationCompletion = @convention(c) (UInt64, UnsafePointer<UInt8>?, Int) -> Void
private typealias AutomationInvoke = @convention(c) (UInt64, UnsafePointer<UInt8>?, Int, AutomationCompletion?) -> Int32
private typealias AutomationCancel = @convention(c) (UInt64) -> Void
private typealias AutomationRevoke = @convention(c) () -> Void
private typealias AutomationPermission = @convention(c) (UInt32) -> Void
private typealias AutomationVerify = @convention(c) (Int32, UInt32, UnsafePointer<UInt8>?, Int) -> Int32
private typealias AutomationForget = @convention(c) (UnsafePointer<UInt8>?, Int) -> Void

@_silgen_name("ditch_register_automation")
private func registerAutomation(_ version: UInt32, _ invoke: AutomationInvoke?, _ cancel: AutomationCancel?,
                                _ revoke: AutomationRevoke?, _ permission: AutomationPermission?,
                                _ verify: AutomationVerify?, _ forget: AutomationForget?, _ name: UnsafePointer<UInt8>?, _ length: Int) -> Int32
@_silgen_name("ditch_desktop_session_unavailable")
private func desktopSessionUnavailable()

/// Optional native module: loading it dynamically preserves Ditch's existing
/// OS baseline. Never search PATH, user plug-in folders, or a standalone app.
enum AutomationModuleLoader {
  private static var handle: UnsafeMutableRawPointer?

  static func load() -> Bool {
    guard Thread.isMainThread else { return false }
    guard #available(macOS 15.0, *) else { return false }
    guard handle == nil, let frameworks = Bundle.main.privateFrameworksURL else { return false }
    let path = frameworks.appendingPathComponent("DitchAutomation.framework/Versions/A/DitchAutomation").path
    guard let library = dlopen(path, RTLD_NOW | RTLD_LOCAL) else { return false }
    func symbol<T>(_ name: String, _: T.Type) -> T? {
      guard let pointer = dlsym(library, name) else { return nil }
      return unsafeBitCast(pointer, to: T.self)
    }
    typealias Version = @convention(c) () -> UInt32
    typealias Initialize = @convention(c) (UnsafePointer<UInt8>?, Int, AutomationRevoke?) -> Int32
    guard let version = symbol("ditch_automation_abi_version", Version.self), version() == 1,
          let initialize = symbol("ditch_automation_initialize", Initialize.self),
          let invoke = symbol("ditch_automation_invoke", AutomationInvoke.self),
          let cancel = symbol("ditch_automation_cancel", AutomationCancel.self),
          let revoke = symbol("ditch_automation_revoke_all", AutomationRevoke.self),
          let permission = symbol("ditch_automation_request_permission", AutomationPermission.self),
          let verify = symbol("ditch_automation_verify_peer", AutomationVerify.self),
          let forget = symbol("ditch_automation_forget_session", AutomationForget.self) else {
      dlclose(library)
      return false
    }
    // Swift tasks and callbacks may retain module code after initialization.
    // Keep the module resident even if registration subsequently fails.
    handle = library
    let home = FileManager.default.homeDirectoryForCurrentUser
    let captures = home.appendingPathComponent(DitchIdentity.supportRelativePath)
      .appendingPathComponent("DesktopCaptures")
    let coordination = home.appendingPathComponent("Library/Application Support/Ditch Desktop Coordination")
    guard let config = try? JSONSerialization.data(withJSONObject: ["captures": captures.path, "coordination": coordination.path]) else { return false }
    let initialized = config.withUnsafeBytes { buffer in
      initialize(buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count, { desktopSessionUnavailable() })
    }
    guard initialized == 0 else { return false }
    let name = Data((Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
                     ?? Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "Ditch Runtime").utf8)
    return name.withUnsafeBytes { buffer in
      registerAutomation(1, invoke, cancel, revoke, permission, verify, forget,
                         buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count) == 0
    }
  }
}
