import AppKit

enum NativeSurface: Hashable {
    case quickPrompt
    case attention
    case taskDetail
    case status
    case settings
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
    static func centeredFrame(size: NSSize, preferred: NSRect) -> NSRect {
        let size = NSSize(width: min(size.width, preferred.width), height: min(size.height, preferred.height))
        return NSRect(
            x: preferred.midX - size.width / 2,
            y: preferred.midY - size.height / 2,
            width: size.width,
            height: size.height
        )
    }

    static func correctedFrame(_ frame: NSRect, visibleFrames: [NSRect], preferred: NSRect) -> NSRect {
        guard !visibleFrames.contains(where: { $0.intersects(frame) }) else { return frame }
        return centeredFrame(size: frame.size, preferred: preferred)
    }
}

enum CodexTrayTitle {
    static func make(_ status: NativeStatus?) -> String {
        guard let usedPercent = status?.codex.rateLimits?["primary"]?.usedPercent,
              (0...100).contains(usedPercent) else { return "—" }
        return String(format: "%.0f%%", usedPercent)
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
