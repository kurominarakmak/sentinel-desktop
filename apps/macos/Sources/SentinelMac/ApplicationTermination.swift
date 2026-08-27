import Foundation

/// Prevents repeated user/system quit requests from starting duplicate bridge
/// shutdowns or replying to AppKit more than once.
struct ApplicationTerminationGate {
    private var requested = false
    private var replied = false

    mutating func begin() -> Bool {
        guard !requested else { return false }
        requested = true
        return true
    }

    mutating func finish() -> Bool {
        guard requested, !replied else { return false }
        replied = true
        return true
    }
}
