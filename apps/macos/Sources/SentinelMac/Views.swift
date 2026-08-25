import AppKit
import SwiftUI

extension Notification.Name {
    static let sentinelQuickPromptShown = Notification.Name("sentinelQuickPromptShown")
}

struct QuickPromptView: View {
    @ObservedObject var bridge: NativeBridge
    let accepted: () -> Void
    @State private var prompt = ""
    @State private var implementerProvider = ""
    @State private var implementerModel = ""
    @State private var reviewerProvider = ""
    @State private var reviewerModel = ""
    @State private var reviewerEffort = ""
    @State private var workflowMode = NativeWorkflowMode.manual
    @State private var restored = false
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

    private var configuration: NativeTaskConfiguration? {
        guard model(providerID: implementerProvider, modelID: implementerModel, role: .implementer) != nil,
              let reviewer = model(providerID: reviewerProvider, modelID: reviewerModel, role: .reviewer)
        else { return nil }
        let effort: String?
        if reviewer.supportedReasoningEfforts.isEmpty {
            effort = nil
        } else {
            guard reviewer.supportedReasoningEfforts.contains(reviewerEffort) else { return nil }
            effort = reviewerEffort
        }
        return NativeTaskConfiguration(
            implementer: APIProviderSelection(
                providerId: implementerProvider,
                modelId: implementerModel
            ),
            reviewer: APIProviderSelection(
                providerId: reviewerProvider,
                modelId: reviewerModel,
                reasoningEffort: effort
            ),
            workflowMode: workflowMode
        )
    }

