import SwiftUI

enum SentinelTokens {
    static let panelWidth: CGFloat = 360
    static let cornerRadius: CGFloat = 14
    static let spacing: CGFloat = 10
    static let compactSpacing: CGFloat = 6
    static let border = Color.primary.opacity(0.12)
    static let surface = Color(nsColor: .windowBackgroundColor).opacity(0.92)
    static let accent = Color.accentColor
}

struct SentinelPanel<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        content
            .padding(16)
            .background(SentinelTokens.surface)
            .clipShape(RoundedRectangle(cornerRadius: SentinelTokens.cornerRadius, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: SentinelTokens.cornerRadius, style: .continuous).stroke(SentinelTokens.border))
    }
}

struct ProviderBadge: View {
    let provider: String

    var body: some View {
        Text(provider.isEmpty ? "Sentinel" : provider)
            .font(.caption.weight(.semibold))
            .foregroundStyle(SentinelTokens.accent)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(SentinelTokens.accent.opacity(0.12), in: Capsule())
    }
}

struct StateBadge: View {
    let state: String

    var body: some View {
        Text(state.replacingOccurrences(of: "_", with: " ").capitalized)
            .font(.caption2.weight(.medium))
            .foregroundStyle(.secondary)
    }
}
