import SwiftUI

struct APIProviderCapabilities: Codable, Equatable {
    let streaming: Bool
    let cancellation: Bool
    let toolCalling: Bool
    let structuredOutput: Bool
    let modelSelection: Bool
    let rateLimits: Bool
    let implementation: Bool
    let readOnlyReview: Bool
    let repair: Bool
}

struct APIProviderSelection: Codable, Equatable {
    let providerId: String
    let modelId: String
}

struct APIWorkflowProviderConfiguration: Codable, Equatable {
    let implementer: APIProviderSelection
    let reviewer: APIProviderSelection
    let repair: APIProviderSelection?
}

struct APIProviderSettingsItem: Codable, Equatable, Identifiable {
    let id: String
    let displayName: String
    let modelId: String
    let supportedModels: [String]?
    let baseUrl: String?
    let enabled: Bool
    let capabilities: APIProviderCapabilities
    let credentialState: String
    let custom: Bool

    var availableForSelection: Bool {
        enabled && (credentialState == "configured" || credentialState == "not_required")
    }

    func supportsWorkflowRole(_ role: String) -> Bool {
        guard availableForSelection else { return false }
        switch role {
        case "implementer": return capabilities.implementation
        case "reviewer": return capabilities.readOnlyReview
        case "repair": return capabilities.repair
        default: return false
        }
    }
}

struct APIProviderSettingsSnapshot: Codable, Equatable {
    let version: UInt64
    let providers: [APIProviderSettingsItem]
    let workflow: APIWorkflowProviderConfiguration
}

struct APICustomProviderIntent: Codable, Equatable {
    let providerId: String
    let displayName: String
    let baseUrl: String
    let modelId: String
    let enabled: Bool
    let capabilities: APIProviderCapabilities
    let limits: APIModelLimits
}

struct APIModelLimits: Codable, Equatable {
    let contextTokens: UInt64?
    let maxOutputTokens: UInt64?
}

struct APIProviderSettingsSection: View {
    let snapshot: APIProviderSettingsSnapshot
    @ObservedObject var bridge: NativeBridge
    @State private var customID = "custom."
    @State private var customName = ""
    @State private var customURL = ""
    @State private var customModel = ""

    var body: some View {
        VStack(alignment: .leading, spacing: SentinelTokens.compactSpacing) {
            Text("Provider models and workflow roles")
                .font(.caption.weight(.semibold))
            Text("OMP uses its own login or environment credentials for GLM/Kimi. Other provider keys are sent once to Rust and stored in macOS Keychain.")
                .font(.caption2)
                .foregroundStyle(.secondary)
            if let message = bridge.settingsMutationMessage {
                Text(message)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .accessibilityLabel("Provider settings: \(message)")
            }

            ForEach(snapshot.providers.filter { $0.id != "codex" && $0.id != "claude_code" }) { provider in
                Divider()
                APIProviderRow(provider: provider, bridge: bridge)
            }

            Divider()
            workflowSelectors
            Divider()
            customProviderEditor
        }
    }

