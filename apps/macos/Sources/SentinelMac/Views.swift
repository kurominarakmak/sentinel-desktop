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
                if let attention = bridge.attention {
                    HStack {
                        ProviderBadge(provider: attention.provider ?? "Sentinel")
                        Spacer()
                        StateBadge(state: attention.displayState)
                    }
                    Text(attention.task.summary).font(.headline).lineLimit(2)
                    if let activity = attention.activity {
                        Label(activity.replacingOccurrences(of: "_", with: " "), systemImage: "waveform.path.ecg")
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    HStack(spacing: SentinelTokens.compactSpacing) {
                        if let validation = attention.validation { StateBadge(state: "validation \(validation)") }
                        if let review = attention.review { Text(review).font(.caption2.weight(.semibold)).foregroundStyle(.orange) }
                    }
                    if attention.task.recoveryRequired || attention.displayState == "failed" {
                        Text(attention.task.recoveryReason?.replacingOccurrences(of: "_", with: " ") ?? "Task requires attention.")
                            .font(.caption).foregroundStyle(.red).lineLimit(2)
                    }
                    actions(attention.actions)
                } else {
                    HStack { ProviderBadge(provider: "Sentinel"); Spacer(); StateBadge(state: "ready") }
                    Text("No active task").font(.headline)
                    Text(bridge.availabilityMessage ?? "Use ⌘⇧Space to create a task.").font(.caption).foregroundStyle(.secondary)
                    Button("Open Task Detail", action: openDetail)
                }
                if let message = bridge.attentionActionMessage {
                    if bridge.attentionActionInFlight {
                        Text(message).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    } else {
                        Text(message).font(.caption).foregroundStyle(.red).lineLimit(2)
                    }
                }
            }
        }
        .frame(minWidth: SentinelTokens.panelWidth, idealWidth: SentinelTokens.panelWidth, maxWidth: SentinelTokens.panelWidth, minHeight: 180, maxHeight: 260)
    }

    @ViewBuilder
    private func actions(_ actions: AttentionActions) -> some View {
        HStack(spacing: SentinelTokens.compactSpacing) {
            if actions.stop { Button("Stop") { bridge.requestAttentionAction("stop") }.disabled(bridge.attentionActionInFlight) }
            if actions.approve { Button("Approve") { bridge.requestAttentionAction("approve") }.disabled(bridge.attentionActionInFlight) }
            if actions.reject { Button("Reject") { bridge.requestAttentionAction("reject") }.disabled(bridge.attentionActionInFlight) }
            Spacer()
            Button("Open Task Detail", action: openDetail).keyboardShortcut(.return)
        }
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
