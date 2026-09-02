import AppKit

/// Bounded, opt-in diagnostic output for native E2E navigation.  It is never
/// enabled in ordinary launches and intentionally records identifiers and
/// state only (never prompts, approval contexts, or provider data).
final class E2ERuntimeTrace {
    static let argument = "--e2e-diagnostics"
    static let pathEnvironmentKey = "SENTINEL_E2E_DIAGNOSTICS_PATH"

    private let enabled: Bool
    private let path: String?
    private let lock = NSLock()
    private var sequence: UInt64 = 0
    private var writes = 0
    private let maximumWrites = 300

    init(
        arguments: [String] = CommandLine.arguments,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        enabled = arguments.contains(Self.argument)
        path = environment[Self.pathEnvironmentKey]
        if enabled, let path {
            FileManager.default.createFile(atPath: path, contents: nil)
        }
    }

    func record(_ event: String, _ fields: [String: String] = [:]) {
        guard enabled else { return }
        lock.lock()
        defer { lock.unlock() }
        guard writes < maximumWrites else { return }
        writes += 1
        sequence &+= 1
        let renderedFields = fields
            .sorted { $0.key < $1.key }
            .map { "\($0.key)=\($0.value)" }
            .joined(separator: " ")
        let line = "sentinel-e2e-trace #\(sequence) \(event)\(renderedFields.isEmpty ? "" : " \(renderedFields)")\n"
        guard let data = line.data(using: .utf8) else { return }
        if let path, let handle = FileHandle(forWritingAtPath: path) {
            defer { try? handle.close() }
            _ = try? handle.seekToEnd()
            try? handle.write(contentsOf: data)
        } else {
            FileHandle.standardError.write(data)
        }
    }
}

enum NativeSurface: Hashable {
    case quickPrompt
    case attention
    case taskDetail
    case status
    case settings
}

/// Launch-only opt-in for native UI automation.  This deliberately accepts an
/// exact argument/environment value so ordinary launches have no extra UI.
struct E2EQuickPromptLaunchConfiguration: Equatable {
    static let argument = "--e2e-open-quick-prompt"
    static let environmentKey = "SENTINEL_E2E_OPEN_QUICK_PROMPT"

    let opensQuickPrompt: Bool

    init(
        arguments: [String] = CommandLine.arguments,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        opensQuickPrompt = arguments.contains(Self.argument) || environment[Self.environmentKey] == "1"
    }
}

/// Test-only opt-in that names a durable task but never changes it. The app
/// waits for the normal bridge snapshot, then presents the normal attention
/// panel only when that snapshot advertises the real pending approval action.
struct E2EApprovalLaunchConfiguration: Equatable {
    static let argument = "--e2e-open-approval"
    static let environmentKey = "SENTINEL_E2E_OPEN_APPROVAL_TASK"
    let taskID: String?

    init(arguments: [String] = CommandLine.arguments,
         environment: [String: String] = ProcessInfo.processInfo.environment) {
        if let index = arguments.firstIndex(of: Self.argument), arguments.indices.contains(index + 1) {
            taskID = arguments[index + 1]
        } else {
            taskID = environment[Self.environmentKey]
        }
    }
}

enum E2EQuickPromptLaunchAction: Equatable {
    case none
    case waitForBridge
    case presentQuickPrompt
}

/// Ensures a launch-time request is consumed once. The AppDelegate maps the
/// presentation action to its existing `showQuickPrompt()` controller path.
struct E2EQuickPromptLaunchCoordinator {
    private var waitingForBridge = false
    private var consumed = false

    mutating func request(
        configuration: E2EQuickPromptLaunchConfiguration,
        bridgeConnected: Bool
    ) -> E2EQuickPromptLaunchAction {
        guard configuration.opensQuickPrompt, !consumed, !waitingForBridge else { return .none }
        if bridgeConnected {
            consumed = true
            return .presentQuickPrompt
        }
        waitingForBridge = true
        return .waitForBridge
    }

    mutating func bridgeDidConnect() -> E2EQuickPromptLaunchAction {
        guard waitingForBridge, !consumed else { return .none }
        waitingForBridge = false
        consumed = true
        return .presentQuickPrompt
    }
}

/// Records native surface ownership independently from AppKit so repeated open
/// requests can be tested without creating windows.
struct NativeSurfaceRegistry {
    private(set) var owned: Set<NativeSurface> = []

    mutating func requestOpen(_ surface: NativeSurface) -> Bool {
        owned.insert(surface).inserted
    }
}

