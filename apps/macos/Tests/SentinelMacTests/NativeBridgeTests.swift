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

    func testDecodesDynamicModelCatalogAndExactReasoningCapabilities() throws {
        let data = Data(#"{"kind":"discover_models_result","requestId":"models-1","providerId":"codex","catalog":{"providerId":"codex","discoveryKind":"enumerated","models":[{"providerId":"codex","modelId":"runtime-model","displayName":"Runtime Model","supportedReasoningEfforts":["medium","ultra"],"defaultReasoningEffort":"medium","isDefault":true,"availability":"available"}]}}"#.utf8)
        guard case .discoverModelsResult(let requestID, let providerID, let catalog, let error) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected model discovery result")
        }
        XCTAssertEqual(requestID, "models-1")
        XCTAssertEqual(providerID, "codex")
        XCTAssertNil(error)
        XCTAssertEqual(catalog?.models.first?.modelId, "runtime-model")
        XCTAssertEqual(catalog?.models.first?.supportedReasoningEfforts, ["medium", "ultra"])
    }

    func testDecodesTypedDiscoveryCommandFailureWithoutLosingSafeDetail() throws {
        let data = Data(#"{"kind":"discover_models_result","requestId":"models-omp","providerId":"omp","error":{"code":"discovery_command_failed","message":"OMP model discovery command failed (exit 17): authentication missing"}}"#.utf8)
        guard case .discoverModelsResult(_, let providerID, let catalog, let error) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected discovery failure")
        }
        XCTAssertEqual(providerID, "omp")
        XCTAssertNil(catalog)
        XCTAssertEqual(error?.code, "discovery_command_failed")
        XCTAssertEqual(error?.message, "OMP model discovery command failed (exit 17): authentication missing")
    }

    func testBridgeDatabasePathUsesDecodedFilesystemPath() {
        let url = URL(string: "file:///Users/test/Library/Application%20Support/dev.agent-sentinel.spike/phase2.sqlite3")!
        XCTAssertEqual(
            url.path(percentEncoded: false),
            "/Users/test/Library/Application Support/dev.agent-sentinel.spike/phase2.sqlite3"
        )
    }

    func testDiscoveryGateRejectsStaleProviderResponseAndKeepsRolesIndependent() {
        var gate = ModelDiscoveryGate()
        gate.begin(role: .implementer, providerID: "omp", requestID: "impl-old")
        gate.begin(role: .implementer, providerID: "codex", requestID: "impl-new")
        gate.begin(role: .reviewer, providerID: "omp", requestID: "review")
        XCTAssertNil(gate.apply(requestID: "impl-old", providerID: "omp"))
        XCTAssertEqual(gate.apply(requestID: "review", providerID: "omp"), .reviewer)
        XCTAssertEqual(gate.apply(requestID: "impl-new", providerID: "codex"), .implementer)
    }

    func testUnavailableSavedModelAndUnsupportedEffortRemainInvalid() {
        let model = NativeProviderModel(
            providerId: "codex",
            modelId: "runtime-model",
            displayName: nil,
            supportedReasoningEfforts: ["low", "high"],
            defaultReasoningEffort: "high",
            isDefault: true,
            availability: "available"
        )
        let catalog = NativeProviderModelCatalog(
            providerId: "codex",
            discoveryKind: "enumerated",
            models: [model]
        )
        XCTAssertNil(QuickPromptSelectionValidation.availableModel(
            modelID: "saved-but-gone",
            catalog: catalog
        ))
        XCTAssertFalse(QuickPromptSelectionValidation.reviewerEffortIsValid("medium", for: model))
        XCTAssertTrue(QuickPromptSelectionValidation.reviewerEffortIsValid("high", for: model))
    }

    func testChangingReviewerModelRecomputesEffortFromItsDynamicCapabilities() {
        let first = NativeProviderModel(
            providerId: "codex",
            modelId: "runtime-one",
            displayName: nil,
            supportedReasoningEfforts: ["low", "high"],
            defaultReasoningEffort: "high",
            isDefault: true,
            availability: "available"
        )
        let second = NativeProviderModel(
            providerId: "codex",
            modelId: "runtime-two",
            displayName: nil,
            supportedReasoningEfforts: ["future-effort"],
            defaultReasoningEffort: nil,
            isDefault: false,
            availability: "available"
        )
        let unsupported = NativeProviderModel(
            providerId: "claude_code",
            modelId: "cli-owned",
            displayName: nil,
            supportedReasoningEfforts: [],
            defaultReasoningEffort: nil,
            isDefault: true,
            availability: "available"
        )
        XCTAssertEqual(QuickPromptSelectionValidation.preferredReviewerEffort(for: first), "high")
        XCTAssertEqual(QuickPromptSelectionValidation.preferredReviewerEffort(for: second), "future-effort")
        XCTAssertNil(QuickPromptSelectionValidation.preferredReviewerEffort(for: unsupported))
    }

    func testCapabilitiesRestoreIndependentPreferencesAndManualWorkflow() throws {
        let data = Data(#"{"kind":"capabilities","repository":"/repo","providers":[{"id":"omp","label":"OMP","available":true},{"id":"codex","label":"Codex","available":true}],"quickPromptPreferences":{"implementer":{"providerId":"omp","modelId":"runtime-a"},"reviewer":{"providerId":"codex","modelId":"runtime-b","reasoningEffort":"high"},"workflowMode":"manual"},"quickPromptDefaults":{"implementerProviderId":"codex","reviewerProviderId":"claude_code"}}"#.utf8)
        guard case .capabilities(_, _, let preferences, let defaults) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected capabilities")
        }
        XCTAssertEqual(preferences?.implementer.modelId, "runtime-a")
        XCTAssertEqual(preferences?.reviewer.modelId, "runtime-b")
        XCTAssertEqual(preferences?.reviewer.reasoningEffort, "high")
        XCTAssertEqual(preferences?.workflowMode, .manual)
        XCTAssertEqual(defaults?.reviewerProviderId, "claude_code")
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
        let data = Data(#"{"kind":"task_detail","detail":{"task":{"id":"task-1","summary":"Ship it","lifecycle":"ready_for_human","recoveryRequired":false,"recoveryReason":null,"version":4,"updatedAtMs":9},"configuration":{"implementer":{"providerId":"omp","modelId":"historical-implementation"},"reviewer":{"providerId":"codex","modelId":"historical-review","reasoningEffort":"medium"},"workflowMode":"auto_integrate"},"sessions":[{"provider":"Codex","sessionRef":"thread-1","state":"active","updatedAtMs":8}],"activity":[{"kind":"task_prepared","provider":"Codex","occurredAtMs":1,"payload":"{}"},{"kind":"review_reported","provider":"Claude Code","occurredAtMs":2,"payload":"{}"}],"worktree":{"repositoryRoot":"/repo","path":"/worktree","branch":"agent/task","baseCommit":"abc","state":"ready"},"diff":{"targetBranch":"main","targetAdvanced":true,"mergeReady":false,"summary":"{\"filesChanged\":2}","conflicts":"[]"},"validations":[{"id":"check","profile":"test","check":"unit","required":true,"state":"failed","summary":"exit 1","updatedAtMs":3,"command":"[\"cargo\",\"test\"]","exitCode":1,"durationMs":12,"stdout":"safe output","stderr":"redacted output","outcome":"failed"}],"findings":[{"id":"finding","repairRoundId":"round-1","severity":"blocker","disposition":"confirmed_blocking","summary":"Fix me","evidence":"{\"file\":\"src/a.rs\",\"line\":3}"}],"repairRounds":[{"id":"round-1","round":1,"state":"completed","updatedAtMs":4}],"finalApprovalPacket":"{\"unresolved_risks\":[\"risk\"]}","actions":{"stop":false,"approve":true,"reject":true,"approvalID":"approval-1"}}}"#.utf8)
        guard case .taskDetail(let detail) = try JSONDecoder().decode(BridgeMessage.self, from: data) else { return XCTFail("expected task detail") }
        XCTAssertEqual(detail.activity.map(\.occurredAtMs), [1, 2])
        XCTAssertEqual(detail.worktree?.branch, "agent/task")
        XCTAssertEqual(detail.validations.first?.exitCode, 1)
        XCTAssertEqual(detail.findings.first?.severity, "blocker")
        XCTAssertEqual(detail.repairRounds.first?.round, 1)
        XCTAssertEqual(detail.actions.approvalID, "approval-1")
        XCTAssertEqual(detail.configuration?.implementer.modelId, "historical-implementation")
        XCTAssertEqual(detail.configuration?.reviewer.reasoningEffort, "medium")
        XCTAssertEqual(detail.configuration?.workflowMode, .autoIntegrate)
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
        let data = Data(#"{"kind":"status","status":{"version":1,"sentinel":"ready","activeTask":null,"recoveryRequired":false,"codex":{"name":"Codex","installation":"available","runtime":"active","usage":"live","rateLimits":{"credits":{"balance":"0","hasCredits":false,"unlimited":false},"individualLimit":null,"limitId":"codex","planType":"plus","primary":{"usedPercent":39,"resetsAt":1787196921,"windowDurationMins":10080},"secondary":null,"spendControlReached":false}},"claude":{"name":"Claude Code","installation":"unavailable","runtime":"none","usage":"unavailable","rateLimits":null}}}"#.utf8)
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

    func testRepositoryMutationDecodesConfirmedSettingsAndGatePreventsDuplicates() throws {
        let data = Data(#"{"kind":"settings_mutation_result","requestId":"setting-1","accepted":true,"settings":{"version":9,"globalShortcut":"Command+Shift+Space","repository":"/repo","defaultProvider":"codex","codex":{"name":"Codex","installation":"available","executableOverride":null,"supportsExecutableOverride":false,"authentication":"CLI-owned"},"claude":{"name":"Claude Code","installation":"unavailable","executableOverride":null,"supportsExecutableOverride":false,"authentication":"unavailable"},"validationProfiles":[],"validationError":null}}"#.utf8)
        guard case .settingsMutationResult(let requestID, let accepted, let settings, let message) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected settings mutation")
        }
        XCTAssertEqual(requestID, "setting-1")
        XCTAssertTrue(accepted)
        XCTAssertEqual(settings?.repository, "/repo")
        XCTAssertNil(message)

        var gate = SettingsMutationGate()
        XCTAssertTrue(gate.begin(requestID: "setting-1"))
        XCTAssertFalse(gate.begin(requestID: "setting-2"))
        XCTAssertFalse(gate.complete(requestID: "stale"))
        XCTAssertTrue(gate.complete(requestID: "setting-1"))
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

    func testNormalLaunchDoesNotRequestQuickPrompt() {
        var coordinator = E2EQuickPromptLaunchCoordinator()
        let normal = E2EQuickPromptLaunchConfiguration(arguments: ["Agent Sentinel"], environment: [:])
        XCTAssertFalse(normal.opensQuickPrompt)
        XCTAssertEqual(coordinator.request(configuration: normal, bridgeConnected: true), .none)
        XCTAssertEqual(coordinator.bridgeDidConnect(), .none)
    }

    func testE2ELaunchArgumentPresentsExistingQuickPromptOnlyAfterBridgeIsReady() {
        var coordinator = E2EQuickPromptLaunchCoordinator()
        let configuration = E2EQuickPromptLaunchConfiguration(
            arguments: ["Agent Sentinel", E2EQuickPromptLaunchConfiguration.argument],
            environment: [:]
        )
        XCTAssertTrue(configuration.opensQuickPrompt)
        XCTAssertEqual(coordinator.request(configuration: configuration, bridgeConnected: false), .waitForBridge)
        XCTAssertEqual(coordinator.bridgeDidConnect(), .presentQuickPrompt)
    }

    func testE2ELaunchEnvironmentPresentsAndRepeatedRequestsReuseTheExistingController() {
        var coordinator = E2EQuickPromptLaunchCoordinator()
        let configuration = E2EQuickPromptLaunchConfiguration(
            arguments: ["Agent Sentinel"],
            environment: [E2EQuickPromptLaunchConfiguration.environmentKey: "1"]
        )
        XCTAssertEqual(coordinator.request(configuration: configuration, bridgeConnected: true), .presentQuickPrompt)
        XCTAssertEqual(coordinator.request(configuration: configuration, bridgeConnected: true), .none)
        XCTAssertEqual(coordinator.bridgeDidConnect(), .none)

        var registry = NativeSurfaceRegistry()
        XCTAssertTrue(registry.requestOpen(.quickPrompt))
        XCTAssertFalse(registry.requestOpen(.quickPrompt))
    }

    func testOmittingE2ELaunchFlagRestoresNormalBehaviorWithoutCreatingATask() {
        var coordinator = E2EQuickPromptLaunchCoordinator()
        let configuration = E2EQuickPromptLaunchConfiguration(
            arguments: ["Agent Sentinel"],
            environment: [E2EQuickPromptLaunchConfiguration.environmentKey: "0"]
        )
        XCTAssertEqual(coordinator.request(configuration: configuration, bridgeConnected: true), .none)
        XCTAssertEqual(TaskSubmissionState.idle, .idle)
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

    func testMissingCodexUsageRetriesAreBoundedAndStopAfterVerifiedSnapshot() {
        var gate = CodexUsageRetryGate()
        XCTAssertTrue(gate.observe(hasVerifiedUsage: false))
        XCTAssertFalse(gate.observe(hasVerifiedUsage: false))
        gate.fired()
        XCTAssertTrue(gate.observe(hasVerifiedUsage: false))
        gate.fired()
        XCTAssertTrue(gate.observe(hasVerifiedUsage: false))
        gate.fired()
        XCTAssertFalse(gate.observe(hasVerifiedUsage: false))

        gate.reset()
        XCTAssertFalse(gate.observe(hasVerifiedUsage: true))
        XCTAssertEqual(gate.remainingAttempts, 0)
    }

    func testStatusGateAcceptsNewTaskEvenWhenItsVersionRestartsLower() {
        let provider = NativeProviderStatus(name: "Codex", installation: "available", runtime: "active", usage: "live", rateLimits: nil)
        let old = NativeStatus(version: 9, sentinel: "failed", activeTask: NativeTask(id: "old", summary: "Old", lifecycle: "failed", recoveryRequired: false, recoveryReason: nil, version: 9, updatedAtMs: 9), recoveryRequired: false, codex: provider, claude: provider)
        let replacement = NativeStatus(version: 1, sentinel: "implementing", activeTask: NativeTask(id: "new", summary: "New", lifecycle: "implementing", recoveryRequired: false, recoveryReason: nil, version: 1, updatedAtMs: 10), recoveryRequired: false, codex: provider, claude: provider)
        var gate = StatusUpdateGate()
        XCTAssertTrue(gate.apply(old))
        XCTAssertTrue(gate.apply(replacement))
        XCTAssertEqual(gate.current?.activeTask?.id, "new")
    }
}
