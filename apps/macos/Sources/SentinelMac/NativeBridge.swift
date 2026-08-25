import Foundation

extension Notification.Name {
    static let sentinelAttentionActionAccepted = Notification.Name("sentinelAttentionActionAccepted")
}

struct NativeTask: Codable, Equatable {
    let id: String
    let summary: String
    let lifecycle: String
    let recoveryRequired: Bool
    let recoveryReason: String?
    let version: UInt64
    let updatedAtMs: Int64
}

struct ProviderCapability: Codable, Equatable, Identifiable {
    let id: String
    let label: String
    let available: Bool
}

enum QuickPromptRole: String, CaseIterable, Hashable {
    case implementer
    case reviewer
}

enum NativeWorkflowMode: String, Codable, CaseIterable, Equatable {
    case manual
    case autoIntegrate = "auto_integrate"
}

struct NativeProviderModel: Codable, Equatable, Identifiable {
    let providerId: String
    let modelId: String
    let displayName: String?
    let supportedReasoningEfforts: [String]
    let defaultReasoningEffort: String?
    let isDefault: Bool
    let availability: String

    var id: String { "\(providerId):\(modelId)" }
    var available: Bool { availability == "available" }
    var label: String { displayName ?? modelId }
}

struct NativeProviderModelCatalog: Codable, Equatable {
    let providerId: String
    let discoveryKind: String
    let models: [NativeProviderModel]
}

struct NativeModelDiscoveryError: Codable, Equatable {
    let code: String
    let message: String
}

struct NativeQuickPromptPreferences: Codable, Equatable {
    let implementer: APIProviderSelection
    let reviewer: APIProviderSelection
    let workflowMode: NativeWorkflowMode
}

struct NativeQuickPromptDefaults: Codable, Equatable {
    let implementerProviderId: String
    let reviewerProviderId: String
}

struct NativeTaskConfiguration: Codable, Equatable {
    let implementer: APIProviderSelection
    let reviewer: APIProviderSelection
    let workflowMode: NativeWorkflowMode
}

enum ModelDiscoveryPhase: Equatable {
    case idle
    case loading(providerID: String)
    case loaded(NativeProviderModelCatalog)
    case failed(providerID: String, code: String, message: String)
}

struct ModelDiscoveryGate {
    private(set) var pending: [QuickPromptRole: (requestID: String, providerID: String)] = [:]

    mutating func begin(role: QuickPromptRole, providerID: String, requestID: String) {
        pending[role] = (requestID, providerID)
    }

    mutating func apply(requestID: String, providerID: String) -> QuickPromptRole? {
        guard let match = pending.first(where: {
            $0.value.requestID == requestID && $0.value.providerID == providerID
        }) else { return nil }
        pending.removeValue(forKey: match.key)
        return match.key
    }

    mutating func failToSend(requestID: String) -> QuickPromptRole? {
        guard let match = pending.first(where: { $0.value.requestID == requestID }) else { return nil }
        pending.removeValue(forKey: match.key)
        return match.key
    }
}

enum QuickPromptSelectionValidation {
    static func availableModel(
        modelID: String,
        catalog: NativeProviderModelCatalog?
    ) -> NativeProviderModel? {
        catalog?.models.first(where: { $0.modelId == modelID && $0.available })
    }

    static func reviewerEffortIsValid(
        _ effort: String?,
        for model: NativeProviderModel
    ) -> Bool {
        if model.supportedReasoningEfforts.isEmpty { return effort == nil }
        return effort.map { model.supportedReasoningEfforts.contains($0) } ?? false
    }

    static func preferredReviewerEffort(for model: NativeProviderModel) -> String? {
        if let defaultEffort = model.defaultReasoningEffort,
           model.supportedReasoningEfforts.contains(defaultEffort) {
            return defaultEffort
        }
        return model.supportedReasoningEfforts.first
    }
}

struct AttentionActions: Codable, Equatable {
    let stop: Bool
    let approve: Bool
    let reject: Bool
    let approvalID: String?
}

struct NativeAttention: Codable, Equatable {
    let task: NativeTask
    let displayState: String
    let provider: String?
    let activity: String?
    let validation: String?
    let review: String?
    let actions: AttentionActions
}

