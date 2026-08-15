import SwiftUI

enum SentinelTokens {
    static let panelWidth: CGFloat = 360
    static let cornerRadius: CGFloat = 16
    static let controlRadius: CGFloat = 10
    static let spacing: CGFloat = 10
    static let compactSpacing: CGFloat = 6
    static let border = Color.primary.opacity(0.14)
    static let highlight = Color.white.opacity(0.16)
    static let opaqueSurface = Color(nsColor: .windowBackgroundColor)
    static let evidenceSurface = Color(nsColor: .textBackgroundColor).opacity(0.82)
    static let accent = Color.accentColor
}

enum SentinelPanelStyle: Equatable {
    case floating
    case window
}

enum SentinelGlassRenderingMode: Equatable {
    case nativeLiquidGlass
    case materialFallback
    case opaqueAccessible
}

enum SentinelGlassRenderingPolicy {
    static func mode(
        macOSMajorVersion: Int,
        nativeAPICompiled: Bool,
        reduceTransparency: Bool
    ) -> SentinelGlassRenderingMode {
        if reduceTransparency { return .opaqueAccessible }
        if macOSMajorVersion >= 26, nativeAPICompiled { return .nativeLiquidGlass }
        return .materialFallback
    }

    static var nativeAPICompiled: Bool {
        #if compiler(>=6.2)
        true
        #else
        false
        #endif
    }
}

struct SentinelPanel<Content: View>: View {
    var style: SentinelPanelStyle = .window
    @ViewBuilder var content: Content

    var body: some View {
        content
            .padding(16)
            .modifier(SentinelGlassSurface(style: style))
    }
}

private struct SentinelGlassSurface: ViewModifier {
    let style: SentinelPanelStyle
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    @ViewBuilder
    func body(content: Content) -> some View {
        switch SentinelGlassRenderingPolicy.mode(
            macOSMajorVersion: ProcessInfo.processInfo.operatingSystemVersion.majorVersion,
            nativeAPICompiled: SentinelGlassRenderingPolicy.nativeAPICompiled,
            reduceTransparency: reduceTransparency
        ) {
        case .opaqueAccessible:
            decorated(
                content.background(SentinelTokens.opaqueSurface, in: shape),
                shadowOpacity: 0.08
            )
        case .nativeLiquidGlass, .materialFallback:
            glassOrFallback(content)
        }
    }

    private var shape: RoundedRectangle {
        RoundedRectangle(cornerRadius: SentinelTokens.cornerRadius, style: .continuous)
    }

    @ViewBuilder
    private func glassOrFallback(_ content: Content) -> some View {
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            decorated(
                content.glassEffect(.regular, in: shape),
                shadowOpacity: style == .floating ? 0.18 : 0.10
            )
        } else {
            materialFallback(content)
        }
        #else
        materialFallback(content)
        #endif
    }

    @ViewBuilder
    private func materialFallback(_ content: Content) -> some View {
        if style == .floating {
            decorated(
                content.background(.ultraThinMaterial, in: shape),
                shadowOpacity: 0.18
            )
        } else {
            decorated(
                content.background(.thinMaterial, in: shape),
                shadowOpacity: 0.10
            )
        }
    }

    private func decorated<Surface: View>(_ surface: Surface, shadowOpacity: Double) -> some View {
        surface
            .clipShape(shape)
            .overlay(shape.stroke(SentinelTokens.border, lineWidth: 0.75))
            .overlay(shape.inset(by: 1).stroke(SentinelTokens.highlight, lineWidth: 0.5))
            .shadow(color: .black.opacity(shadowOpacity), radius: 18, y: 8)
    }
}

private struct SentinelEvidenceSurface: ViewModifier {
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency

    func body(content: Content) -> some View {
        content
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                reduceTransparency ? SentinelTokens.opaqueSurface : SentinelTokens.evidenceSurface,
                in: RoundedRectangle(cornerRadius: SentinelTokens.controlRadius, style: .continuous)
            )
            .overlay(
                RoundedRectangle(cornerRadius: SentinelTokens.controlRadius, style: .continuous)
                    .stroke(SentinelTokens.border, lineWidth: 0.5)
            )
    }
}

extension View {
    func sentinelEvidenceSurface() -> some View {
        modifier(SentinelEvidenceSurface())
    }

    @ViewBuilder
    func sentinelPrimaryButtonStyle() -> some View {
        #if compiler(>=6.2)
        if #available(macOS 26.0, *) {
            buttonStyle(.glassProminent)
        } else {
            buttonStyle(.borderedProminent)
        }
        #else
        buttonStyle(.borderedProminent)
        #endif
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
