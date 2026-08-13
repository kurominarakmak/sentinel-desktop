import SwiftUI

struct QuickPromptView: View {
    @ObservedObject var bridge: NativeBridge
    @State private var prompt = ""

    var body: some View {
        SentinelPanel {
            VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                HStack { ProviderBadge(provider: "Sentinel"); Spacer(); Text("Quick Prompt").font(.headline) }
                TextField("Describe a task…", text: $prompt, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .lineLimit(3...6)
                HStack { Text(bridge.activeTask?.summary ?? "No active task").lineLimit(1).foregroundStyle(.secondary); Spacer(); Button("Send") {}.disabled(prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty) }
            }
        }.frame(width: SentinelTokens.panelWidth)
    }
}

struct AttentionWidgetView: View {
    @ObservedObject var bridge: NativeBridge
    let openDetail: () -> Void

    var body: some View {
        SentinelPanel {
            VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
                HStack { ProviderBadge(provider: "Sentinel"); Spacer(); StateBadge(state: bridge.activeTask?.lifecycle ?? "ready") }
                Text(bridge.activeTask?.summary ?? "Ready for a task").font(.headline).lineLimit(2)
                if let reason = bridge.activeTask?.recoveryReason { Text(reason.replacingOccurrences(of: "_", with: " ")).font(.caption).foregroundStyle(.red) }
                Button("Open Task Detail", action: openDetail).keyboardShortcut(.return)
            }
        }.frame(width: SentinelTokens.panelWidth)
    }
}

struct TaskDetailView: View {
    @ObservedObject var bridge: NativeBridge
    var body: some View {
        SentinelPanel {
            VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                HStack { ProviderBadge(provider: "Sentinel"); Spacer(); StateBadge(state: bridge.activeTask?.lifecycle ?? "ready") }
                Text(bridge.activeTask?.summary ?? "No active V3 task").font(.title3.weight(.semibold))
                Divider()
                Text("Activity, changes, validation, review, repair history, and approval remain sourced from the Rust V3 service bridge.")
                    .foregroundStyle(.secondary)
                Spacer()
            }
        }.padding()
    }
}

struct StatusView: View { var body: some View { SentinelPanel { Text("Sentinel status").frame(width: 260, alignment: .leading) } } }
struct SettingsView: View { var body: some View { SentinelPanel { Text("Sentinel settings").frame(width: 260, alignment: .leading) } } }
