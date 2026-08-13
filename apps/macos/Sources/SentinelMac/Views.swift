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
    @State private var expandedSections: Set<String> = ["activity", "validation", "review"]

    var body: some View {
        ScrollView {
            SentinelPanel {
                if let detail = bridge.taskDetail {
                    VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                        summary(detail)
                        if detail.task.recoveryRequired || detail.task.lifecycle == "failed" || detail.task.lifecycle == "blocked" {
                            Label(detail.task.recoveryReason?.replacingOccurrences(of: "_", with: " ") ?? "Task requires attention.", systemImage: "exclamationmark.triangle.fill")
                                .font(.subheadline.weight(.semibold)).foregroundStyle(.red)
                        }
                        section("Activity", id: "activity") { activity(detail.activity) }
                        section("Changes", id: "changes") { changes(detail) }
                        section("Validation · deterministic", id: "validation") { validations(detail.validations) }
                        section("Review · model findings", id: "review") { review(detail.findings) }
                        section("Repair History", id: "repair") { repairs(detail) }
                        section("Final Approval", id: "approval") { approval(detail) }
                    }
                } else {
                    VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                        Text("No active V3 task").font(.title3.weight(.semibold))
                        Text(bridge.availabilityMessage ?? "Task detail will appear after durable Rust state is available.").foregroundStyle(.secondary)
                    }
                }
            }
            .padding()
        }
        .frame(minWidth: 620, minHeight: 520)
        .onAppear { bridge.loadTaskDetail() }
    }

    private func summary(_ detail: NativeTaskDetail) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            HStack {
                ProviderBadge(provider: detail.sessions.last?.provider ?? "Sentinel")
                Spacer()
                StateBadge(state: detail.task.lifecycle)
            }
            Text(detail.task.summary).font(.title3.weight(.semibold)).lineLimit(3)
            if let session = detail.sessions.last {
                Text("Session · \(session.sessionRef) · \(session.state)").font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            if let worktree = detail.worktree {
                Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                    GridRow { Text("Repository").foregroundStyle(.secondary); Text(worktree.repositoryRoot).lineLimit(1) }
                    GridRow { Text("Branch").foregroundStyle(.secondary); Text(worktree.branch).lineLimit(1) }
                    GridRow { Text("Base").foregroundStyle(.secondary); Text(worktree.baseCommit).fontDesign(.monospaced).lineLimit(1) }
                }.font(.caption)
            }
            if let event = detail.activity.last { Label(event.kind.replacingOccurrences(of: "_", with: " "), systemImage: "waveform.path.ecg").font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func section<Content: View>(_ title: String, id: String, @ViewBuilder content: @escaping () -> Content) -> some View {
        DisclosureGroup(isExpanded: Binding(get: { expandedSections.contains(id) }, set: { expanded in
            if expanded { expandedSections.insert(id) } else { expandedSections.remove(id) }
        })) {
            content().padding(.top, SentinelTokens.compactSpacing)
        } label: { Text(title).font(.headline) }
    }

    private func activity(_ events: [DetailEvent]) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            ForEach(events, id: \.occurredAtMs) { event in
                DisclosureGroup {
                    Text(event.payload).font(.caption.monospaced()).textSelection(.enabled)
                } label: {
                    HStack { Text(event.kind.replacingOccurrences(of: "_", with: " ")).fontWeight(meaningful(event.kind) ? .semibold : .regular); Spacer(); Text(event.provider).foregroundStyle(.secondary) }.font(.caption)
                }
            }
        }
    }

    private func changes(_ detail: NativeTaskDetail) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            if let worktree = detail.worktree { Text("\(worktree.path) · \(worktree.state)").font(.caption).textSelection(.enabled) }
            if let diff = detail.diff {
                Text("Target \(diff.targetBranch) · \(diff.mergeReady ? "merge ready" : "not merge ready")\(diff.targetAdvanced ? " · target advanced" : "")").font(.caption)
                DisclosureGroup("Changed-file / diff data") { Text(diff.summary).font(.caption.monospaced()).textSelection(.enabled) }
                if !diff.conflicts.isEmpty { DisclosureGroup("Conflict evidence") { Text(diff.conflicts).font(.caption.monospaced()).textSelection(.enabled) } }
            } else { Text("No durable diff has been prepared.").font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func validations(_ values: [DetailValidation]) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            ForEach(values, id: \.id) { value in
                DisclosureGroup {
                    VStack(alignment: .leading, spacing: 4) {
                        if let command = value.command { Text(command).font(.caption.monospaced()) }
                        if let stdout = value.stdout { output("stdout", stdout) }
                        if let stderr = value.stderr, !stderr.isEmpty { output("stderr", stderr) }
                    }
                } label: {
                    HStack { Text("\(value.profile) / \(value.check)").font(.caption.weight(.semibold)); Spacer(); StateBadge(state: value.outcome ?? value.state) }
                    Text([value.summary, value.exitCode.map { "exit \($0)" }, value.durationMs.map { "\($0) ms" }].compactMap { $0 }.joined(separator: " · ")).font(.caption2).foregroundStyle(.secondary)
                }
            }
            if values.isEmpty { Text("No validation results.").font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func output(_ name: String, _ text: String) -> some View { DisclosureGroup(name) { Text(text).font(.caption.monospaced()).textSelection(.enabled) } }

    private func review(_ findings: [DetailFinding]) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            ForEach(["blocker", "warning", "suggestion"], id: \.self) { severity in
                let group = findings.filter { $0.severity.lowercased() == severity }
                if !group.isEmpty {
                    Text(severity.capitalized).font(.caption.weight(.bold)).foregroundStyle(severity == "blocker" ? .red : .secondary)
                    ForEach(group, id: \.id) { finding in
                        DisclosureGroup { Text(finding.evidence).font(.caption.monospaced()).textSelection(.enabled) } label: {
                            Text("\(finding.summary) · \(finding.disposition)").font(.caption).foregroundStyle(severity == "blocker" && !resolved(finding.disposition) ? .red : .primary)
                        }
                    }
                }
            }
            if findings.isEmpty { Text("No review findings.").font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func repairs(_ detail: NativeTaskDetail) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            ForEach(detail.repairRounds, id: \.id) { round in
                let count = detail.findings.filter { $0.repairRoundID == round.id }.count
                Text("Round \(round.round) · \(round.state) · \(count) finding\(count == 1 ? "" : "s")").font(.caption)
            }
            if detail.repairRounds.isEmpty { Text("No repair rounds.").font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func approval(_ detail: NativeTaskDetail) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            if let packet = detail.finalApprovalPacket { DisclosureGroup("Durable approval packet") { Text(packet).font(.caption.monospaced()).textSelection(.enabled) } }
            HStack {
                if detail.actions.approve { Button("Approve") { bridge.requestAttentionAction("approve") }.disabled(bridge.attentionActionInFlight) }
                if detail.actions.reject { Button("Reject") { bridge.requestAttentionAction("reject") }.disabled(bridge.attentionActionInFlight) }
            }
            if detail.finalApprovalPacket == nil { Text("No final approval packet.").font(.caption).foregroundStyle(.secondary) }
        }
    }

    private func meaningful(_ kind: String) -> Bool { !kind.contains("tool_") && !kind.contains("provider_") }
    private func resolved(_ disposition: String) -> Bool { ["resolved", "repaired", "dismissed"].contains(disposition.lowercased()) }
}

struct StatusView: View { var body: some View { SentinelPanel { Text("Sentinel status").frame(width: 260, alignment: .leading) } } }
struct SettingsView: View { var body: some View { SentinelPanel { Text("Sentinel settings").frame(width: 260, alignment: .leading) } } }
