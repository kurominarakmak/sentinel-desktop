import XCTest
@testable import SentinelMac

final class NativeBridgeTests: XCTestCase {
    func testDecodesDurableTaskUpdateWithoutWorkflowInference() throws {
        let data = Data(#"{"kind":"task_update","task":{"id":"task-1","summary":"Ship it","lifecycle":"reviewing","recoveryRequired":false,"recoveryReason":null,"version":4,"updatedAtMs":9}}"#.utf8)
        XCTAssertEqual(try JSONDecoder().decode(BridgeMessage.self, from: data), .taskUpdate(NativeTask(id: "task-1", summary: "Ship it", lifecycle: "reviewing", recoveryRequired: false, recoveryReason: nil, version: 4, updatedAtMs: 9)))
    }

    func testDecodesAcceptedAndRejectedTaskStartResults() throws {
        let accepted = Data(#"{"kind":"task_start_result","requestId":"request-1","accepted":true,"task":{"id":"task-1","summary":"Ship it","lifecycle":"implementing","recoveryRequired":false,"recoveryReason":null,"version":4,"updatedAtMs":9}}"#.utf8)
        XCTAssertEqual(try JSONDecoder().decode(BridgeMessage.self, from: accepted), .taskStartResult(requestID: "request-1", accepted: true, task: NativeTask(id: "task-1", summary: "Ship it", lifecycle: "implementing", recoveryRequired: false, recoveryReason: nil, version: 4, updatedAtMs: 9), message: nil))
        let rejected = Data(#"{"kind":"task_start_result","requestId":"request-2","accepted":false,"message":"repository is unavailable"}"#.utf8)
        XCTAssertEqual(try JSONDecoder().decode(BridgeMessage.self, from: rejected), .taskStartResult(requestID: "request-2", accepted: false, task: nil, message: "repository is unavailable"))
    }

    func testSubmissionGateRejectsEmptyUnavailableAndDuplicateRequests() {
        var gate = TaskSubmissionGate()
        XCTAssertEqual(gate.begin(prompt: "  ", providerAvailable: true, requestID: "empty"), .rejected("Describe the task before sending."))
        XCTAssertEqual(gate.begin(prompt: "Task", providerAvailable: false, requestID: "provider"), .rejected("Selected provider is unavailable."))
        XCTAssertEqual(gate.begin(prompt: "Task", providerAvailable: true, requestID: "one"), .accepted("one"))
        XCTAssertEqual(gate.begin(prompt: "Task", providerAvailable: true, requestID: "two"), .rejected("Task submission is already in progress."))
        XCTAssertFalse(gate.complete(requestID: "wrong"))
        XCTAssertTrue(gate.complete(requestID: "one"))
    }
}
