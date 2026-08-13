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

enum BridgeMessage: Decodable, Equatable {
    case activeTask(NativeTask?)
    case taskUpdate(NativeTask?)
    case subscribed
    case capabilities(repository: String?, providers: [ProviderCapability])
    case attentionState(NativeAttention?)
    case attentionUpdate(NativeAttention?)
    case attentionActionResult(requestID: String, accepted: Bool, message: String?)
    case taskStartResult(requestID: String, accepted: Bool, task: NativeTask?, message: String?)
    case supervisorResult(Bool)
    case unavailable(String)

    private enum CodingKeys: String, CodingKey { case kind, task, ok, message, repository, providers, requestId, accepted, attention }

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

    private var process: Process?
    private var input: FileHandle?
    private var submissionGate = TaskSubmissionGate()
    private var attentionActionGate = AttentionActionGate()
    private var attentionUpdateGate = AttentionUpdateGate()

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