struct BridgeConnectionState {
    private(set) var generation: UInt64 = 0
    private var subscribedGeneration: UInt64?

    mutating func began() -> UInt64 {
        generation &+= 1
        subscribedGeneration = nil
        return generation
    }

    func accepts(_ messageGeneration: UInt64) -> Bool {
        messageGeneration == generation
    }

    mutating func claimSubscription(for messageGeneration: UInt64) -> Bool {
        guard accepts(messageGeneration), subscribedGeneration != messageGeneration else { return false }
        subscribedGeneration = messageGeneration
        return true
    }
}

struct BridgeLineBuffer {
    private var pending = Data()

    var pendingByteCount: Int { pending.count }

    mutating func append(_ chunk: Data) -> [Data] {
        pending.append(chunk)
        var lines: [Data] = []
        while let newline = pending.firstIndex(of: 10) {
            lines.append(pending.prefix(upTo: newline))
            pending.removeSubrange(...newline)
        }
        return lines
    }

    mutating func reset() { pending.removeAll(keepingCapacity: true) }
}

struct PreviousApplicationFocus {
    private(set) var processIdentifier: pid_t?

    init(processIdentifier: pid_t? = nil) {
        self.processIdentifier = processIdentifier
    }

    mutating func capture(frontmost: NSRunningApplication?, sentinelPID: pid_t) {
        guard let frontmost, frontmost.processIdentifier != sentinelPID, !frontmost.isTerminated else {
            return
        }
        processIdentifier = frontmost.processIdentifier
    }

    mutating func takeRestoreCandidate(runningProcessIdentifiers: Set<pid_t>, sentinelPID: pid_t) -> pid_t? {
        defer { processIdentifier = nil }
        guard let processIdentifier,
              processIdentifier != sentinelPID,
              runningProcessIdentifiers.contains(processIdentifier) else {
            return nil
        }
        return processIdentifier
    }
}

enum FloatingPanelPlacement {
    static let edgeInset: CGFloat = 18
    static let topInset: CGFloat = 12

    /// `visibleFrame` already excludes the menu bar and Dock. Anchor from its
    /// top-right so a compact utility panel grows down rather than around a
    /// center point.
    static func topRightFrame(size: NSSize, preferred: NSRect) -> NSRect {
        let size = NSSize(
            width: min(size.width, max(0, preferred.width - edgeInset * 2)),
            height: min(size.height, max(0, preferred.height - topInset - edgeInset))
        )
        return NSRect(
            x: preferred.maxX - edgeInset - size.width,
            y: preferred.maxY - topInset - size.height,
            width: size.width,
            height: size.height
        )
    }

    static func correctedFrame(_ frame: NSRect, visibleFrames: [NSRect], preferred: NSRect) -> NSRect {
        guard visibleFrames.contains(where: { $0.contains(frame) }) else {
            return topRightFrame(size: frame.size, preferred: preferred)
        }
        return frame
    }

    static func resizedFrameKeepingTop(_ frame: NSRect, height: CGFloat) -> NSRect {
        var resized = frame
        resized.origin.y = frame.maxY - height
        resized.size.height = height
        return resized
    }
}

/// Standard document/settings windows deliberately use a different policy
/// from the menu-bar assistant. Their initial placement is centered on the
/// relevant screen; AppKit retains the user-moved frame afterwards.
enum NativeWindowPlacement {
    static func initialFrame(size: NSSize, preferred: NSRect) -> NSRect {
        let fittingSize = NSSize(
            width: min(size.width, preferred.width),
            height: min(size.height, preferred.height)
        )
        return NSRect(
            x: preferred.midX - fittingSize.width / 2,
            y: preferred.midY - fittingSize.height / 2,
            width: fittingSize.width,
            height: fittingSize.height
        )
    }
}

/// Maps already-durable attention state to concise user-facing language. It
/// has no authority to alter the task, approval actions, or recovery policy.
enum NeedsAttentionPresentation {
    static func stateLabel(_ attention: NativeAttention) -> String {
        if attention.task.isReadyForHuman || attention.actions.approve { return "Ready for approval" }
        if attention.task.recoveryRequired { return "Recovery required" }
        if attention.displayState.lowercased().contains("blocked") || attention.task.lifecycle == "blocked" { return "Blocked" }
        return attention.displayState.replacingOccurrences(of: "_", with: " ").capitalized
    }

    static func opensExistingTaskDetail(taskID: String) -> String { taskID }
}