struct DetailSession: Codable, Equatable { let provider: String; let sessionRef: String; let state: String; let updatedAtMs: Int64 }
struct DetailEvent: Codable, Equatable { let kind: String; let provider: String; let occurredAtMs: Int64; let payload: String }
struct DetailWorktree: Codable, Equatable { let repositoryRoot: String; let path: String; let branch: String; let baseCommit: String; let state: String }
struct DetailDiff: Codable, Equatable { let targetBranch: String; let targetAdvanced: Bool; let mergeReady: Bool; let summary: String; let conflicts: String }
struct DetailValidation: Codable, Equatable { let id: String; let profile: String; let check: String; let required: Bool; let state: String; let summary: String?; let updatedAtMs: Int64; let command: String?; let exitCode: Int?; let durationMs: Int?; let stdout: String?; let stderr: String?; let outcome: String? }
struct DetailFinding: Codable, Equatable { let id: String; let repairRoundID: String?; let severity: String; let disposition: String; let summary: String; let evidence: String }
struct DetailRepairRound: Codable, Equatable { let id: String; let round: Int; let state: String; let updatedAtMs: Int64 }

struct NativeTaskDetail: Codable, Equatable {
    let task: NativeTask
    let sessions: [DetailSession]
    let activity: [DetailEvent]
    let worktree: DetailWorktree?
    let diff: DetailDiff?
    let validations: [DetailValidation]
    let findings: [DetailFinding]
    let repairRounds: [DetailRepairRound]
    let finalApprovalPacket: String?
    let actions: AttentionActions
    var configuration: NativeTaskConfiguration? = nil
}

struct RateLimitWindow: Decodable, Equatable {
    let usedPercent: Double
    let resetsAt: String?
    let windowDurationMins: Int?

    init(usedPercent: Double, resetsAt: String?, windowDurationMins: Int?) {
        self.usedPercent = usedPercent
        self.resetsAt = resetsAt
        self.windowDurationMins = windowDurationMins
    }

    private enum CodingKeys: String, CodingKey { case usedPercent, resetsAt, windowDurationMins }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        usedPercent = try values.decode(Double.self, forKey: .usedPercent)
        windowDurationMins = try values.decodeIfPresent(Int.self, forKey: .windowDurationMins)
        if let text = try? values.decode(String.self, forKey: .resetsAt) {
            resetsAt = text
        } else if let seconds = try? values.decode(Double.self, forKey: .resetsAt) {
            resetsAt = ISO8601DateFormatter().string(from: Date(timeIntervalSince1970: seconds))
        } else {
            resetsAt = nil
        }
    }
}

struct NativeProviderStatus: Decodable, Equatable {
    let name: String
    let installation: String
    let runtime: String
    let usage: String
    let rateLimits: [String: RateLimitWindow]?

    init(name: String, installation: String, runtime: String, usage: String, rateLimits: [String: RateLimitWindow]?) {
        self.name = name
        self.installation = installation
        self.runtime = runtime
        self.usage = usage
        self.rateLimits = rateLimits
    }

    private enum CodingKeys: String, CodingKey { case name, installation, runtime, usage, rateLimits }
    private enum RateLimitKeys: String, CodingKey { case primary, secondary }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        installation = try values.decode(String.self, forKey: .installation)
        runtime = try values.decode(String.self, forKey: .runtime)
        usage = try values.decode(String.self, forKey: .usage)

        guard values.contains(.rateLimits), try !values.decodeNil(forKey: .rateLimits) else {
            rateLimits = nil
            return
        }
        let windows = try values.nestedContainer(keyedBy: RateLimitKeys.self, forKey: .rateLimits)
        var supported: [String: RateLimitWindow] = [:]
        if let primary = try windows.decodeIfPresent(RateLimitWindow.self, forKey: .primary) {
            supported[RateLimitKeys.primary.rawValue] = primary
        }
        if let secondary = try windows.decodeIfPresent(RateLimitWindow.self, forKey: .secondary) {
            supported[RateLimitKeys.secondary.rawValue] = secondary
        }
        rateLimits = supported.isEmpty ? nil : supported
    }
}

struct NativeStatus: Decodable, Equatable {
    let version: UInt64
    let sentinel: String
    let activeTask: NativeTask?
    let recoveryRequired: Bool
    let codex: NativeProviderStatus
    let claude: NativeProviderStatus
}

struct SettingsProvider: Codable, Equatable {
    let name: String
    let installation: String
    let executableOverride: String?
    let supportsExecutableOverride: Bool
    let authentication: String
}

struct SettingsProfileStep: Codable, Equatable {
    let name: String
    let kind: String
    let cwd: String
    let timeoutMs: UInt64
    let required: Bool
}

struct SettingsProfile: Codable, Equatable {
    let id: String
    let steps: [SettingsProfileStep]
}

struct NativeSettings: Codable, Equatable {
    let version: UInt64
    let globalShortcut: String
    let repository: String?
    let defaultProvider: String
    let codex: SettingsProvider
    let claude: SettingsProvider
    let validationProfiles: [SettingsProfile]
    let validationError: String?
    let providerSettings: APIProviderSettingsSnapshot?

