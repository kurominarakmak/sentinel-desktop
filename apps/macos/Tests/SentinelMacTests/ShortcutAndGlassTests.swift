import Carbon.HIToolbox
import XCTest
@testable import SentinelMac

final class ShortcutAndGlassTests: XCTestCase {
    func testShortcutPersistsAndRestoresAcrossControllerLaunches() {
        let defaults = isolatedDefaults()
        let store = ShortcutPreferenceStore(defaults: defaults)
        let chosen = GlobalShortcut(
            keyCode: UInt32(kVK_ANSI_P),
            modifiers: [.command, .option]
        )

        let firstRegistrar = FakeHotKeyRegistrar()
        let first = GlobalShortcutController(store: store, registrar: firstRegistrar)
        first.start {}
        XCTAssertTrue(first.replace(with: chosen))
        XCTAssertEqual(store.load(), chosen)
        first.stop()

        let secondRegistrar = FakeHotKeyRegistrar()
        let second = GlobalShortcutController(store: store, registrar: secondRegistrar)
        XCTAssertEqual(second.current, chosen)
        second.start {}
        XCTAssertEqual(secondRegistrar.activeShortcuts, [chosen])
        second.stop()
    }

    func testReplacementUnregistersPreviousShortcutAndKeepsOneRegistration() {
        let registrar = FakeHotKeyRegistrar()
        let controller = GlobalShortcutController(
            store: ShortcutPreferenceStore(defaults: isolatedDefaults()),
            registrar: registrar
        )
        let replacement = GlobalShortcut(
            keyCode: UInt32(kVK_ANSI_K),
            modifiers: [.command, .control]
        )

        controller.start {}
        XCTAssertTrue(controller.replace(with: replacement))
        XCTAssertEqual(registrar.unregisterCount, 1)
        XCTAssertEqual(registrar.activeShortcuts, [replacement])
        controller.stop()
    }

    func testInvalidShortcutIsRejectedWithoutUnregisteringCurrentShortcut() {
        let registrar = FakeHotKeyRegistrar()
        let controller = GlobalShortcutController(
            store: ShortcutPreferenceStore(defaults: isolatedDefaults()),
            registrar: registrar
        )
        controller.start {}

        let invalid = GlobalShortcut(keyCode: UInt32(kVK_ANSI_A), modifiers: [.shift])
        XCTAssertFalse(controller.replace(with: invalid))
        XCTAssertEqual(controller.current, .default)
        XCTAssertEqual(registrar.unregisterCount, 0)
        XCTAssertEqual(registrar.activeShortcuts, [.default])
        XCTAssertNotNil(controller.errorMessage)
        controller.stop()
    }

    func testRegistrationConflictRestoresPreviousShortcut() {
        let registrar = FakeHotKeyRegistrar()
        let defaults = isolatedDefaults()
        let store = ShortcutPreferenceStore(defaults: defaults)
        let controller = GlobalShortcutController(store: store, registrar: registrar)
        let conflicting = GlobalShortcut(
            keyCode: UInt32(kVK_ANSI_R),
            modifiers: [.command, .option]
        )
        registrar.rejectedShortcut = conflicting

        controller.start {}
        XCTAssertFalse(controller.replace(with: conflicting))
        XCTAssertEqual(controller.current, .default)
        XCTAssertEqual(registrar.activeShortcuts, [.default])
        XCTAssertNil(store.load())
        XCTAssertNotNil(controller.errorMessage)
        controller.stop()
    }

    func testResetRestoresAndPersistsCommandShiftSpace() {
        let defaults = isolatedDefaults()
        let store = ShortcutPreferenceStore(defaults: defaults)
        let registrar = FakeHotKeyRegistrar()
        let controller = GlobalShortcutController(store: store, registrar: registrar)
        let chosen = GlobalShortcut(
            keyCode: UInt32(kVK_ANSI_S),
            modifiers: [.command, .control]
        )

        controller.start {}
        XCTAssertTrue(controller.replace(with: chosen))
        XCTAssertTrue(controller.resetToDefault())
        XCTAssertEqual(controller.current, .default)
        XCTAssertEqual(controller.current.displayName, "⌘⇧Space")
        XCTAssertEqual(store.load(), .default)
        XCTAssertEqual(registrar.activeShortcuts, [.default])
        controller.stop()
    }

    func testGlassRenderingPolicyUsesNativeOnlyWhenCompiledAndSupported() {
        XCTAssertEqual(
            SentinelGlassRenderingPolicy.mode(
                macOSMajorVersion: 26,
                nativeAPICompiled: true,
                reduceTransparency: false
            ),
            .nativeLiquidGlass
        )
        XCTAssertEqual(
            SentinelGlassRenderingPolicy.mode(
                macOSMajorVersion: 25,
                nativeAPICompiled: true,
                reduceTransparency: false
            ),
            .materialFallback
        )
        XCTAssertEqual(
            SentinelGlassRenderingPolicy.mode(
                macOSMajorVersion: 26,
                nativeAPICompiled: false,
                reduceTransparency: false
            ),
            .materialFallback
        )
        XCTAssertEqual(
            SentinelGlassRenderingPolicy.mode(
                macOSMajorVersion: 26,
                nativeAPICompiled: true,
                reduceTransparency: true
            ),
            .opaqueAccessible
        )
    }

    private func isolatedDefaults() -> UserDefaults {
        let suite = "SentinelMacTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defaults.removePersistentDomain(forName: suite)
        return defaults
    }
}

private final class FakeHotKeyRegistrar: GlobalHotKeyRegistering {
    enum Failure: Error {
        case conflict
    }

    var rejectedShortcut: GlobalShortcut?
    private(set) var unregisterCount = 0
    private var active: [ObjectIdentifier: GlobalShortcut] = [:]

    var activeShortcuts: [GlobalShortcut] {
        active.values.sorted { $0.displayName < $1.displayName }
    }

    func register(
        _ shortcut: GlobalShortcut,
        handler: @escaping () -> Void
    ) throws -> GlobalHotKeyRegistration {
        if shortcut == rejectedShortcut { throw Failure.conflict }
        let token = GlobalHotKeyRegistration()
        active[ObjectIdentifier(token)] = shortcut
        return token
    }

    func unregister(_ registration: GlobalHotKeyRegistration) {
        if active.removeValue(forKey: ObjectIdentifier(registration)) != nil {
            unregisterCount += 1
        }
    }
}
