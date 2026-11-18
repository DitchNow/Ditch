import AppKit
import PeekabooAutomationKit
import XCTest
@testable import DitchAutomation

final class AutomationTargetRegistryTests: XCTestCase {
    @MainActor private final class ProcessFixture {
        var bundle = "test.app"
        var start: UInt64? = 100
        var running = true
        var identity: AutomationTargetRegistry.ProcessIdentity? {
            running ? .init(bundle: bundle, start: start) : nil
        }
    }

    @MainActor func testRegistrationUsesProcessStartAndDetectsPIDReuse() throws {
        let process = ProcessFixture()
        let registry = AutomationTargetRegistry { _ in process.identity }
        // This is the same registration path used for NSRunningApplication;
        // no LaunchServices timestamp exists or is required in the identity.
        let id = try registry.register(pid: 42, bundle: "test.app", session: "turn")
        XCTAssertEqual(id, try registry.register(pid: 42, bundle: "test.app", session: "turn"))
        XCTAssertEqual(try registry.resolve(id, session: "turn", requiredBundle: nil).processStart, 100)
        process.start = nil
        XCTAssertThrowsError(try registry.resolve(id, session: "turn", requiredBundle: nil)) {
            XCTAssertEqual($0 as? AutomationFailure, .processIdentityUnavailable)
        }
        XCTAssertThrowsError(try registry.register(pid: 42, bundle: "test.app", session: "turn")) {
            XCTAssertEqual($0 as? AutomationFailure, .processIdentityUnavailable)
        }
        process.start = 200
        XCTAssertThrowsError(try registry.resolve(id, session: "turn", requiredBundle: nil)) {
            XCTAssertEqual($0 as? AutomationFailure, .targetChanged)
        }
        let replacement = try registry.register(pid: 42, bundle: "test.app", session: "turn")
        XCTAssertNotEqual(id, replacement)
        process.running = false
        XCTAssertThrowsError(try registry.resolve(replacement, session: "turn", requiredBundle: nil)) {
            XCTAssertEqual($0 as? AutomationFailure, .applicationGone)
        }
        XCTAssertThrowsError(try registry.resolve(replacement, session: "turn", requiredBundle: nil)) {
            XCTAssertEqual($0 as? AutomationFailure, .targetNotDiscovered)
        }
    }

    @MainActor func testBundleChangeAndTurnOwnershipInvalidateAuthority() throws {
        let process = ProcessFixture()
        let registry = AutomationTargetRegistry { _ in process.identity }
        let id = try registry.register(pid: 42, bundle: process.bundle, session: "one")
        XCTAssertThrowsError(try registry.resolve(id, session: "two", requiredBundle: nil))
        XCTAssertThrowsError(try registry.resolve(id, session: "one", requiredBundle: "other.app")) {
            XCTAssertEqual($0 as? AutomationFailure, .protectedTarget)
        }
        process.bundle = "other.app"
        XCTAssertThrowsError(try registry.resolve(id, session: "one", requiredBundle: nil)) {
            XCTAssertEqual($0 as? AutomationFailure, .targetChanged)
        }
        XCTAssertThrowsError(try registry.register(pid: 42, bundle: "test.app", session: "one"))
    }

    func testPinnedProviderReadsKernelIdentityOfDirectTestProcess() throws {
        let provider = SystemExactWindowMetadataProvider()
        let identity = try XCTUnwrap(provider.processStartIdentity(for: getpid()))
        XCTAssertEqual(identity, provider.processStartIdentity(for: getpid()))
        XCTAssertNil(provider.processStartIdentity(for: -1))
    }

    func testPartialDiscoveryRetainsUsableTargetsAndTypedFailures() throws {
        var discovery = AutomationDiscovery()
        discovery.append(["name": "unavailable"]) { throw AutomationFailure.processIdentityUnavailable }
        discovery.append(["name": "usable"]) { "t_good" }
        let result = try XCTUnwrap(JSONSerialization.jsonObject(with: discovery.data(emptyStatus: "noMatches")) as? [String: Any])
        XCTAssertEqual(result["status"] as? String, "partial")
        XCTAssertEqual(discovery.targets.count, 1)
        XCTAssertEqual(discovery.targets.first?["target_id"] as? String, "t_good")
        XCTAssertNil(discovery.unavailable.first?["target_id"])
        XCTAssertEqual(discovery.unavailable.first?["error"] as? String, "processIdentityUnavailable")
    }

    func testEmptyDiscoveryDistinguishesNoMatchNoWindowAndUnavailable() throws {
        var discovery = AutomationDiscovery()
        for empty in ["noMatches", "noVisibleWindows"] {
            let result = try XCTUnwrap(JSONSerialization.jsonObject(with: discovery.data(emptyStatus: empty)) as? [String: Any])
            XCTAssertEqual(result["status"] as? String, empty)
        }
        discovery.append(["name": "requested app"]) { throw AutomationFailure.processIdentityUnavailable }
        let result = try XCTUnwrap(JSONSerialization.jsonObject(with: discovery.data(emptyStatus: "noMatches")) as? [String: Any])
        XCTAssertEqual(result["status"] as? String, "unavailable")
        XCTAssertTrue(discovery.targets.isEmpty)
    }
}
