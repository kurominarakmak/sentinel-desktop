import SwiftUI

extension Notification.Name {
    static let sentinelQuickPromptShown = Notification.Name("sentinelQuickPromptShown")
}

struct QuickPromptView: View {
    @ObservedObject var bridge: NativeBridge
    let accepted: () -> Void
    @State private var prompt = ""
    @State private var provider = "codex"
    @FocusState private var promptFocused: Bool

    private var sending: Bool {
        if case .sending = bridge.taskSubmission { return true }
        return false
    }

    private var feedback: String? {
        switch bridge.taskSubmission {
        case .idle: return bridge.availabilityMessage
        case .sending: return "Sending task to Rust supervisor…"
        case .accepted: return "Task accepted."
        case .rejected(let message): return message
        }
    }

    var body: some View {
        SentinelPanel {
            VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                HStack { ProviderBadge(provider: "Sentinel"); Spacer(); Text("Quick Prompt").font(.headline) }
                if let repository = bridge.repositoryContext {
                    Label(repository, systemImage: "folder").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                } else {
                    Text("Repository unavailable").font(.caption).foregroundStyle(.orange)
                }
                HStack(spacing: SentinelTokens.compactSpacing) {
                    ForEach(bridge.providers) { capability in
                        Button(capability.label) { provider = capability.id }
                            .buttonStyle(.bordered)
                            .tint(provider == capability.id ? SentinelTokens.accent : .secondary)
                            .disabled(!capability.available || sending)
                            .accessibilityLabel("Use \(capability.label)")
                    }
                }
                TextField("Describe a task…", text: $prompt, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .lineLimit(3...6)
                    .focused($promptFocused)
                    .onSubmit(send)
                HStack {
                    Text("⌘↩ to send · Esc to hide").font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Button(sending ? "Sending…" : "Send", action: send)
                        .keyboardShortcut(.return, modifiers: .command)
                        .disabled(sending || prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || bridge.providers.first(where: { $0.id == provider })?.available != true)
                }
                if let feedback {
                    if sending {
                        Text(feedback).font(.caption).foregroundStyle(.secondary).accessibilityLabel(feedback)
                    } else {
                        Text(feedback).font(.caption).foregroundStyle(.red).accessibilityLabel(feedback)
                    }
                }
            }
        }
        .frame(width: SentinelTokens.panelWidth)
        .onAppear { promptFocused = true }
        .onReceive(NotificationCenter.default.publisher(for: .sentinelQuickPromptShown)) { _ in promptFocused = true }
        .onChange(of: bridge.taskSubmission) { _, state in
            if case .accepted = state {
                prompt = ""
                accepted()
            }
        }
    }

    private func send() {
        bridge.submitTask(provider: provider, summary: prompt.trimmingCharacters(in: .whitespacesAndNewlines), prompt: prompt)
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
