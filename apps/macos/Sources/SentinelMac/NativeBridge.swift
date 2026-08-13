import Foundation

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
}

struct RateLimitWindow: Codable, Equatable {
    let usedPercent: Double
    let resetsAt: String?
    let windowDurationMins: Int?
}

struct NativeProviderStatus: Codable, Equatable {
    let name: String
    let installation: String
    let runtime: String
    let usage: String
    let rateLimits: [String: RateLimitWindow]?
}

struct NativeStatus: Codable, Equatable {
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
}

enum TaskSubmissionState: Equatable {
    case idle
    case sending
    case accepted(NativeTask)
    case rejected(String)
}

enum TaskSubmissionBegin: Equatable { case accepted(String), rejected(String) }

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
        guard update.version >= (current?.version ?? 0) else { return false }
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
    case capabilities(repository: String?, providers: [ProviderCapability])
    case attentionState(NativeAttention?)
    case attentionUpdate(NativeAttention?)
    case attentionActionResult(requestID: String, accepted: Bool, message: String?)
    case taskDetail(NativeTaskDetail)
    case status(NativeStatus)
    case statusUpdate(NativeStatus)
    case settings(NativeSettings)
    case taskStartResult(requestID: String, accepted: Bool, task: NativeTask?, message: String?)
    case supervisorResult(Bool)
    case unavailable(String)

    private enum CodingKeys: String, CodingKey { case kind, task, ok, message, repository, providers, requestId, accepted, attention, detail, status, settings }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(String.self, forKey: .kind) {
        case "active_task": self = .activeTask(try values.decodeIfPresent(NativeTask.self, forKey: .task))
        case "task_update": self = .taskUpdate(try values.decodeIfPresent(NativeTask.self, forKey: .task))
        case "subscribed": self = .subscribed
        case "capabilities": self = .capabilities(repository: try values.decodeIfPresent(String.self, forKey: .repository), providers: try values.decodeIfPresent([ProviderCapability].self, forKey: .providers) ?? [])
        case "attention_state": self = .attentionState(try values.decodeIfPresent(NativeAttention.self, forKey: .attention))
        case "attention_update": self = .attentionUpdate(try values.decodeIfPresent(NativeAttention.self, forKey: .attention))
        case "attention_action_result": self = .attentionActionResult(requestID: try values.decode(String.self, forKey: .requestId), accepted: try values.decode(Bool.self, forKey: .accepted), message: try values.decodeIfPresent(String.self, forKey: .message))
        case "task_detail": self = .taskDetail(try values.decode(NativeTaskDetail.self, forKey: .detail))
        case "status": self = .status(try values.decode(NativeStatus.self, forKey: .status))
        case "status_update": self = .statusUpdate(try values.decode(NativeStatus.self, forKey: .status))
        case "settings": self = .settings(try values.decode(NativeSettings.self, forKey: .settings))
        case "task_start_result": self = .taskStartResult(requestID: try values.decode(String.self, forKey: .requestId), accepted: try values.decode(Bool.self, forKey: .accepted), task: try values.decodeIfPresent(NativeTask.self, forKey: .task), message: try values.decodeIfPresent(String.self, forKey: .message))
        case "supervisor_result": self = .supervisorResult(try values.decode(Bool.self, forKey: .ok))
        default: self = .unavailable(try values.decodeIfPresent(String.self, forKey: .message) ?? "Native bridge unavailable")
        }
    }
}

@MainActor
final class NativeBridge: ObservableObject {
    @Published private(set) var activeTask: NativeTask?
    @Published private(set) var availabilityMessage: String?
    @Published private(set) var providers: [ProviderCapability] = []
    @Published private(set) var repositoryContext: String?
    @Published private(set) var taskSubmission = TaskSubmissionState.idle
    @Published private(set) var attention: NativeAttention?
    @Published private(set) var attentionActionMessage: String?
    @Published private(set) var attentionActionInFlight = false
    @Published private(set) var taskDetail: NativeTaskDetail?
    @Published private(set) var status: NativeStatus?
    @Published private(set) var settings: NativeSettings?

    private var process: Process?
    private var input: FileHandle?
    private var submissionGate = TaskSubmissionGate()
    private var attentionActionGate = AttentionActionGate()
    private var attentionUpdateGate = AttentionUpdateGate()
    private var detailUpdateGate = DetailUpdateGate()
    private var statusUpdateGate = StatusUpdateGate()
    private var settingsUpdateGate = SettingsUpdateGate()