    private var workflowSelectors: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Workflow routing").font(.caption.weight(.semibold))
            providerPicker(
                "Implementer",
                selected: snapshot.workflow.implementer,
                role: "implementer"
            )
            providerPicker(
                "Reviewer",
                selected: snapshot.workflow.reviewer,
                role: "reviewer"
            )
            providerPicker(
                "Repair",
                selected: snapshot.workflow.repair ?? snapshot.workflow.implementer,
                role: "repair"
            )
        }
    }

    private func providerPicker(
        _ label: String,
        selected: APIProviderSelection,
        role: String
    ) -> some View {
        let selectable = selectableProviders(for: role)
        return HStack {
            Text(label)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 90, alignment: .leading)
            Picker(label, selection: Binding(
                get: { selected.providerId },
                set: { updateWorkflow(role: role, providerID: $0) }
            )) {
                ForEach(selectable) { provider in
                    Text("\(provider.displayName) · \(provider.modelId)")
                        .tag(provider.id)
                }
            }
            .labelsHidden()
            .disabled(bridge.settingsMutationInFlight || selectable.isEmpty)
        }
    }

    private func selectableProviders(for role: String) -> [APIProviderSettingsItem] {
        snapshot.providers.filter { provider in
            provider.supportsWorkflowRole(role)
        }
    }

    private func updateWorkflow(role: String, providerID: String) {
        guard let provider = snapshot.providers.first(where: { $0.id == providerID }) else { return }
        let selection = APIProviderSelection(providerId: provider.id, modelId: provider.modelId)
        bridge.setWorkflowProviders(APIWorkflowProviderConfiguration(
            implementer: role == "implementer" ? selection : snapshot.workflow.implementer,
            reviewer: role == "reviewer" ? selection : snapshot.workflow.reviewer,
            repair: role == "repair" ? selection : snapshot.workflow.repair
        ))
    }

    private var customProviderEditor: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Custom OpenAI-compatible endpoint")
                .font(.caption.weight(.semibold))
            TextField("Provider ID (custom.company)", text: $customID)
            TextField("Display name", text: $customName)
            TextField("HTTPS base URL", text: $customURL)
            TextField("Model ID", text: $customModel)
            HStack {
                Spacer()
                Button("Add Provider") {
                    bridge.upsertCustomProvider(APICustomProviderIntent(
                        providerId: customID,
                        displayName: customName,
                        baseUrl: customURL,
                        modelId: customModel,
                        enabled: true,
                        capabilities: APIProviderCapabilities(
                            streaming: true,
                            cancellation: true,
                            toolCalling: true,
                            structuredOutput: true,
                            modelSelection: true,
                            rateLimits: false,
                            implementation: true,
                            readOnlyReview: true,
                            repair: true
                        ),
                        limits: APIModelLimits(contextTokens: nil, maxOutputTokens: nil)
                    ))
                }
                .disabled(
                    bridge.settingsMutationInFlight
                        || customID.isEmpty
                        || customName.isEmpty
                        || customURL.isEmpty
                        || customModel.isEmpty
                )
            }
        }
        .textFieldStyle(.roundedBorder)
    }
}

private struct APIProviderRow: View {
    let provider: APIProviderSettingsItem
    @ObservedObject var bridge: NativeBridge
    @State private var modelID: String
    @State private var apiKey = ""

    init(provider: APIProviderSettingsItem, bridge: NativeBridge) {
        self.provider = provider
        self.bridge = bridge
        _modelID = State(initialValue: provider.modelId)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                ProviderBadge(provider: provider.displayName)
                Text(provider.credentialState.replacingOccurrences(of: "_", with: " "))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Spacer()
                Toggle("Enabled", isOn: Binding(
                    get: { provider.enabled },
                    set: { bridge.setAPIProviderEnabled(provider.id, enabled: $0) }
                ))
                .toggleStyle(.switch)
                .controlSize(.small)
                .disabled(bridge.settingsMutationInFlight)
            }
            if let baseURL = provider.baseUrl {
                Text(baseURL)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
            }
            HStack {
                if let models = provider.supportedModels, models.count > 1 {
                    Picker("Model", selection: $modelID) {
                        ForEach(models, id: \.self) { model in
                            Text(model).tag(model)
                        }
                    }
                    .labelsHidden()
                    .onChange(of: modelID) { _, newValue in
                        guard newValue != provider.modelId else { return }
                        bridge.setAPIProviderModel(provider.id, modelID: newValue)
                    }
                } else {
                    TextField("Model ID", text: $modelID)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit {
                            bridge.setAPIProviderModel(provider.id, modelID: modelID)
                        }
                    Button("Set Model") {
                        bridge.setAPIProviderModel(provider.id, modelID: modelID)
                    }
                    .disabled(bridge.settingsMutationInFlight || modelID.isEmpty || modelID == provider.modelId)
                }
            }
            HStack {
                SecureField(
                    provider.credentialState == "configured" ? "Replacement API key" : "API key",
                    text: $apiKey
                )
                .textFieldStyle(.roundedBorder)
                Button(provider.credentialState == "configured" ? "Replace" : "Set") {
                    let submitted = apiKey
                    apiKey = ""
                    bridge.setAPIProviderCredential(provider.id, apiKey: submitted)
                }
                .disabled(bridge.settingsMutationInFlight || apiKey.count < 8)
                if provider.credentialState == "configured" {
                    Button("Remove", role: .destructive) {
                        bridge.removeAPIProviderCredential(provider.id)
                    }
                    .disabled(bridge.settingsMutationInFlight)
                }
                if provider.custom {
                    Button("Delete", role: .destructive) {
                        bridge.removeCustomProvider(provider.id)
                    }
                    .disabled(bridge.settingsMutationInFlight)
                }
            }
        }
        .onChange(of: provider.modelId) { _, newValue in modelID = newValue }
    }
}