/// A model-first, provider-neutral view of runtime telemetry.  It never
/// guesses a provider from a model name and never turns unrelated activity
/// into usage.
struct ModelUsagePresentation: Equatable {
    struct Row: Identifiable, Equatable {
        let model: NativeProviderModel
        let usage: ModelUsageStatus?
        var id: String { model.id }
        var label: String { model.label }
        var detail: String {
            usage?.percentageLabel
                ?? (usage?.usageCapability == "authentication_required" ? "Authentication required" : "Usage unavailable")
        }
    }

    let rows: [Row]
    let sharedQuotas: [ModelUsageStatus]

    static func make(
        catalogs: [String: NativeProviderModelCatalog],
        selections: [APIProviderSelection],
        usage: [ModelUsageStatus]
    ) -> ModelUsagePresentation {
        var models = Dictionary(uniqueKeysWithValues: catalogs.values.flatMap(\.models).map { ($0.id, $0) })
        for selection in selections where models["\(selection.providerId):\(selection.modelId)"] == nil {
            models["\(selection.providerId):\(selection.modelId)"] = NativeProviderModel(
                providerId: selection.providerId, modelId: selection.modelId, displayName: nil,
                supportedReasoningEfforts: [], defaultReasoningEffort: nil, isDefault: false, availability: "available"
            )
        }
        let rows = models.values.sorted { $0.label.localizedStandardCompare($1.label) == .orderedAscending }.map { model in
            Row(model: model, usage: usage.first { $0.granularity == .model && $0.providerId == model.providerId && $0.modelId == model.modelId })
        }
        return ModelUsagePresentation(rows: rows, sharedQuotas: usage.filter { $0.granularity == .providerAccount })
    }

    func idleItem(preferred: APIProviderSelection?) -> String? {
        guard let preferred else { return nil }
        let row = rows.first { $0.model.providerId == preferred.providerId && $0.model.modelId == preferred.modelId }
        let source = usageForIdle(preferred)
        if let source, source.granularity == .providerAccount {
            if let percent = source.percentageLabel { return "✦ \(source.providerDisplayName) \(percent)" }
            if source.availability == "available" { return "✦ \(source.providerDisplayName) Ready" }
            return "✦ \(source.providerDisplayName)"
        }
        let name = row?.label ?? source?.modelDisplayName ?? source?.providerDisplayName
        guard let name else { return nil }
        if let percent = source?.percentageLabel {
            return "✦ \(name) \(percent)"
        }
        if source?.availability == "available" { return "✦ \(name) Ready" }
        return "✦ \(name)"
    }

    private func usageForIdle(_ selection: APIProviderSelection) -> ModelUsageStatus? {
        if let exact = rows.first(where: { $0.model.providerId == selection.providerId && $0.model.modelId == selection.modelId })?.usage { return exact }
        // A shared account quota is allowed in the compact item only when it
        // is the actual provider quota, never copied onto each of its models.
        return sharedQuotas.first { $0.providerId == selection.providerId }
    }
}

enum StatusBarPresentation {
    static func make(status: NativeStatus?, attention: NativeAttention?, idleTitle: String?, showDone: Bool = true) -> String {
        guard let status else { return "✦" }
        if status.recoveryRequired || status.activeTask?.lifecycle == "blocked" || attention?.task.recoveryRequired == true { return "✦ Attention" }
        if attention?.actions.approve == true || status.activeTask?.isReadyForHuman == true { return "✦ Approval" }
        if let task = status.activeTask {
            switch task.lifecycle.lowercased() {
            case "validating": return "✦ Testing…"
            case "reviewing": return "✦ Reviewing…"
            case "repairing": return "✦ Repairing…"
            case "integrating": return "✦ Integrating…"
            case "completed", "done", "complete": return showDone ? "✦ Done" : (idleTitle ?? "✦")
            case "implementing", "editing": return "✦ Editing…"
            default: return "✦ Working…"
            }
        }
        return idleTitle ?? "✦"
    }
}

struct CodexUsageRetryGate {
    private(set) var remainingAttempts = 3
    private(set) var scheduled = false

    mutating func observe(hasVerifiedUsage: Bool) -> Bool {
        if hasVerifiedUsage {
            remainingAttempts = 0
            scheduled = false
            return false
        }
        guard remainingAttempts > 0, !scheduled else { return false }
        remainingAttempts -= 1
        scheduled = true
        return true
    }

    mutating func fired() {
        scheduled = false
    }

    mutating func reset() {
        remainingAttempts = 3
        scheduled = false
    }
}