    init(
        version: UInt64,
        globalShortcut: String,
        repository: String?,
        defaultProvider: String,
        codex: SettingsProvider,
        claude: SettingsProvider,
        validationProfiles: [SettingsProfile],
        validationError: String?,
        providerSettings: APIProviderSettingsSnapshot? = nil
    ) {
        self.version = version
        self.globalShortcut = globalShortcut
        self.repository = repository
        self.defaultProvider = defaultProvider
        self.codex = codex
        self.claude = claude
        self.validationProfiles = validationProfiles
        self.validationError = validationError
        self.providerSettings = providerSettings
    }
}

enum TaskSubmissionState: Equatable {
    case idle
    case sending
    case accepted(NativeTask)
    case rejected(String)
}

enum TaskSubmissionBegin: Equatable { case accepted(String), rejected(String) }

struct TaskUpdateGate {
    private(set) var current: NativeTask?

    mutating func apply(_ update: NativeTask?) -> Bool {
        guard let update else {
            current = nil
            return true
        }
        guard let current else {
            self.current = update
            return true
        }
        guard current.id != update.id || update.version >= current.version else { return false }
        self.current = update
        return true
    }
}

struct TaskSubmissionGate {
    private(set) var pendingRequestID: String?

    mutating func begin(prompt: String, providerAvailable: Bool, requestID: String) -> TaskSubmissionBegin {
        guard pendingRequestID == nil else { return .rejected("Task submission is already in progress.") }
        guard !prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return .rejected("Describe the task before sending.") }
        guard providerAvailable else { return .rejected("Selected provider is unavailable.") }
        pendingRequestID = requestID
        return .accepted(requestID)
    }

    mutating func complete(requestID: String) -> Bool {
        guard pendingRequestID == requestID else { return false }
        pendingRequestID = nil
        return true
    }

    mutating func rejectPending() { pendingRequestID = nil }
}

struct AttentionActionGate {
    private(set) var pendingRequestID: String?

    mutating func begin(requestID: String) -> Bool {
        guard pendingRequestID == nil else { return false }
        pendingRequestID = requestID
        return true
    }

    mutating func complete(requestID: String) -> Bool {
        guard pendingRequestID == requestID else { return false }
        pendingRequestID = nil
        return true
    }
}

struct SettingsMutationGate {
    private(set) var pendingRequestID: String?

    mutating func begin(requestID: String) -> Bool {
        guard pendingRequestID == nil else { return false }
        pendingRequestID = requestID
        return true
    }

    mutating func complete(requestID: String) -> Bool {
        guard pendingRequestID == requestID else { return false }
        pendingRequestID = nil
        return true
    }
}

struct AttentionUpdateGate {
    private(set) var current: NativeAttention?

    mutating func apply(_ update: NativeAttention?) -> Bool {
        guard let update else {
            current = nil
            return true
        }
        if let existing = current?.task, existing.id == update.task.id, update.task.version < existing.version { return false }
        current = update
        return true
    }
}

struct DetailUpdateGate {
    private(set) var current: NativeTaskDetail?

    mutating func apply(_ detail: NativeTaskDetail) -> Bool {
        if let current, current.task.id == detail.task.id, detail.task.version < current.task.version { return false }
        current = detail
        return true
    }
}

struct StatusUpdateGate {
    private(set) var current: NativeStatus?
    mutating func apply(_ update: NativeStatus) -> Bool {
        if let existing = current,
           update.version < existing.version,
           update.activeTask == nil || existing.activeTask?.id == update.activeTask?.id {
            return false
        }
        current = update
        return true
    }
}

struct SettingsUpdateGate {
    private(set) var current: NativeSettings?
    mutating func apply(_ update: NativeSettings) -> Bool {
        guard update.version >= (current?.version ?? 0) else { return false }
        current = update
        return true
    }
}

enum BridgeMessage: Decodable, Equatable {
    case activeTask(NativeTask?)
    case taskUpdate(NativeTask?)
    case subscribed
    case capabilities(repository: String?, providers: [ProviderCapability], preferences: NativeQuickPromptPreferences?, defaults: NativeQuickPromptDefaults?)
    case discoverModelsResult(requestID: String, providerID: String, catalog: NativeProviderModelCatalog?, error: NativeModelDiscoveryError?)
    case quickPromptPreferencesResult(requestID: String, accepted: Bool, message: String?)
    case attentionState(NativeAttention?)
    case attentionUpdate(NativeAttention?)
    case attentionActionResult(requestID: String, accepted: Bool, message: String?)
    case taskDetail(NativeTaskDetail)
    case status(NativeStatus)
    case statusUpdate(NativeStatus)
    case settings(NativeSettings)
    case settingsMutationResult(requestID: String, accepted: Bool, settings: NativeSettings?, message: String?)
    case taskStartResult(requestID: String, accepted: Bool, task: NativeTask?, message: String?)
    case runtimeDiagnostics
    case unavailable(String)

