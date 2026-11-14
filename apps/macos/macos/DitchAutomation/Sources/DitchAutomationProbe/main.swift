import AppKit
import DitchAutomation
import Foundation

/// Explicit diagnostic target. No permission prompts or desktop input.
@main
struct DitchAutomationProbe {
    @MainActor static func main() throws {
        let args = CommandLine.arguments
        guard args.count == 2 else {
            throw NSError(domain: "DitchAutomationProbe", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "Supply an isolated coordination directory"])
        }
        let host = AutomationHost(coordinationDirectory: URL(fileURLWithPath: args[1], isDirectory: true))
        let data = try JSONEncoder().encode(host.permissions())
        print(String(decoding: data, as: UTF8.self))
    }
}
