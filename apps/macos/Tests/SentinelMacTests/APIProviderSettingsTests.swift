import XCTest
@testable import SentinelMac

final class APIProviderSettingsTests: XCTestCase {
    func testDecodesRustConfirmedProviderSettingsWithoutCredentialMaterial() throws {
        let data = Data(#"{"kind":"settings","settings":{"version":1000002,"globalShortcut":"Command+Shift+Space","repository":"/repo","defaultProvider":"codex","codex":{"name":"Codex","installation":"available","executableOverride":null,"supportsExecutableOverride":false,"authentication":"CLI-owned"},"claude":{"name":"Claude Code","installation":"available","executableOverride":null,"supportsExecutableOverride":false,"authentication":"CLI-owned"},"validationProfiles":[],"validationError":null,"providerSettings":{"version":2,"providers":[{"id":"glm","displayName":"GLM","modelId":"glm-5.2","supportedModels":["glm-5.2","glm-4.7"],"baseUrl":"https://api.z.ai/api/paas/v4","enabled":true,"capabilities":{"streaming":true,"cancellation":true,"toolCalling":true,"structuredOutput":true,"modelSelection":true,"rateLimits":true,"implementation":true,"readOnlyReview":true,"repair":true},"credentialState":"configured","custom":false}],"workflow":{"implementer":{"providerId":"codex","modelId":"cli-owned"},"reviewer":{"providerId":"glm","modelId":"glm-5.2"},"repair":null}}}}"#.utf8)

        guard case .settings(let settings) = try JSONDecoder().decode(BridgeMessage.self, from: data) else {
            return XCTFail("expected settings snapshot")
        }
        XCTAssertEqual(settings.providerSettings?.providers.first?.credentialState, "configured")
        XCTAssertEqual(settings.providerSettings?.providers.first?.supportedModels?.last, "glm-4.7")
        XCTAssertEqual(settings.providerSettings?.workflow.reviewer.providerId, "glm")
        XCTAssertFalse(String(describing: settings).localizedCaseInsensitiveContains("apiKey"))
    }

    func testProviderSnapshotGateRejectsStaleRustState() {
        let provider = SettingsProvider(
            name: "Codex",
            installation: "available",
            executableOverride: nil,
            supportsExecutableOverride: false,
            authentication: "CLI-owned"
        )
        let current = NativeSettings(
            version: 3_000_010,
            globalShortcut: "Command+Shift+Space",
            repository: "/repo",
            defaultProvider: "codex",
            codex: provider,
            claude: provider,
            validationProfiles: [],
            validationError: nil,
            providerSettings: nil
        )
        let stale = NativeSettings(
            version: 3_000_009,
            globalShortcut: "Command+Shift+Space",
            repository: nil,
            defaultProvider: "codex",
            codex: provider,
            claude: provider,
            validationProfiles: [],
            validationError: nil,
            providerSettings: nil
        )
        var gate = SettingsUpdateGate()
        XCTAssertTrue(gate.apply(current))
        XCTAssertFalse(gate.apply(stale))
        XCTAssertEqual(gate.current?.repository, "/repo")
    }

    func testCustomProviderIntentContainsOnlyEndpointConfiguration() throws {
        let intent = APICustomProviderIntent(
            providerId: "custom.example",
            displayName: "Example",
            baseUrl: "https://models.example/v1",
            modelId: "example-model",
            enabled: true,
            capabilities: APIProviderCapabilities(
                streaming: true,
                cancellation: true,
                toolCalling: false,
                structuredOutput: false,
                modelSelection: true,
                rateLimits: false,
                implementation: true,
                readOnlyReview: true,
                repair: true
            ),
            limits: APIModelLimits(contextTokens: nil, maxOutputTokens: nil)
        )
        let encoded = String(decoding: try JSONEncoder().encode(intent), as: UTF8.self)
        XCTAssertFalse(encoded.localizedCaseInsensitiveContains("apiKey"))
        XCTAssertFalse(encoded.localizedCaseInsensitiveContains("executable"))
    }

    func testWorkflowRoleSelectionHonorsRustReportedCapabilities() {
        let provider = APIProviderSettingsItem(
            id: "custom.review",
            displayName: "Review Only",
            modelId: "review-model",
            supportedModels: nil,
            baseUrl: "https://models.example/v1",
            enabled: true,
            capabilities: APIProviderCapabilities(
                streaming: true,
                cancellation: true,
                toolCalling: false,
                structuredOutput: true,
                modelSelection: true,
                rateLimits: false,
                implementation: false,
                readOnlyReview: true,
                repair: false
            ),
            credentialState: "configured",
            custom: true
        )

        XCTAssertFalse(provider.supportsWorkflowRole("implementer"))
        XCTAssertTrue(provider.supportsWorkflowRole("reviewer"))
        XCTAssertFalse(provider.supportsWorkflowRole("repair"))
    }
}