    private enum CodingKeys: String, CodingKey { case kind, task, message, repository, providers, requestId, providerId, accepted, attention, detail, status, settings, catalog, error, quickPromptPreferences, quickPromptDefaults }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(String.self, forKey: .kind) {
        case "active_task": self = .activeTask(try values.decodeIfPresent(NativeTask.self, forKey: .task))
        case "task_update": self = .taskUpdate(try values.decodeIfPresent(NativeTask.self, forKey: .task))
        case "subscribed": self = .subscribed
        case "capabilities": self = .capabilities(repository: try values.decodeIfPresent(String.self, forKey: .repository), providers: try values.decodeIfPresent([ProviderCapability].self, forKey: .providers) ?? [], preferences: try values.decodeIfPresent(NativeQuickPromptPreferences.self, forKey: .quickPromptPreferences), defaults: try values.decodeIfPresent(NativeQuickPromptDefaults.self, forKey: .quickPromptDefaults))
        case "discover_models_result": self = .discoverModelsResult(requestID: try values.decode(String.self, forKey: .requestId), providerID: try values.decode(String.self, forKey: .providerId), catalog: try values.decodeIfPresent(NativeProviderModelCatalog.self, forKey: .catalog), error: try values.decodeIfPresent(NativeModelDiscoveryError.self, forKey: .error))
        case "quick_prompt_preferences_result": self = .quickPromptPreferencesResult(requestID: try values.decode(String.self, forKey: .requestId), accepted: try values.decode(Bool.self, forKey: .accepted), message: try values.decodeIfPresent(String.self, forKey: .message))
        case "attention_state": self = .attentionState(try values.decodeIfPresent(NativeAttention.self, forKey: .attention))
        case "attention_update": self = .attentionUpdate(try values.decodeIfPresent(NativeAttention.self, forKey: .attention))
        case "attention_action_result": self = .attentionActionResult(requestID: try values.decode(String.self, forKey: .requestId), accepted: try values.decode(Bool.self, forKey: .accepted), message: try values.decodeIfPresent(String.self, forKey: .message))
        case "task_detail": self = .taskDetail(try values.decode(NativeTaskDetail.self, forKey: .detail))
        case "status": self = .status(try values.decode(NativeStatus.self, forKey: .status))
        case "status_update": self = .statusUpdate(try values.decode(NativeStatus.self, forKey: .status))
        case "settings": self = .settings(try values.decode(NativeSettings.self, forKey: .settings))
        case "settings_mutation_result": self = .settingsMutationResult(requestID: try values.decode(String.self, forKey: .requestId), accepted: try values.decode(Bool.self, forKey: .accepted), settings: try values.decodeIfPresent(NativeSettings.self, forKey: .settings), message: try values.decodeIfPresent(String.self, forKey: .message))
        case "task_start_result": self = .taskStartResult(requestID: try values.decode(String.self, forKey: .requestId), accepted: try values.decode(Bool.self, forKey: .accepted), task: try values.decodeIfPresent(NativeTask.self, forKey: .task), message: try values.decodeIfPresent(String.self, forKey: .message))
        case "runtime_diagnostics": self = .runtimeDiagnostics
        default: self = .unavailable(try values.decodeIfPresent(String.self, forKey: .message) ?? "Native bridge unavailable")
        }
    }
}

@MainActor
final class NativeBridge: ObservableObject {
    @Published private(set) var activeTask: NativeTask?
    @Published private(set) var availabilityMessage: String?
    @Published private(set) var providers: [ProviderCapability] = []
    @Published private(set) var modelDiscovery: [QuickPromptRole: ModelDiscoveryPhase] = [:]
    @Published private(set) var quickPromptPreferences: NativeQuickPromptPreferences?
    @Published private(set) var quickPromptDefaults: NativeQuickPromptDefaults?
    @Published private(set) var quickPromptPreferencesMessage: String?
    @Published private(set) var repositoryContext: String?
    @Published private(set) var taskSubmission = TaskSubmissionState.idle
    @Published private(set) var attention: NativeAttention?
    @Published private(set) var attentionActionMessage: String?
    @Published private(set) var attentionActionInFlight = false
    @Published private(set) var taskDetail: NativeTaskDetail?
    @Published private(set) var status: NativeStatus?
    @Published private(set) var settings: NativeSettings?
    @Published private(set) var settingsMutationMessage: String?
    @Published private(set) var settingsMutationInFlight = false
    @Published private(set) var bridgeConnected = false

