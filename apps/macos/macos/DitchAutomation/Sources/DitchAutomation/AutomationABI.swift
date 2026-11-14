import AppKit
import Foundation
import PeekabooFoundation

public typealias AutomationCompletion = @convention(c) (UInt64, UnsafePointer<UInt8>?, Int) -> Void
public typealias AutomationRevoked = @convention(c) () -> Void

/// Cancellation is accepted from any Rust thread, including before the main
/// actor starts the operation. Completion is still delivered exactly once.
final class NativeOperation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelled = false
    private var task: Task<Void, Never>?
    private var startWaiter: CheckedContinuation<Void, Never>?

    func waitUntilAttached() async {
        await withCheckedContinuation { continuation in
            self.lock.lock()
            if self.task != nil {
                self.lock.unlock()
                continuation.resume()
            } else {
                self.startWaiter = continuation
                self.lock.unlock()
            }
        }
    }

    func checkCancellation() throws {
        self.lock.lock()
        let cancelled = self.cancelled
        self.lock.unlock()
        if cancelled { throw CancellationError() }
    }

    func attach(_ task: Task<Void, Never>) {
        self.lock.lock()
        self.task = task
        let cancelled = self.cancelled
        let waiter = self.startWaiter
        self.startWaiter = nil
        self.lock.unlock()
        if cancelled { task.cancel() }
        waiter?.resume()
    }

    func cancel() {
        self.lock.lock()
        self.cancelled = true
        let task = self.task
        self.lock.unlock()
        task?.cancel()
    }
}

final class OperationTable: @unchecked Sendable {
    static let shared = OperationTable()
    private let lock = NSLock()
    private var operations: [UInt64: NativeOperation] = [:]

    func reserve(_ id: UInt64) -> NativeOperation? {
        self.lock.lock()
        defer { self.lock.unlock() }
        guard self.operations.isEmpty else { return nil }
        let operation = NativeOperation()
        self.operations[id] = operation
        return operation
    }

    func finish(_ id: UInt64) {
        self.lock.lock()
        self.operations.removeValue(forKey: id)
        self.lock.unlock()
    }

    func cancel(_ id: UInt64? = nil) {
        self.lock.lock()
        let operations = id.map { self.operations[$0].map { [$0] } ?? [] } ?? Array(self.operations.values)
        self.lock.unlock()
        for operation in operations { operation.cancel() }
    }
}

@MainActor
private enum NativeRuntime {
    static var adapter: PeekabooAdapter?
    static var observers: [NSObjectProtocol] = []
    static var revoked: AutomationRevoked?

    static func invalidate() {
        OperationTable.shared.cancel()
        self.revoked?()
        Task { await self.adapter?.invalidate() }
    }
}

@_cdecl("ditch_automation_abi_version")
public func automationABIVersion() -> UInt32 { 1 }

/// Called on AppKit's main thread before starting the Rust runtime. Byte input
/// is borrowed for this call only. No system permission is requested here.
@_cdecl("ditch_automation_initialize")
public func automationInitialize(_ bytes: UnsafePointer<UInt8>?, _ length: Int,
                                 _ revoked: AutomationRevoked?) -> Int32 {
    guard Thread.isMainThread, let bytes, (1...8192).contains(length), let revoked else { return 1 }
    let data = Data(bytes: bytes, count: length)
    struct Configuration: Decodable { let captures: String; let coordination: String }
    guard let config = try? JSONDecoder().decode(Configuration.self, from: data) else { return 1 }
    return MainActor.assumeIsolated {
        guard NativeRuntime.adapter == nil else { return 1 }
        do {
            let host = AutomationHost(coordinationDirectory: URL(fileURLWithPath: config.coordination))
            NativeRuntime.adapter = try PeekabooAdapter(host: host, captures: URL(fileURLWithPath: config.captures))
            NativeRuntime.revoked = revoked
            let workspace = NSWorkspace.shared.notificationCenter
            for name in [NSWorkspace.willSleepNotification, NSWorkspace.sessionDidResignActiveNotification] {
                NativeRuntime.observers.append(workspace.addObserver(forName: name, object: nil, queue: .main) { _ in
                    MainActor.assumeIsolated { NativeRuntime.invalidate() }
                })
            }
            for name in [NSWorkspace.didActivateApplicationNotification, NSWorkspace.activeSpaceDidChangeNotification] {
                NativeRuntime.observers.append(workspace.addObserver(forName: name, object: nil, queue: .main) { _ in
                    MainActor.assumeIsolated { NativeRuntime.adapter?.clearFeedback() }
                })
            }
            NativeRuntime.observers.append(DistributedNotificationCenter.default().addObserver(
                forName: NSNotification.Name("com.apple.screenIsLocked"), object: nil, queue: .main
            ) { _ in MainActor.assumeIsolated { NativeRuntime.invalidate() } })
            return 0
        } catch { return 1 }
    }
}

/// Output bytes are borrowed only during completion; the Rust caller copies
/// them synchronously. No Swift allocation crosses the ABI for Rust to free.
@_cdecl("ditch_automation_invoke")
public func automationInvoke(_ id: UInt64, _ bytes: UnsafePointer<UInt8>?, _ length: Int,
                             _ completion: AutomationCompletion?) -> Int32 {
    guard let bytes, let completion, (1...65536).contains(length),
          let operation = OperationTable.shared.reserve(id) else { return 1 }
    let data = Data(bytes: bytes, count: length)
    operation.attach(Task { @MainActor in
        let result: Data
        do {
            await operation.waitUntilAttached()
            try operation.checkCancellation()
            try Task.checkCancellation()
            guard let adapter = NativeRuntime.adapter else { throw AutomationFailure.unavailable }
            let request = try JSONDecoder().decode(NativeRequest.self, from: data)
            let payload = try await adapter.invoke(request)
            let value = try JSONSerialization.jsonObject(with: payload)
            result = try JSONSerialization.data(withJSONObject: ["ok": true, "result": value])
        } catch {
            result = (try? JSONEncoder().encode(AutomationDiagnostic(error)))
                ?? Data("{\"ok\":false,\"error\":\"operationFailed\"}".utf8)
        }
        OperationTable.shared.finish(id)
        result.withUnsafeBytes { buffer in completion(id, buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count) }
    })
    return 0
}

@_cdecl("ditch_automation_cancel")
public func automationCancel(_ id: UInt64) {
    OperationTable.shared.cancel(id)
    Task { @MainActor in NativeRuntime.adapter?.clearFeedback() }
}

@_cdecl("ditch_automation_forget_session")
public func automationForgetSession(_ bytes: UnsafePointer<UInt8>?, _ length: Int) {
    guard let bytes, (1...64).contains(length) else { return }
    let session = String(decoding: UnsafeBufferPointer(start: bytes, count: length), as: UTF8.self)
    Task { @MainActor in await NativeRuntime.adapter?.invalidate(session: session) }
}

@_cdecl("ditch_automation_revoke_all")
public func automationRevokeAll() {
    OperationTable.shared.cancel()
    Task { @MainActor in await NativeRuntime.adapter?.invalidate() }
}

/// Separate first-party setup entry point. It cannot be reached through a tool
/// name in automationInvoke, and must be protected by the host's UI channel.
@_cdecl("ditch_automation_request_permission")
public func automationRequestPermission(_ kind: UInt32) {
    let permission: String
    switch kind {
    case 1: permission = "accessibility"
    case 2: permission = "screenRecording"
    case 3: permission = "postEvent"
    default: return
    }
    Task { @MainActor in AutomationPermissions.request(permission) }
}
