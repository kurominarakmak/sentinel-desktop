import XCTest
@testable import SentinelMac

final class NativeBridgeTests: XCTestCase {
    private func attention(id: String = "task-1", version: UInt64 = 1, state: String = "working", actions: AttentionActions = AttentionActions(stop: true, approve: false, reject: false, approvalID: nil)) -> NativeAttention {
        NativeAttention(task: NativeTask(id: id, summary: "Ship it", lifecycle: "implementing", recoveryRequired: state == "recovery_required", recoveryReason: state == "recovery_required" ? "provider_missing" : nil, version: version, updatedAtMs: Int64(version)), displayState: state, provider: "Codex", activity: "tool_started", validation: "passed", review: nil, actions: actions)
    }

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

    func testDecodesDurableAttentionStateAndAuthorizedActions() throws {
        let data = Data(#"{"kind":"attention_state","attention":{"task":{"id":"task-1","summary":"Ship it","lifecycle":"ready_for_human","recoveryRequired":false,"recoveryReason":null,"version":4,"updatedAtMs":9},"displayState":"ready_for_review","provider":"Claude Code","activity":"review_reported","validation":"passed","review":"1 blocker","actions":{"stop":false,"approve":true,"reject":true,"approvalID":"approval-1"}}}"#.utf8)
        let expected = NativeAttention(task: NativeTask(id: "task-1", summary: "Ship it", lifecycle: "ready_for_human", recoveryRequired: false, recoveryReason: nil, version: 4, updatedAtMs: 9), displayState: "ready_for_review", provider: "Claude Code", activity: "review_reported", validation: "passed", review: "1 blocker", actions: AttentionActions(stop: false, approve: true, reject: true, approvalID: "approval-1"))
        XCTAssertEqual(try JSONDecoder().decode(BridgeMessage.self, from: data), .attentionState(expected))
    }

    func testAttentionGateRejectsStaleUpdateAcceptsReplacementAndClearsNoTask() {
        var gate = AttentionUpdateGate()
        XCTAssertTrue(gate.apply(attention(version: 3)))
        XCTAssertFalse(gate.apply(attention(version: 2, state: "failed")))
        XCTAssertEqual(gate.current?.task.version, 3)
        XCTAssertTrue(gate.apply(attention(id: "task-2", version: 1, state: "recovery_required")))
        XCTAssertEqual(gate.current?.task.id, "task-2")
        XCTAssertTrue(gate.apply(nil))
        XCTAssertNil(gate.current)
    }

    func testAttentionActionGatePermitsOnlyOnePendingAction() {
        var gate = AttentionActionGate()
        XCTAssertTrue(gate.begin(requestID: "stop"))
        XCTAssertFalse(gate.begin(requestID: "approve"))
        XCTAssertFalse(gate.complete(requestID: "approve"))
        XCTAssertTrue(gate.complete(requestID: "stop"))
    }

    func testDecodesAcceptedAndRejectedAttentionActionResponses() throws {
        let accepted = Data(#"{"kind":"attention_action_result","requestId":"action-1","accepted":true}"#.utf8)
        XCTAssertEqual(try JSONDecoder().decode(BridgeMessage.self, from: accepted), .attentionActionResult(requestID: "action-1", accepted: true, message: nil))
        let rejected = Data(#"{"kind":"attention_action_result","requestId":"action-2","accepted":false,"message":"action is not authorized for the current task"}"#.utf8)
        XCTAssertEqual(try JSONDecoder().decode(BridgeMessage.self, from: rejected), .attentionActionResult(requestID: "action-2", accepted: false, message: "action is not authorized for the current task"))
    }
}