    private var process: Process?
    private var input: FileHandle?
    private var output: FileHandle?
    private var connectionState = BridgeConnectionState()
    private var reconnectScheduled = false
    private var stopped = false
    private var outputBuffer = BridgeLineBuffer()
    private var submissionGate = TaskSubmissionGate()
    private var modelDiscoveryGate = ModelDiscoveryGate()
    private var preferencesSaveWorkItem: DispatchWorkItem?
    private var attentionActionGate = AttentionActionGate()
    private var attentionUpdateGate = AttentionUpdateGate()
    private var detailUpdateGate = DetailUpdateGate()
    private var statusUpdateGate = StatusUpdateGate()
    private var settingsUpdateGate = SettingsUpdateGate()
    private var settingsMutationGate = SettingsMutationGate()
    private var settingsMutationSuccessMessage = "Settings updated."
    private var activeTaskUpdateGate = TaskUpdateGate()
    private var codexUsageRetryGate = CodexUsageRetryGate()
    private var restoreFocusAfterAttentionAction = false

    func start() {
        guard !stopped, process == nil else { return }
        guard let executable = bridgeExecutable() else {
            availabilityMessage = "Native bridge is not bundled yet. Build sentinel-native-bridge for development."
            return
        }
        let generation = connectionState.began()
        codexUsageRetryGate.reset()
        let process = Process()
        process.executableURL = executable
        var arguments = [bridgeDatabasePath()]
        if CommandLine.arguments.contains(E2EQuickPromptLaunchConfiguration.argument) {
            arguments.append("--e2e-diagnostics")
        }
        process.arguments = arguments
        let input = Pipe()
        let output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            self.process = process
            self.input = input.fileHandleForWriting
            self.output = output.fileHandleForReading
            outputBuffer.reset()
            bridgeConnected = true
            availabilityMessage = nil
            process.terminationHandler = { [weak self] _ in
                DispatchQueue.main.async { self?.sidecarTerminated(generation: generation) }
            }
            output.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                DispatchQueue.main.async { self?.receivedOutput(data, generation: generation) }
            }
            refreshSnapshots(generation: generation)
            if CommandLine.arguments.contains(E2EQuickPromptLaunchConfiguration.argument) {
                _ = send(["kind": "runtime_diagnostics"])
            }
        } catch {
            bridgeConnected = false
            availabilityMessage = "Native bridge could not start."
            scheduleReconnect()
        }
    }

    func stop() {
        stopped = true
        reconnectScheduled = false
        preferencesSaveWorkItem?.cancel()
        preferencesSaveWorkItem = nil
        _ = connectionState.began()
        output?.readabilityHandler = nil
        input?.closeFile()
        input = nil
        output = nil
        if process?.isRunning == true {
            process?.terminate()
        }
        process = nil
        bridgeConnected = false
    }

    func requestAttentionAction(_ action: String, restoreFocusAfterAcceptance: Bool = false) {
        let task = attention?.task ?? taskDetail?.task
        let actions = attention?.actions ?? taskDetail?.actions
        guard let task else {
            attentionActionMessage = "There is no active task."
            return
        }
        let requestID = UUID().uuidString.lowercased()
        guard attentionActionGate.begin(requestID: requestID) else {
            attentionActionMessage = "An action is already in progress."
            return
        }
        attentionActionInFlight = true
        attentionActionMessage = nil
        restoreFocusAfterAttentionAction = restoreFocusAfterAcceptance
        var request: [String: Any] = ["kind": "attention_action", "request_id": requestID, "action": action, "task_id": task.id]
        if (action == "approve" || action == "reject"), let approvalID = actions?.approvalID {
            request["approval_id"] = approvalID
        }
        guard send(request) else {
            _ = attentionActionGate.complete(requestID: requestID)
            attentionActionInFlight = false
            restoreFocusAfterAttentionAction = false
            attentionActionMessage = "Native bridge is unavailable."
            return
        }
    }

    func retryReview(taskID: String) {
        let requestID = UUID().uuidString.lowercased()
        guard send(["kind": "retry_review", "request_id": requestID, "task_id": taskID]) else {
            attentionActionMessage = "Native bridge is unavailable."
            return
        }
    }

    func loadTaskDetail(taskID: String? = nil) {
        guard let taskID = taskID ?? attention?.task.id ?? activeTask?.id else { return }
        _ = send(["kind": "task_detail", "task_id": taskID])
    }

    func loadStatus() { _ = send(["kind": "status"]) }
    func loadSettings() { _ = send(["kind": "settings"]) }

    func discoverModels(providerID: String, role: QuickPromptRole) {
        let requestID = UUID().uuidString.lowercased()
        modelDiscoveryGate.begin(role: role, providerID: providerID, requestID: requestID)
        modelDiscovery[role] = .loading(providerID: providerID)
        guard send([
            "kind": "discover_models",
            "request_id": requestID,
            "provider_id": providerID,
        ]) else {
            guard let failedRole = modelDiscoveryGate.failToSend(requestID: requestID) else { return }
            modelDiscovery[failedRole] = .failed(
                providerID: providerID,
                code: "bridge_unavailable",
                message: "Provider unavailable"
            )
            return
        }
    }

    func saveQuickPromptPreferences(_ preferences: NativeQuickPromptPreferences) {
        preferencesSaveWorkItem?.cancel()
        let workItem = DispatchWorkItem { [weak self] in
            guard let self else { return }
            let requestID = UUID().uuidString.lowercased()
            let payload: [String: Any] = [
                "implementer": self.selectionPayload(preferences.implementer),
                "reviewer": self.selectionPayload(preferences.reviewer),
                "workflowMode": preferences.workflowMode.rawValue,
            ]
            guard self.send([
                "kind": "save_quick_prompt_preferences",
                "request_id": requestID,
                "preferences": payload,
            ]) else {
                self.quickPromptPreferencesMessage = "Preferences could not be saved."
                return
            }
            self.quickPromptPreferences = preferences
        }
        preferencesSaveWorkItem = workItem
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: workItem)
    }

    func setRepository(_ path: String) {
        let requestID = UUID().uuidString.lowercased()
        guard settingsMutationGate.begin(requestID: requestID) else {
            settingsMutationMessage = "A settings change is already in progress."
            return
        }
        settingsMutationInFlight = true
        settingsMutationSuccessMessage = "Repository updated."
        settingsMutationMessage = nil
        guard send(["kind": "set_repository", "request_id": requestID, "path": path]) else {
            _ = settingsMutationGate.complete(requestID: requestID)
            settingsMutationInFlight = false
            settingsMutationMessage = "Native bridge is unavailable."
            return
        }
    }

    func setAPIProviderEnabled(_ providerID: String, enabled: Bool) {
        mutateProviderSettings(
            ["action": "set_enabled", "provider_id": providerID, "enabled": enabled],
            success: "Provider updated."
        )
    }

    func setAPIProviderModel(_ providerID: String, modelID: String) {
        mutateProviderSettings(
            ["action": "set_model", "provider_id": providerID, "model_id": modelID],
            success: "Model updated."
        )
    }

    func setAPIProviderCredential(_ providerID: String, apiKey: String) {
        mutateProviderSettings(
            ["action": "set_credential", "provider_id": providerID, "api_key": apiKey],
            success: "API key stored securely in Keychain."
        )
    }

    func removeAPIProviderCredential(_ providerID: String) {
        mutateProviderSettings(
            ["action": "remove_credential", "provider_id": providerID],
            success: "API key removed from Keychain."
        )
    }

    func setWorkflowProviders(_ workflow: APIWorkflowProviderConfiguration) {
        guard let encoded = try? JSONEncoder().encode(workflow),
              let value = try? JSONSerialization.jsonObject(with: encoded) else {
            settingsMutationMessage = "Provider selection is invalid."
            return
        }
        mutateProviderSettings(
            ["action": "set_workflow", "workflow": value],
            success: "Workflow providers updated."
        )
    }

    func upsertCustomProvider(_ provider: APICustomProviderIntent) {
        guard let encoded = try? JSONEncoder().encode(provider),
              let value = try? JSONSerialization.jsonObject(with: encoded) else {
            settingsMutationMessage = "Custom provider configuration is invalid."
            return
        }
        mutateProviderSettings(
            ["action": "upsert_custom", "provider": value],
            success: "Custom provider updated."
        )
    }

    func removeCustomProvider(_ providerID: String) {
        mutateProviderSettings(
            ["action": "remove_custom", "provider_id": providerID],
            success: "Custom provider removed."
        )
    }

    private func mutateProviderSettings(_ mutation: [String: Any], success: String) {
        let requestID = UUID().uuidString.lowercased()
        guard settingsMutationGate.begin(requestID: requestID) else {
            settingsMutationMessage = "A settings change is already in progress."
            return
        }
        settingsMutationInFlight = true
        settingsMutationSuccessMessage = success
        settingsMutationMessage = nil
        guard send([
            "kind": "provider_settings_mutation",
            "request_id": requestID,
            "mutation": mutation,
        ]) else {
            _ = settingsMutationGate.complete(requestID: requestID)
            settingsMutationInFlight = false
            settingsMutationMessage = "Native bridge is unavailable."
            return
        }
    }

    func submitTask(configuration: NativeTaskConfiguration, summary: String, prompt: String) {
        let trimmed = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        let requestID = UUID().uuidString.lowercased()
        let providersAvailable = [
            configuration.implementer.providerId,
            configuration.reviewer.providerId,
        ].allSatisfy { providerID in
            providers.first(where: { $0.id == providerID })?.available == true
        }
        switch submissionGate.begin(prompt: trimmed, providerAvailable: providersAvailable, requestID: requestID) {
        case .rejected(let message):
            taskSubmission = .rejected(message)
            return
        case .accepted: break
        }
        taskSubmission = .sending
        guard send([
            "kind": "start_task",
            "request_id": requestID,
            "summary": summary,
            "prompt": trimmed,
            "implementer": selectionPayload(configuration.implementer),
            "reviewer": selectionPayload(configuration.reviewer),
            "workflow_mode": configuration.workflowMode.rawValue,
        ]) else {
            submissionGate.rejectPending()
            taskSubmission = .rejected("Native bridge is unavailable.")
            return
        }
    }

    private func selectionPayload(_ selection: APIProviderSelection) -> [String: Any] {
        var payload: [String: Any] = [
            "providerId": selection.providerId,
            "modelId": selection.modelId,
        ]
        if let effort = selection.reasoningEffort {
            payload["reasoningEffort"] = effort
        }
        return payload
    }

    private func receive(_ message: BridgeMessage, generation: UInt64) {
        guard connectionState.accepts(generation) else { return }
        switch message {
        case .activeTask(let task), .taskUpdate(let task): applyActiveTask(task)
        case .attentionState(let attention), .attentionUpdate(let attention): apply(attention)
        case .taskDetail(let detail):
            guard detailUpdateGate.apply(detail) else { return }
            taskDetail = detailUpdateGate.current
        case .status(let status), .statusUpdate(let status):
            guard statusUpdateGate.apply(status) else { return }
            self.status = statusUpdateGate.current
            scheduleCodexUsageRetryIfNeeded(status, generation: generation)
        case .settings(let settings):
            guard settingsUpdateGate.apply(settings) else { return }
            self.settings = settingsUpdateGate.current
        case .settingsMutationResult(let requestID, let accepted, let settings, let message):
            guard settingsMutationGate.complete(requestID: requestID) else { return }
            settingsMutationInFlight = false
            if accepted, let settings, settingsUpdateGate.apply(settings) {
                self.settings = settingsUpdateGate.current
                repositoryContext = settings.repository
                settingsMutationMessage = settingsMutationSuccessMessage
                _ = send(["kind": "capabilities"])
            } else {
                settingsMutationMessage = message ?? "Repository setting was rejected."
            }
        case .discoverModelsResult(let requestID, let providerID, let catalog, let error):
            guard let role = modelDiscoveryGate.apply(
                requestID: requestID,
                providerID: providerID
            ) else { return }
            if let catalog, catalog.providerId == providerID {
                modelDiscovery[role] = .loaded(catalog)
            } else {
                modelDiscovery[role] = .failed(
                    providerID: providerID,
                    code: error?.code ?? "invalid_response",
                    message: error?.message ?? "Provider returned invalid model data"
                )
            }
        case .runtimeDiagnostics:
            break
        case .quickPromptPreferencesResult(_, let accepted, let message):
            quickPromptPreferencesMessage = accepted
                ? nil
                : (message ?? "Preferences could not be saved.")
        case .attentionActionResult(let requestID, let accepted, let message):
            guard attentionActionGate.complete(requestID: requestID) else { return }
            attentionActionInFlight = false
            attentionActionMessage = accepted ? "Action accepted." : (message ?? "Action was rejected.")
            if accepted {
                NotificationCenter.default.post(
                    name: .sentinelAttentionActionAccepted,
                    object: restoreFocusAfterAttentionAction
                )
            }
            restoreFocusAfterAttentionAction = false
        case .capabilities(let repository, let providers, let preferences, let defaults):
            repositoryContext = repository
            self.providers = providers
            quickPromptPreferences = preferences
            quickPromptDefaults = defaults
        case .taskStartResult(let requestID, let accepted, let task, let message):
            guard submissionGate.complete(requestID: requestID) else { return }
            if accepted, let task {
                applyActiveTask(task)
                taskSubmission = .accepted(task)
            } else {
                taskSubmission = .rejected(message ?? "Task submission was rejected.")
            }
        case .unavailable(let message): availabilityMessage = String(message.prefix(240))
        case .subscribed: break
        }
    }

    private func apply(_ update: NativeAttention?) {
        guard attentionUpdateGate.apply(update) else { return }
        attention = attentionUpdateGate.current
        applyActiveTask(attention?.task)
        if let taskID = attention?.task.id { loadTaskDetail(taskID: taskID) }
        loadStatus()
    }

    private func applyActiveTask(_ update: NativeTask?) {
        guard activeTaskUpdateGate.apply(update) else { return }
        activeTask = activeTaskUpdateGate.current
        if let taskID = update?.id, taskDetail?.task.id != taskID {
            loadTaskDetail(taskID: taskID)
        }
    }

    private func receivedMalformedMessage(generation: UInt64) {
        guard connectionState.accepts(generation) else { return }
        availabilityMessage = "Native bridge returned an invalid response."
    }

    private func receivedOutput(_ data: Data, generation: UInt64) {
        guard connectionState.accepts(generation) else { return }
        for line in outputBuffer.append(data) where !line.isEmpty {
            guard let message = try? JSONDecoder().decode(BridgeMessage.self, from: line) else {
                receivedMalformedMessage(generation: generation)
                continue
            }
            receive(message, generation: generation)
        }
    }

    private func sidecarTerminated(generation: UInt64) {
        guard connectionState.accepts(generation) else { return }
        input = nil
        output?.readabilityHandler = nil
        output = nil
        outputBuffer.reset()
        process = nil
        bridgeConnected = false
        availabilityMessage = "Native bridge disconnected. Reconnecting…"
        if case .sending = taskSubmission {
            submissionGate.rejectPending()
            taskSubmission = .rejected("Native bridge disconnected before task submission was confirmed.")
        }
        if attentionActionInFlight {
            attentionActionInFlight = false
            attentionActionMessage = "Native bridge disconnected before the action was confirmed."
        }
        restoreFocusAfterAttentionAction = false
        if settingsMutationInFlight {
            settingsMutationInFlight = false
            settingsMutationMessage = "Native bridge disconnected before the setting was confirmed."
        }
        attentionActionGate = AttentionActionGate()
        settingsMutationGate = SettingsMutationGate()
        resetUpdateGates()
        scheduleReconnect()
    }

    private func scheduleReconnect() {
        guard !stopped, !reconnectScheduled, bridgeExecutable() != nil else { return }
        reconnectScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard let self else { return }
            self.reconnectScheduled = false
            if !self.stopped { self.start() }
        }
    }

    private func resetUpdateGates() {
        modelDiscoveryGate = ModelDiscoveryGate()
        modelDiscovery = [:]
        attentionUpdateGate = AttentionUpdateGate()
        detailUpdateGate = DetailUpdateGate()
        statusUpdateGate = StatusUpdateGate()
        settingsUpdateGate = SettingsUpdateGate()
        activeTaskUpdateGate = TaskUpdateGate()
        codexUsageRetryGate.reset()
    }

    private func scheduleCodexUsageRetryIfNeeded(_ status: NativeStatus, generation: UInt64) {
        let verified = status.codex.rateLimits?["primary"].map {
            (0...100).contains($0.usedPercent)
        } == true
        guard codexUsageRetryGate.observe(hasVerifiedUsage: verified) else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
            guard let self, self.connectionState.accepts(generation), self.bridgeConnected else {
                return
            }
            self.codexUsageRetryGate.fired()
            self.loadStatus()
        }
    }

    private func refreshSnapshots(generation: UInt64) {
        guard connectionState.claimSubscription(for: generation) else { return }
        _ = send(["kind": "subscribe"])
        _ = send(["kind": "active_task"])
        _ = send(["kind": "attention_state"])
        _ = send(["kind": "status"])
        _ = send(["kind": "settings"])
        _ = send(["kind": "capabilities"])
    }

    @discardableResult
    private func send(_ value: [String: Any]) -> Bool {
        guard process?.isRunning == true,
              let data = try? JSONSerialization.data(withJSONObject: value),
              let input else { return false }
        input.write(data)
        input.write(Data("\n".utf8))
        return true
    }

    private func bridgeExecutable() -> URL? {
        if let override = ProcessInfo.processInfo.environment["SENTINEL_NATIVE_BRIDGE"] { return URL(fileURLWithPath: override) }
        return Bundle.main.url(forAuxiliaryExecutable: "sentinel-native-bridge")
    }

    private func appDataDirectory() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let directory = base.appending(path: "dev.agent-sentinel.spike")
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    private func bridgeDatabasePath() -> String {
        appDataDirectory()
            .appending(path: "phase2.sqlite3")
            .path(percentEncoded: false)
    }
}