    func start() {
        guard process == nil else { return }
        guard let executable = bridgeExecutable() else {
            availabilityMessage = "Native bridge is not bundled yet. Build sentinel-native-bridge for development."
            return
        }
        let process = Process()
        process.executableURL = executable
        process.arguments = [appDataDirectory().appending(path: "phase2.sqlite3").path()]
        let input = Pipe()
        let output = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            self.process = process
            self.input = input.fileHandleForWriting
            output.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                for line in String(decoding: data, as: UTF8.self).split(separator: "\n") {
                    guard let message = try? JSONDecoder().decode(BridgeMessage.self, from: Data(line.utf8)) else { continue }
                    DispatchQueue.main.async { self?.receive(message) }
                }
            }
            send(["kind": "subscribe"])
            send(["kind": "active_task"])
            send(["kind": "attention_state"])
            send(["kind": "status"])
            send(["kind": "capabilities"])
        } catch {
            availabilityMessage = "Native bridge could not start."
        }
    }

    func decideFinalApproval(id: String, approve: Bool) {
        send(["kind": "supervisor", "command": "decide_final_approval", "approval_id": id, "approve": approve])
    }

    func requestAttentionAction(_ action: String) {
        guard let attention else {
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
        var request: [String: Any] = ["kind": "attention_action", "request_id": requestID, "action": action, "task_id": attention.task.id]
        if (action == "approve" || action == "reject"), let approvalID = attention.actions.approvalID {
            request["approval_id"] = approvalID
        }
        guard send(request) else {
            _ = attentionActionGate.complete(requestID: requestID)
            attentionActionInFlight = false
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

    func submitTask(provider: String, summary: String, prompt: String) {
        let trimmed = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        let requestID = UUID().uuidString.lowercased()
        switch submissionGate.begin(prompt: trimmed, providerAvailable: providers.first(where: { $0.id == provider })?.available == true, requestID: requestID) {
        case .rejected(let message):
            taskSubmission = .rejected(message)
            return
        case .accepted: break
        }
        taskSubmission = .sending
        guard send(["kind": "start_task", "request_id": requestID, "provider": provider, "summary": summary, "prompt": trimmed]) else {
            submissionGate.rejectPending()
            taskSubmission = .rejected("Native bridge is unavailable.")
            return
        }
    }

    private func receive(_ message: BridgeMessage) {
        switch message {
        case .activeTask(let task), .taskUpdate(let task): activeTask = task
        case .attentionState(let attention), .attentionUpdate(let attention): apply(attention)
        case .taskDetail(let detail):
            guard detailUpdateGate.apply(detail) else { return }
            taskDetail = detailUpdateGate.current
        case .status(let status), .statusUpdate(let status):
            guard statusUpdateGate.apply(status) else { return }
            self.status = statusUpdateGate.current
        case .settings(let settings):
            guard settingsUpdateGate.apply(settings) else { return }
            self.settings = settingsUpdateGate.current
        case .attentionActionResult(let requestID, let accepted, let message):
            guard attentionActionGate.complete(requestID: requestID) else { return }
            attentionActionInFlight = false
            attentionActionMessage = accepted ? "Action accepted." : (message ?? "Action was rejected.")
        case .capabilities(let repository, let providers):
            repositoryContext = repository
            self.providers = providers
        case .taskStartResult(let requestID, let accepted, let task, let message):
            guard submissionGate.complete(requestID: requestID) else { return }
            if accepted, let task {
                activeTask = task
                taskSubmission = .accepted(task)
            } else {
                taskSubmission = .rejected(message ?? "Task submission was rejected.")
            }
        case .unavailable(let message): availabilityMessage = message
        case .subscribed, .supervisorResult: break
        }
    }

    private func apply(_ update: NativeAttention?) {
        guard attentionUpdateGate.apply(update) else { return }
        attention = attentionUpdateGate.current
        activeTask = attention?.task
        if let taskID = attention?.task.id { loadTaskDetail(taskID: taskID) }
        loadStatus()
    }

    @discardableResult
    private func send(_ value: [String: Any]) -> Bool {
        guard let data = try? JSONSerialization.data(withJSONObject: value), let input else { return false }
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
}
