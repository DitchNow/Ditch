import XCTest
@testable import DitchAutomation

final class AutomationHostTests: XCTestCase {
    @MainActor func testUnknownPermissionCannotPromptOrAuthorize() {
        XCTAssertFalse(AutomationPermissions.request("fullAccess"))
        XCTAssertFalse(AutomationPermissions.request(""))
    }

    func testCancellationBeforeTaskAttachmentPreventsDispatch() async {
        let operation = NativeOperation()
        operation.cancel()
        let task = Task { @MainActor in
            do {
                await operation.waitUntilAttached()
                try operation.checkCancellation()
                try Task.checkCancellation()
                XCTFail("cancelled operation reached input dispatch")
            } catch is CancellationError {} catch { XCTFail("unexpected error") }
        }
        operation.attach(task)
        await task.value
    }

    func testCancellationInterruptsSequenceAndRetainsExclusiveOwnershipUntilCompletion() async {
        let table = OperationTable()
        let operation = table.reserve(10)!
        XCTAssertNil(table.reserve(11))
        let started = expectation(description: "sequence started")
        let task = Task { @MainActor in
            started.fulfill()
            do {
                try await Task.sleep(for: .seconds(10))
                XCTFail("cancelled sequence continued")
            } catch is CancellationError {} catch { XCTFail("unexpected error") }
        }
        operation.attach(task)
        await fulfillment(of: [started], timeout: 1)
        table.cancel(10)
        XCTAssertNil(table.reserve(11), "cancel request is not completion")
        await task.value
        table.finish(10)
        XCTAssertNotNil(table.reserve(11))
        table.finish(11)
    }

    func testABIRejectsUnboundedInputWithoutSchedulingWork() {
        XCTAssertEqual(automationABIVersion(), 1)
        XCTAssertNotEqual(automationInvoke(1, nil, 0, nil), 0)
        XCTAssertNotEqual(automationInitialize(nil, 0, nil), 0)
    }

    @MainActor func testStartupRemovesOnlyOwnedCaptureArtifacts() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let capture = directory.appendingPathComponent("capture-expired.png")
        let unrelated = directory.appendingPathComponent("keep.txt")
        try Data([1]).write(to: capture)
        try Data([2]).write(to: unrelated)
        _ = try PeekabooAdapter(host: AutomationHost(coordinationDirectory: directory), captures: directory)
        XCTAssertFalse(FileManager.default.fileExists(atPath: capture.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: unrelated.path))
    }
}