    var body: some View {
        SentinelPanel(style: .floating) {
            VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                HStack { ProviderBadge(provider: "Sentinel"); Spacer(); Text("Quick Prompt").font(.headline) }
                if let repository = bridge.repositoryContext {
                    Label(repository, systemImage: "folder").font(.caption).foregroundStyle(.secondary).lineLimit(1)
                } else {
                    Text("Repository unavailable").font(.caption).foregroundStyle(.orange)
                }
                TextField("Describe a task…", text: $prompt, axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .lineLimit(3...6)
                    .focused($promptFocused)
                    .onSubmit(send)
                selectionSection(
                    title: "Implementer",
                    role: .implementer,
                    providerID: implementerProvider,
                    modelID: implementerModel
                )
                selectionSection(
                    title: "Reviewer",
                    role: .reviewer,
                    providerID: reviewerProvider,
                    modelID: reviewerModel
                )
                if let reviewer = model(
                    providerID: reviewerProvider,
                    modelID: reviewerModel,
                    role: .reviewer
                ), !reviewer.supportedReasoningEfforts.isEmpty {
                    HStack(spacing: SentinelTokens.compactSpacing) {
                        Text("Reasoning").font(.caption).foregroundStyle(.secondary)
                            .frame(width: 76, alignment: .leading)
                        Picker("Reviewer reasoning effort", selection: Binding(
                            get: { reviewerEffort },
                            set: { reviewerEffort = $0; persistIfValid() }
                        )) {
                            ForEach(reviewer.supportedReasoningEfforts, id: \.self) { effort in
                                Text(effort.capitalized).tag(effort)
                            }
                        }
                        .labelsHidden()
                        .disabled(sending)
                    }
                }
                VStack(alignment: .leading, spacing: 3) {
                    Text("Workflow").font(.caption.weight(.semibold))
                    Picker("Workflow", selection: Binding(
                        get: { workflowMode },
                        set: { workflowMode = $0; persistIfValid() }
                    )) {
                        Text("Manual").tag(NativeWorkflowMode.manual)
                        Text("Auto Integrate").tag(NativeWorkflowMode.autoIntegrate)
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .disabled(sending)
                    Text(workflowMode == .manual
                         ? "Stops for your approval before finalizing changes."
                         : "Automatically applies verified changes to the selected local branch after validation and a clean review. No automatic push.")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                HStack {
                    Text("⌘↩ run · Esc hide")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                    Spacer()
                    Button(sending ? "Starting…" : "Run Task", action: send)
                        .keyboardShortcut(.return, modifiers: .command)
                        .sentinelPrimaryButtonStyle()
                        .disabled(sending || bridge.repositoryContext == nil || prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || configuration == nil)
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
        .onAppear {
            promptFocused = true
            restoreSelectionsIfPossible()
        }
        .onReceive(NotificationCenter.default.publisher(for: .sentinelQuickPromptShown)) { _ in
            promptFocused = true
            refreshMissingCatalogs()
        }
        .onChange(of: bridge.providers) { _, _ in
            restoreSelectionsIfPossible()
        }
        .onChange(of: bridge.modelDiscovery) { _, _ in
            reconcileDiscoveries()
        }
        .onChange(of: bridge.taskSubmission) { _, state in
            if case .accepted = state {
                prompt = ""
                accepted()
            }
        }
    }

    private func send() {
        guard let configuration else { return }
        bridge.submitTask(
            configuration: configuration,
            summary: prompt.trimmingCharacters(in: .whitespacesAndNewlines),
            prompt: prompt
        )
    }

    @ViewBuilder
    private func selectionSection(
        title: String,
        role: QuickPromptRole,
        providerID: String,
        modelID: String
    ) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(title).font(.caption.weight(.semibold))
            HStack(spacing: SentinelTokens.compactSpacing) {
                Picker("\(title) provider", selection: Binding(
                    get: { providerID },
                    set: { selectProvider($0, role: role) }
                )) {
                    ForEach(bridge.providers) { capability in
                        Text(capability.label).tag(capability.id).disabled(!capability.available)
                    }
                }
                .labelsHidden()
                .disabled(sending)
                modelPicker(title: title, role: role, providerID: providerID, modelID: modelID)
                Button {
                    bridge.discoverModels(providerID: providerID, role: role)
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .disabled(sending || providerID.isEmpty)
                .help("Refresh \(title.lowercased()) models")
                .accessibilityLabel("Refresh \(title.lowercased()) models")
            }
            discoveryStatus(role: role, providerID: providerID, modelID: modelID)
        }
    }

    @ViewBuilder
    private func modelPicker(
        title: String,
        role: QuickPromptRole,
        providerID: String,
        modelID: String
    ) -> some View {
        let models = catalog(role: role, providerID: providerID)?.models ?? []
        Picker("\(title) model", selection: Binding(
            get: { modelID },
            set: { selectModel($0, role: role) }
        )) {
            if !modelID.isEmpty && !models.contains(where: { $0.modelId == modelID && $0.available }) {
                Text("\(modelID) (Unavailable)").tag(modelID)
            }
            ForEach(models.filter(\.available)) { model in
                Text(model.label).tag(model.modelId)
            }
        }
        .labelsHidden()
        .disabled(sending || models.isEmpty)
        .frame(maxWidth: .infinity)
    }

    @ViewBuilder
    private func discoveryStatus(role: QuickPromptRole, providerID: String, modelID: String) -> some View {
        switch bridge.modelDiscovery[role] ?? .idle {
        case .idle:
            EmptyView()
        case .loading(let loadingProvider) where loadingProvider == providerID:
            Label("Loading models…", systemImage: "progress.indicator")
                .font(.caption2).foregroundStyle(.secondary)
        case .loaded(let catalog) where catalog.providerId == providerID && catalog.models.isEmpty:
            Text("No models available").font(.caption2).foregroundStyle(.orange)
        case .loaded(let catalog) where catalog.providerId == providerID:
            if !modelID.isEmpty && !catalog.models.contains(where: { $0.modelId == modelID && $0.available }) {
                Text("Selected model is unavailable. Refresh or choose another model.")
                    .font(.caption2).foregroundStyle(.orange)
            } else if catalog.discoveryKind == "current_configuration" {
                Text("Enumeration unsupported; using Claude Code's current/default configuration.")
                    .font(.caption2).foregroundStyle(.secondary)
            }
        case .failed(let failedProvider, let code, let message) where failedProvider == providerID:
            Text(code == "authentication_required" ? "Authentication required" : message)
                .font(.caption2).foregroundStyle(.orange)
        default:
            EmptyView()
        }
    }

    private func restoreSelectionsIfPossible() {
        guard !restored, !bridge.providers.isEmpty else { return }
        if let preferences = bridge.quickPromptPreferences {
            implementerProvider = preferences.implementer.providerId
            implementerModel = preferences.implementer.modelId
            reviewerProvider = preferences.reviewer.providerId
            reviewerModel = preferences.reviewer.modelId
            reviewerEffort = preferences.reviewer.reasoningEffort ?? ""
            workflowMode = preferences.workflowMode
        } else {
            implementerProvider = bridge.quickPromptDefaults?.implementerProviderId
                ?? bridge.providers.first(where: \.available)?.id
                ?? bridge.providers.first?.id
                ?? ""
            reviewerProvider = bridge.quickPromptDefaults?.reviewerProviderId
                ?? bridge.providers.first(where: \.available)?.id
                ?? bridge.providers.first?.id
                ?? ""
            workflowMode = .manual
        }
        restored = true
        refreshMissingCatalogs()
    }

    private func refreshMissingCatalogs() {
        guard restored else { return }
        if !implementerProvider.isEmpty {
            bridge.discoverModels(providerID: implementerProvider, role: .implementer)
        }
        if !reviewerProvider.isEmpty {
            bridge.discoverModels(providerID: reviewerProvider, role: .reviewer)
        }
    }

    private func selectProvider(_ providerID: String, role: QuickPromptRole) {
        switch role {
        case .implementer:
            implementerProvider = providerID
            implementerModel = ""
        case .reviewer:
            reviewerProvider = providerID
            reviewerModel = ""
            reviewerEffort = ""
        }
        bridge.discoverModels(providerID: providerID, role: role)
    }

    private func selectModel(_ modelID: String, role: QuickPromptRole) {
        switch role {
        case .implementer:
            implementerModel = modelID
        case .reviewer:
            reviewerModel = modelID
            if let model = model(providerID: reviewerProvider, modelID: modelID, role: .reviewer) {
                reviewerEffort = QuickPromptSelectionValidation.preferredReviewerEffort(for: model)
                    ?? ""
            } else {
                reviewerEffort = ""
            }
        }
        persistIfValid()
    }

    private func reconcileDiscoveries() {
        if implementerModel.isEmpty,
           let catalog = catalog(role: .implementer, providerID: implementerProvider),
           let selected = catalog.models.first(where: { $0.available && $0.isDefault })
                ?? catalog.models.first(where: \.available) {
            implementerModel = selected.modelId
        }
        if reviewerModel.isEmpty,
           let catalog = catalog(role: .reviewer, providerID: reviewerProvider),
           let selected = catalog.models.first(where: { $0.available && $0.isDefault })
                ?? catalog.models.first(where: \.available) {
            reviewerModel = selected.modelId
            reviewerEffort = QuickPromptSelectionValidation.preferredReviewerEffort(for: selected)
                ?? ""
        }
        persistIfValid()
    }

    private func persistIfValid() {
        guard let configuration else { return }
        bridge.saveQuickPromptPreferences(NativeQuickPromptPreferences(
            implementer: configuration.implementer,
            reviewer: configuration.reviewer,
            workflowMode: configuration.workflowMode
        ))
    }

    private func catalog(role: QuickPromptRole, providerID: String) -> NativeProviderModelCatalog? {
        guard case .loaded(let catalog) = bridge.modelDiscovery[role],
              catalog.providerId == providerID else { return nil }
        return catalog
    }

    private func model(
        providerID: String,
        modelID: String,
        role: QuickPromptRole
    ) -> NativeProviderModel? {
        guard bridge.providers.first(where: { $0.id == providerID })?.available == true else {
            return nil
        }
        return QuickPromptSelectionValidation.availableModel(
            modelID: modelID,
            catalog: catalog(role: role, providerID: providerID)
        )
    }
}

struct AttentionWidgetView: View {
    @ObservedObject var bridge: NativeBridge
    let openDetail: () -> Void

    var body: some View {
        SentinelPanel(style: .floating) {
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
                    Text(bridge.availabilityMessage ?? "Open Quick Prompt to create a task.").font(.caption).foregroundStyle(.secondary)
                    Button("Open Task Detail", action: openDetail)
                        .sentinelPrimaryButtonStyle()
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
            if actions.stop { Button("Stop") { bridge.requestAttentionAction("stop", restoreFocusAfterAcceptance: true) }.disabled(bridge.attentionActionInFlight) }
            if actions.approve { Button("Approve") { bridge.requestAttentionAction("approve", restoreFocusAfterAcceptance: true) }.disabled(bridge.attentionActionInFlight) }
            if actions.reject { Button("Reject") { bridge.requestAttentionAction("reject", restoreFocusAfterAcceptance: true) }.disabled(bridge.attentionActionInFlight) }
            Spacer()
            Button("Open Task Detail", action: openDetail)
                .keyboardShortcut(.return)
                .sentinelPrimaryButtonStyle()
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
                        if let configuration = detail.configuration {
                            taskConfiguration(configuration)
                        }
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

    private func taskConfiguration(_ configuration: NativeTaskConfiguration) -> some View {
        Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
            GridRow {
                Text("Implementer").foregroundStyle(.secondary)
                Text("\(providerName(configuration.implementer.providerId)) · \(configuration.implementer.modelId)")
                    .lineLimit(1)
            }
            GridRow {
                Text("Reviewer").foregroundStyle(.secondary)
                Text([
                    providerName(configuration.reviewer.providerId),
                    configuration.reviewer.modelId,
                    configuration.reviewer.reasoningEffort,
                ].compactMap { $0 }.joined(separator: " · "))
                    .lineLimit(1)
            }
            GridRow {
                Text("Workflow").foregroundStyle(.secondary)
                Text(configuration.workflowMode == .manual ? "Manual" : "Auto Integrate")
            }
        }
        .font(.caption)
    }

    private func providerName(_ providerID: String) -> String {
        switch providerID {
        case "omp": return "OMP"
        case "claude_code": return "Claude Code"
        case "codex": return "Codex"
        default: return providerID
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
                    Text(event.payload)
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                        .sentinelEvidenceSurface()
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
                DisclosureGroup("Changed-file / diff data") {
                    Text(diff.summary).font(.caption.monospaced()).textSelection(.enabled).sentinelEvidenceSurface()
                }
                if !diff.conflicts.isEmpty {
                    DisclosureGroup("Conflict evidence") {
                        Text(diff.conflicts).font(.caption.monospaced()).textSelection(.enabled).sentinelEvidenceSurface()
                    }
                }
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

    private func output(_ name: String, _ text: String) -> some View {
        DisclosureGroup(name) {
            Text(text).font(.caption.monospaced()).textSelection(.enabled).sentinelEvidenceSurface()
        }
    }

    private func review(_ findings: [DetailFinding]) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            ForEach(["blocker", "warning", "suggestion"], id: \.self) { severity in
                let group = findings.filter { $0.severity.lowercased() == severity }
                if !group.isEmpty {
                    Text(severity.capitalized).font(.caption.weight(.bold)).foregroundStyle(severity == "blocker" ? .red : .secondary)
                    ForEach(group, id: \.id) { finding in
                        DisclosureGroup {
                            Text(finding.evidence).font(.caption.monospaced()).textSelection(.enabled).sentinelEvidenceSurface()
                        } label: {
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
            if let packet = detail.finalApprovalPacket {
                DisclosureGroup("Durable approval packet") {
                    Text(packet).font(.caption.monospaced()).textSelection(.enabled).sentinelEvidenceSurface()
                }
            }
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

struct StatusView: View {
    @ObservedObject var bridge: NativeBridge

    var body: some View {
        SentinelPanel {
            if let status = bridge.status {
                VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                    HStack { ProviderBadge(provider: "Sentinel"); Spacer(); StateBadge(state: status.sentinel) }
                    if let task = status.activeTask {
                        Text(task.summary).font(.subheadline.weight(.semibold)).lineLimit(2)
                        Text("\(task.lifecycle) · \(status.recoveryRequired ? "recovery required" : "durable state")").font(.caption).foregroundStyle(status.recoveryRequired ? .red : .secondary)
                    } else {
                        Text("No active task").font(.caption).foregroundStyle(.secondary)
                    }
                    provider(status.codex)
                    provider(status.claude)
                }
            } else {
                VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
                    Text("Sentinel Status").font(.headline)
                    Text(bridge.availabilityMessage ?? "Waiting for the Rust status bridge…").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
        .frame(minWidth: 320, idealWidth: 360, maxWidth: 420)
        .onAppear { bridge.loadStatus() }
    }

    private func provider(_ provider: NativeProviderStatus) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack { ProviderBadge(provider: provider.name); Spacer(); Text(provider.installation).font(.caption2).foregroundStyle(.secondary).lineLimit(1) }
            Text(provider.runtime).font(.caption).lineLimit(1)
            if let limits = provider.rateLimits {
                rateLimit("Primary", limits["primary"])
                rateLimit("Secondary", limits["secondary"])
            } else {
                Text(provider.usage).font(.caption2).foregroundStyle(.secondary).lineLimit(2)
            }
        }
        .padding(.vertical, 3)
    }

    @ViewBuilder
    private func rateLimit(_ title: String, _ window: RateLimitWindow?) -> some View {
        if let window {
            HStack(spacing: 6) {
                Text(title).font(.caption2).frame(width: 58, alignment: .leading)
                ProgressView(value: window.usedPercent, total: 100).frame(maxWidth: .infinity)
                Text("\(window.usedPercent, specifier: "%.0f")%").font(.caption2.monospacedDigit())
            }
            Text([window.resetsAt, window.windowDurationMins.map { "\($0) min window" }].compactMap { $0 }.joined(separator: " · ")).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
        } else {
            Text("\(title): unavailable").font(.caption2).foregroundStyle(.secondary)
        }
    }
}
struct SettingsView: View {
    @ObservedObject var bridge: NativeBridge
    @ObservedObject var shortcutController: GlobalShortcutController

    var body: some View {
        ScrollView {
            SentinelPanel {
                if let settings = bridge.settings {
                    VStack(alignment: .leading, spacing: SentinelTokens.spacing) {
                        HStack {
                            ProviderBadge(provider: "Sentinel")
                            Spacer()
                            Text("Settings").font(.headline)
                        }
                        general(settings)
                        providers(settings)
                        validation(settings)
                        Text("Workflow, security, credentials, and provider authentication remain managed by Rust.")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                } else {
                    VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
                        Text("Sentinel Settings").font(.headline)
                        shortcutSettings()
                        Text(bridge.availabilityMessage ?? "Waiting for Rust-confirmed settings…")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .padding()
        }
        .frame(minWidth: 460, idealWidth: 520, minHeight: 400, idealHeight: 470)
        .scrollIndicators(.visible)
        .onAppear { bridge.loadSettings() }
        .onDisappear { shortcutController.cancelRecording() }
    }

    private func general(_ settings: NativeSettings) -> some View {
        settingsSection("General") {
            shortcutSettings()
            settingRow("Default provider", settings.defaultProvider.capitalized)
            settingRow("Repository", settings.repository ?? "No repository context")
            HStack {
                Button("Choose Repository…", action: chooseRepository)
                    .disabled(bridge.settingsMutationInFlight)
                if bridge.settingsMutationInFlight { ProgressView().controlSize(.small) }
                Spacer()
            }
            if let message = bridge.settingsMutationMessage {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(message == "Repository updated." ? Color.secondary : Color.red)
                    .lineLimit(2)
            }
        }
    }

    private func shortcutSettings() -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: SentinelTokens.compactSpacing) {
                Text("Open Quick Prompt")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .frame(width: 124, alignment: .leading)
                Text(shortcutController.current.displayName)
                    .font(.caption.monospaced())
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .accessibilityLabel("Current shortcut \(shortcutController.current.displayName)")
                Spacer()
                Button(shortcutController.isRecording ? "Cancel" : "Record") {
                    shortcutController.beginRecording()
                }
                .accessibilityLabel(shortcutController.isRecording ? "Cancel shortcut recording" : "Record Quick Prompt shortcut")
                Button("Reset to Default") {
                    _ = shortcutController.resetToDefault()
                }
                .disabled(shortcutController.current == .default && !shortcutController.isRecording)
            }
            if shortcutController.isRecording {
                Text("Press a key with Command, Option, or Control. Press Escape to cancel.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if let error = shortcutController.errorMessage {
                Text(error)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .accessibilityLabel("Shortcut error: \(error)")
            }
        }
    }

    private func chooseRepository() {
        let panel = NSOpenPanel()
        panel.title = "Choose a Git Repository"
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        bridge.setRepository(url.path)
    }

    private func providers(_ settings: NativeSettings) -> some View {
        settingsSection("Providers") {
            if let apiProviders = settings.providerSettings {
                APIProviderSettingsSection(snapshot: apiProviders, bridge: bridge)
                Divider()
            }
            Text("CLI-owned providers")
                .font(.caption.weight(.semibold))
            provider(settings.codex)
            Divider()
            provider(settings.claude)
        }
    }

    private func provider(_ provider: SettingsProvider) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                ProviderBadge(provider: provider.name)
                Spacer()
                Text(provider.installation).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
            if let override = provider.executableOverride {
                settingRow("Executable override", override)
            } else {
                Text("No configured executable override.").font(.caption).foregroundStyle(.secondary)
            }
            if !provider.supportsExecutableOverride {
                Text("Executable overrides are configured by Rust at startup and are not editable here.")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Text(provider.authentication).font(.caption2).foregroundStyle(.secondary).lineLimit(2)
        }
    }

    private func validation(_ settings: NativeSettings) -> some View {
        settingsSection("Validation") {
            if let error = settings.validationError {
                Label(error, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
            } else if settings.validationProfiles.isEmpty {
                Text("No repository validation profiles are configured.").font(.caption).foregroundStyle(.secondary)
            } else {
                ForEach(settings.validationProfiles, id: \.id) { profile in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(profile.id).font(.caption.weight(.semibold))
                        ForEach(Array(profile.steps.enumerated()), id: \.offset) { _, step in
                            VStack(alignment: .leading, spacing: 1) {
                                Text("\(step.kind.capitalized) · \(step.name)").font(.caption)
                                Text("\(step.cwd) · \(step.timeoutMs) ms\(step.required ? " · required" : "")")
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }
                        }
                    }
                }
            }
        }
    }

    private func settingsSection<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            Text(title).font(.subheadline.weight(.semibold))
            content()
        }
    }

    private func settingRow(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: SentinelTokens.compactSpacing) {
            Text(label).font(.caption).foregroundStyle(.secondary).frame(width: 124, alignment: .leading)
            Text(value).font(.caption).lineLimit(1).truncationMode(.middle).textSelection(.enabled)
        }
    }
}
