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

enum BridgeMessage: Decodable, Equatable {
    case activeTask(NativeTask?)
    case taskUpdate(NativeTask?)
    case subscribed
    case supervisorResult(Bool)
    case unavailable(String)

    private enum CodingKeys: String, CodingKey { case kind, task, ok, message }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        switch try values.decode(String.self, forKey: .kind) {
        case "active_task": self = .activeTask(try values.decodeIfPresent(NativeTask.self, forKey: .task))
        case "task_update": self = .taskUpdate(try values.decodeIfPresent(NativeTask.self, forKey: .task))
        case "subscribed": self = .subscribed
        case "supervisor_result": self = .supervisorResult(try values.decode(Bool.self, forKey: .ok))
        default: self = .unavailable(try values.decodeIfPresent(String.self, forKey: .message) ?? "Native bridge unavailable")
        }
    }
}

@MainActor
final class NativeBridge: ObservableObject {
    @Published private(set) var activeTask: NativeTask?
    @Published private(set) var availabilityMessage: String?

    private var process: Process?
    private var input: FileHandle?

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
        } catch {
            availabilityMessage = "Native bridge could not start."
        }
    }

    func decideFinalApproval(id: String, approve: Bool) {
        send(["kind": "supervisor", "command": "decide_final_approval", "approval_id": id, "approve": approve])
    }

    private func receive(_ message: BridgeMessage) {
        switch message {
        case .activeTask(let task), .taskUpdate(let task): activeTask = task
        case .unavailable(let message): availabilityMessage = message
        case .subscribed, .supervisorResult: break
        }
    }

    private func send(_ value: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: value), let input else { return }
        input.write(data)
        input.write(Data("\n".utf8))
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
