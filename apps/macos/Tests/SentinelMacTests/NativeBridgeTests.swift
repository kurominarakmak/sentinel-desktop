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

    func testDecodesHealthyDurableTaskDetailWithApprovalAndEvidence() throws {
        let data = Data(#"{"kind":"task_detail","detail":{"task":{"id":"task-1","summary":"Ship it","lifecycle":"ready_for_human","recoveryRequired":false,"recoveryReason":null,"version":4,"updatedAtMs":9},"sessions":[{"provider":"Codex","sessionRef":"thread-1","state":"active","updatedAtMs":8}],"activity":[{"kind":"task_prepared","provider":"Codex","occurredAtMs":1,"payload":"{}"},{"kind":"review_reported","provider":"Claude Code","occurredAtMs":2,"payload":"{}"}],"worktree":{"repositoryRoot":"/repo","path":"/worktree","branch":"agent/task","baseCommit":"abc","state":"ready"},"diff":{"targetBranch":"main","targetAdvanced":true,"mergeReady":false,"summary":"{\"filesChanged\":2}","conflicts":"[]"},"validations":[{"id":"check","profile":"test","check":"unit","required":true,"state":"failed","summary":"exit 1","updatedAtMs":3,"command":"[\"cargo\",\"test\"]","exitCode":1,"durationMs":12,"stdout":"safe output","stderr":"redacted output","outcome":"failed"}],"findings":[{"id":"finding","repairRoundId":"round-1","severity":"blocker","disposition":"confirmed_blocking","summary":"Fix me","evidence":"{\"file\":\"src/a.rs\",\"line\":3}"}],"repairRounds":[{"id":"round-1","round":1,"state":"completed","updatedAtMs":4}],"finalApprovalPacket":"{\"unresolved_risks\":[\"risk\"]}","actions":{"stop":false,"approve":true,"reject":true,"approvalID":"approval-1"}}}"#.utf8)
        guard case .taskDetail(let detail) = try JSONDecoder().decode(BridgeMessage.self, from: data) else { return XCTFail("expected task detail") }
        XCTAssertEqual(detail.activity.map(\.occurredAtMs), [1, 2])
        XCTAssertEqual(detail.worktree?.branch, "agent/task")
        XCTAssertEqual(detail.validations.first?.exitCode, 1)
        XCTAssertEqual(detail.findings.first?.severity, "blocker")
        XCTAssertEqual(detail.repairRounds.first?.round, 1)
        XCTAssertEqual(detail.actions.approvalID, "approval-1")
    }

    func testDetailGateRejectsStaleUpdateAndAcceptsReplacement() {
        let action = AttentionActions(stop: false, approve: false, reject: false, approvalID: nil)
        let first = NativeTaskDetail(task: NativeTask(id: "task-1", summary: "one", lifecycle: "implementing", recoveryRequired: false, recoveryReason: nil, version: 2, updatedAtMs: 2), sessions: [], activity: [], worktree: nil, diff: nil, validations: [], findings: [], repairRounds: [], finalApprovalPacket: nil, actions: action)
        let stale = NativeTaskDetail(task: NativeTask(id: "task-1", summary: "old", lifecycle: "implementing", recoveryRequired: false, recoveryReason: nil, version: 1, updatedAtMs: 1), sessions: [], activity: [], worktree: nil, diff: nil, validations: [], findings: [], repairRounds: [], finalApprovalPacket: nil, actions: action)
        let replacement = NativeTaskDetail(task: NativeTask(id: "task-2", summary: "new", lifecycle: "recovering", recoveryRequired: true, recoveryReason: "missing_thread", version: 1, updatedAtMs: 3), sessions: [], activity: [], worktree: nil, diff: nil, validations: [], findings: [], repairRounds: [], finalApprovalPacket: nil, actions: action)
        var gate = DetailUpdateGate()
        XCTAssertTrue(gate.apply(first))
        XCTAssertFalse(gate.apply(stale))
        XCTAssertTrue(gate.apply(replacement))
        XCTAssertEqual(gate.current?.task.id, "task-2")
    }

    func testDecodesCodexRateLimitsAndClaudeAuthLimitedStatus() throws {
        let data = Data(#"{"kind":"status","status":{"version":4,"sentinel":"implementing","activeTask":{"id":"task-1","summary":"Ship it","lifecycle":"implementing","recoveryRequired":false,"recoveryReason":null,"version":4,"updatedAtMs":9},"recoveryRequired":false,"codex":{"name":"Codex","installation":"available · 1.0","runtime":"session active","usage":"live","rateLimits":{"primary":{"usedPercent":42,"resetsAt":"12:00","windowDurationMins":300}}},"claude":{"name":"Claude Code","installation":"available · 1.0","runtime":"no owned session","usage":"Authenticated usage is unavailable in the native bridge.","rateLimits":null}}}"#.utf8)
        guard case .status(let status) = try JSONDecoder().decode(BridgeMessage.self, from: data) else { return XCTFail("expected status") }
        XCTAssertEqual(status.codex.rateLimits?["primary"]?.usedPercent, 42)
        XCTAssertNil(status.codex.rateLimits?["secondary"])
        XCTAssertTrue(status.claude.usage.contains("unavailable"))
        XCTAssertEqual(status.activeTask?.id, "task-1")
    }

    func testDecodesCodexNumericRateLimitResetAndKeepsTrayPercentage() throws {
        let data = Data(#"{"kind":"status","status":{"version":1,"sentinel":"ready","activeTask":null,"recoveryRequired":false,"codex":{"name":"Codex","installation":"available","runtime":"active","usage":"live","rateLimits":{"primary":{"usedPercent":39,"resetsAt":1787196921,"windowDurationMins":10080}}},"claude":{"name":"Claude Code","installation":"unavailable","runtime":"none","usage":"unavailable","rateLimits":null}}}"#.utf8)
        guard case .status(let status) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected status")
        }
        XCTAssertEqual(CodexTrayTitle.make(status), "39%")
        XCTAssertTrue(status.codex.rateLimits?["primary"]?.resetsAt?.contains("T") == true)
    }

    func testStatusGateRejectsStaleAndAcceptsProviderReplacementOrRecovery() {
        let unavailable = NativeProviderStatus(name: "Codex", installation: "unavailable", runtime: "no owned session", usage: "unavailable", rateLimits: nil)
        let claude = NativeProviderStatus(name: "Claude Code", installation: "not installed", runtime: "no owned session", usage: "unavailable", rateLimits: nil)
        let current = NativeStatus(version: 3, sentinel: "implementing", activeTask: NativeTask(id: "task-1", summary: "work", lifecycle: "implementing", recoveryRequired: false, recoveryReason: nil, version: 3, updatedAtMs: 3), recoveryRequired: false, codex: unavailable, claude: claude)
        let stale = NativeStatus(version: 2, sentinel: "ready", activeTask: nil, recoveryRequired: false, codex: unavailable, claude: claude)
        let recovery = NativeStatus(version: 4, sentinel: "recovering", activeTask: NativeTask(id: "task-2", summary: "recover", lifecycle: "recovering", recoveryRequired: true, recoveryReason: "provider_missing", version: 4, updatedAtMs: 4), recoveryRequired: true, codex: NativeProviderStatus(name: "Codex", installation: "available", runtime: "session recovery_required", usage: "unavailable", rateLimits: nil), claude: claude)
        var gate = StatusUpdateGate()
        XCTAssertTrue(gate.apply(current))
        XCTAssertFalse(gate.apply(stale))
        XCTAssertTrue(gate.apply(recovery))
        XCTAssertEqual(gate.current?.activeTask?.id, "task-2")
        XCTAssertTrue(gate.current?.recoveryRequired == true)
    }

    func testDecodesSanitizedSettingsSnapshotAndConfiguredExecutableOverride() throws {
        let data = Data(#"{"kind":"settings","settings":{"version":8,"globalShortcut":"Command+Shift+Space","repository":"/repo","defaultProvider":"codex","codex":{"name":"Codex","installation":"available","executableOverride":"/usr/local/bin/codex","supportsExecutableOverride":false,"authentication":"CLI-owned authentication; credentials are not exposed to Sentinel."},"claude":{"name":"Claude Code","installation":"unavailable","executableOverride":null,"supportsExecutableOverride":false,"authentication":"Authentication state is unavailable unless the provider proves it."},"validationProfiles":[{"id":"default","steps":[{"name":"unit","kind":"test","cwd":".","timeoutMs":120000,"required":true}]}],"validationError":null,"token":"secret-value"}}"#.utf8)
        guard case .settings(let settings) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected settings snapshot")
        }
        XCTAssertEqual(settings.codex.executableOverride, "/usr/local/bin/codex")
        XCTAssertFalse(settings.codex.supportsExecutableOverride)
        XCTAssertEqual(settings.validationProfiles.first?.steps.first?.kind, "test")
        XCTAssertFalse(String(describing: settings).contains("secret-value"))
    }

    func testSettingsSnapshotRepresentsRejectedOrUnsupportedOverridesWithoutMutation() throws {
        let data = Data(#"{"kind":"settings","settings":{"version":9,"globalShortcut":"Command+Shift+Space","repository":null,"defaultProvider":"codex","codex":{"name":"Codex","installation":"unavailable","executableOverride":null,"supportsExecutableOverride":false,"authentication":"CLI-owned authentication; credentials are not exposed to Sentinel."},"claude":{"name":"Claude Code","installation":"unavailable","executableOverride":null,"supportsExecutableOverride":false,"authentication":"Authentication state is unavailable unless the provider proves it."},"validationProfiles":[],"validationError":"No repository context is available."}}"#.utf8)
        guard case .settings(let settings) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected settings snapshot")
        }
        XCTAssertNil(settings.codex.executableOverride)
        XCTAssertFalse(settings.codex.supportsExecutableOverride)
        XCTAssertNotNil(settings.validationError)
    }

    func testSettingsGateRejectsStaleSnapshotAndPreservesConfirmedState() {
        let provider = SettingsProvider(name: "Codex", installation: "available", executableOverride: nil, supportsExecutableOverride: false, authentication: "CLI-owned")
        let claude = SettingsProvider(name: "Claude Code", installation: "unavailable", executableOverride: nil, supportsExecutableOverride: false, authentication: "unavailable")
        let confirmed = NativeSettings(version: 4, globalShortcut: "Command+Shift+Space", repository: "/repo", defaultProvider: "codex", codex: provider, claude: claude, validationProfiles: [], validationError: nil)
        let stale = NativeSettings(version: 3, globalShortcut: "Command+Shift+Space", repository: nil, defaultProvider: "codex", codex: provider, claude: claude, validationProfiles: [], validationError: "stale")
        var gate = SettingsUpdateGate()
        XCTAssertTrue(gate.apply(confirmed))
        XCTAssertFalse(gate.apply(stale))
        XCTAssertEqual(gate.current?.repository, "/repo")
    }

    func testSurfaceRegistryCreatesEachSurfaceOnlyOnce() {
        var registry = NativeSurfaceRegistry()
        XCTAssertTrue(registry.requestOpen(.quickPrompt))
        XCTAssertFalse(registry.requestOpen(.quickPrompt))
        XCTAssertTrue(registry.requestOpen(.attention))
        XCTAssertTrue(registry.requestOpen(.taskDetail))
        XCTAssertTrue(registry.requestOpen(.status))
        XCTAssertTrue(registry.requestOpen(.settings))
        XCTAssertEqual(registry.owned.count, 5)
    }

    func testFocusRestorationOnlyTargetsStillRunningPreviousApplication() {
        var focus = PreviousApplicationFocus()
        let sentinel: pid_t = 100
        // The pure state machine is intentionally independent of AppKit activation.
        // It will ignore a stale process identifier and clear it after the attempt.
        focus = PreviousApplicationFocus(processIdentifier: 42)
        XCTAssertNil(focus.takeRestoreCandidate(runningProcessIdentifiers: [sentinel], sentinelPID: sentinel))
        XCTAssertNil(focus.takeRestoreCandidate(runningProcessIdentifiers: [42], sentinelPID: sentinel))
        focus = PreviousApplicationFocus(processIdentifier: 42)
        XCTAssertEqual(focus.takeRestoreCandidate(runningProcessIdentifiers: [42, sentinel], sentinelPID: sentinel), 42)
    }

    func testFloatingPanelPlacementCentersOffScreenFrameWithoutMovingVisibleFrame() {
        let screen = NSRect(x: 0, y: 0, width: 1000, height: 800)
        let offScreen = NSRect(x: 4000, y: 4000, width: 320, height: 240)
        let visible = NSRect(x: 100, y: 100, width: 320, height: 240)
        XCTAssertEqual(FloatingPanelPlacement.correctedFrame(offScreen, visibleFrames: [screen], preferred: screen).midX, 500)
        XCTAssertEqual(FloatingPanelPlacement.correctedFrame(visible, visibleFrames: [screen], preferred: screen), visible)
    }

    func testActiveTaskGateRejectsStaleSameTaskAndAcceptsReplacementOrClear() {
        var gate = TaskUpdateGate()
        let current = NativeTask(id: "task-1", summary: "current", lifecycle: "implementing", recoveryRequired: false, recoveryReason: nil, version: 3, updatedAtMs: 3)
        let stale = NativeTask(id: "task-1", summary: "stale", lifecycle: "implementing", recoveryRequired: false, recoveryReason: nil, version: 2, updatedAtMs: 2)
        let replacement = NativeTask(id: "task-2", summary: "replacement", lifecycle: "reviewing", recoveryRequired: false, recoveryReason: nil, version: 1, updatedAtMs: 4)
        XCTAssertTrue(gate.apply(current))
        XCTAssertFalse(gate.apply(stale))
        XCTAssertTrue(gate.apply(replacement))
        XCTAssertEqual(gate.current?.id, "task-2")
        XCTAssertTrue(gate.apply(nil))
        XCTAssertNil(gate.current)
    }

    func testBridgeConnectionRejectsPreReconnectMessagesAndSubscribesOncePerGeneration() {
        var connection = BridgeConnectionState()
        let first = connection.began()
        XCTAssertTrue(connection.claimSubscription(for: first))
        XCTAssertFalse(connection.claimSubscription(for: first))
        let second = connection.began()
        XCTAssertFalse(connection.accepts(first))
        XCTAssertTrue(connection.accepts(second))
        XCTAssertTrue(connection.claimSubscription(for: second))
        XCTAssertFalse(connection.claimSubscription(for: second))
    }

    func testBridgeLineBufferWaitsForWholeStatusFrameBeforeDecoding() throws {
        var frame = Data(#"{"kind":"status","status":{"version":1,"sentinel":"ready","activeTask":null,"recoveryRequired":false,"codex":{"name":"Codex","installation":"available","runtime":"active","usage":"live","rateLimits":{"primary":{"usedPercent":39,"resetsAt":1787196921,"windowDurationMins":10080}}},"claude":{"name":"Claude Code","installation":"unavailable","runtime":"none","usage":"unavailable","rateLimits":null}}}"#.utf8)
        frame.append(10)
        let split = frame.index(frame.startIndex, offsetBy: 73)
        var buffer = BridgeLineBuffer()
        XCTAssertTrue(buffer.append(frame.prefix(upTo: split)).isEmpty)
        let lines = buffer.append(frame.suffix(from: split))
        XCTAssertEqual(lines.count, 1)
        guard case .status(let status) = try JSONDecoder().decode(BridgeMessage.self, from: lines[0]) else {
            return XCTFail("expected complete status frame")
        }
        XCTAssertEqual(CodexTrayTitle.make(status), "39%")
    }

    func testCodexTrayTitleUsesOnlyVerifiedPrimaryUsage() {
        let codex = NativeProviderStatus(name: "Codex", installation: "available", runtime: "active", usage: "live", rateLimits: ["primary": RateLimitWindow(usedPercent: 24.6, resetsAt: nil, windowDurationMins: nil)])
        let claude = NativeProviderStatus(name: "Claude Code", installation: "unavailable", runtime: "none", usage: "unavailable", rateLimits: nil)
        XCTAssertEqual(CodexTrayTitle.make(NativeStatus(version: 1, sentinel: "ready", activeTask: nil, recoveryRequired: false, codex: codex, claude: claude)), "25%")
        XCTAssertEqual(CodexTrayTitle.make(nil), "—")
        XCTAssertEqual(CodexTrayTitle.make(NativeStatus(version: 1, sentinel: "ready", activeTask: nil, recoveryRequired: false, codex: NativeProviderStatus(name: "Codex", installation: "available", runtime: "active", usage: "live", rateLimits: ["primary": RateLimitWindow(usedPercent: 101, resetsAt: nil, windowDurationMins: nil)]), claude: claude)), "—")
    }
}
