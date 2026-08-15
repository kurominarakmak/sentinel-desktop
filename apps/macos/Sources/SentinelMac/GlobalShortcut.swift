import AppKit
import Carbon.HIToolbox
import Combine

struct ShortcutModifiers: OptionSet, Codable, Hashable {
    let rawValue: UInt32

    static let command = ShortcutModifiers(rawValue: 1 << 0)
    static let shift = ShortcutModifiers(rawValue: 1 << 1)
    static let option = ShortcutModifiers(rawValue: 1 << 2)
    static let control = ShortcutModifiers(rawValue: 1 << 3)

    init(rawValue: UInt32) {
        self.rawValue = rawValue
    }

    init(_ flags: NSEvent.ModifierFlags) {
        var value: ShortcutModifiers = []
        if flags.contains(.command) { value.insert(.command) }
        if flags.contains(.shift) { value.insert(.shift) }
        if flags.contains(.option) { value.insert(.option) }
        if flags.contains(.control) { value.insert(.control) }
        self = value
    }

    var carbonValue: UInt32 {
        var value: UInt32 = 0
        if contains(.command) { value |= UInt32(cmdKey) }
        if contains(.shift) { value |= UInt32(shiftKey) }
        if contains(.option) { value |= UInt32(optionKey) }
        if contains(.control) { value |= UInt32(controlKey) }
        return value
    }
}

struct GlobalShortcut: Codable, Equatable, Hashable {
    let keyCode: UInt32
    let modifiers: ShortcutModifiers

    static let `default` = GlobalShortcut(
        keyCode: UInt32(kVK_Space),
        modifiers: [.command, .shift]
    )

    var displayName: String {
        var value = ""
        if modifiers.contains(.command) { value += "⌘" }
        if modifiers.contains(.shift) { value += "⇧" }
        if modifiers.contains(.option) { value += "⌥" }
        if modifiers.contains(.control) { value += "⌃" }
        return value + keyName
    }

    private var keyName: String {
        switch Int(keyCode) {
        case kVK_Space: "Space"
        case kVK_Return: "Return"
        case kVK_Tab: "Tab"
        case kVK_Delete: "Delete"
        case kVK_ForwardDelete: "Forward Delete"
        case kVK_LeftArrow: "←"
        case kVK_RightArrow: "→"
        case kVK_UpArrow: "↑"
        case kVK_DownArrow: "↓"
        case kVK_Home: "Home"
        case kVK_End: "End"
        case kVK_PageUp: "Page Up"
        case kVK_PageDown: "Page Down"
        case kVK_F1: "F1"
        case kVK_F2: "F2"
        case kVK_F3: "F3"
        case kVK_F4: "F4"
        case kVK_F5: "F5"
        case kVK_F6: "F6"
        case kVK_F7: "F7"
        case kVK_F8: "F8"
        case kVK_F9: "F9"
        case kVK_F10: "F10"
        case kVK_F11: "F11"
        case kVK_F12: "F12"
        case kVK_ANSI_A: "A"
        case kVK_ANSI_B: "B"
        case kVK_ANSI_C: "C"
        case kVK_ANSI_D: "D"
        case kVK_ANSI_E: "E"
        case kVK_ANSI_F: "F"
        case kVK_ANSI_G: "G"
        case kVK_ANSI_H: "H"
        case kVK_ANSI_I: "I"
        case kVK_ANSI_J: "J"
        case kVK_ANSI_K: "K"
        case kVK_ANSI_L: "L"
        case kVK_ANSI_M: "M"
        case kVK_ANSI_N: "N"
        case kVK_ANSI_O: "O"
        case kVK_ANSI_P: "P"
        case kVK_ANSI_Q: "Q"
        case kVK_ANSI_R: "R"
        case kVK_ANSI_S: "S"
        case kVK_ANSI_T: "T"
        case kVK_ANSI_U: "U"
        case kVK_ANSI_V: "V"
        case kVK_ANSI_W: "W"
        case kVK_ANSI_X: "X"
        case kVK_ANSI_Y: "Y"
        case kVK_ANSI_Z: "Z"
        case kVK_ANSI_0: "0"
        case kVK_ANSI_1: "1"
        case kVK_ANSI_2: "2"
        case kVK_ANSI_3: "3"
        case kVK_ANSI_4: "4"
        case kVK_ANSI_5: "5"
        case kVK_ANSI_6: "6"
        case kVK_ANSI_7: "7"
        case kVK_ANSI_8: "8"
        case kVK_ANSI_9: "9"
        case kVK_ANSI_Minus: "-"
        case kVK_ANSI_Equal: "="
        case kVK_ANSI_LeftBracket: "["
        case kVK_ANSI_RightBracket: "]"
        case kVK_ANSI_Semicolon: ";"
        case kVK_ANSI_Quote: "'"
        case kVK_ANSI_Comma: ","
        case kVK_ANSI_Period: "."
        case kVK_ANSI_Slash: "/"
        case kVK_ANSI_Backslash: "\\"
        case kVK_ANSI_Grave: "`"
        default: "Key \(keyCode)"
        }
    }
}

enum ShortcutValidationError: LocalizedError, Equatable {
    case unsupportedKey
    case missingPrimaryModifier
    case reservedCombination

    var errorDescription: String? {
        switch self {
        case .unsupportedKey:
            "Choose a non-modifier key."
        case .missingPrimaryModifier:
            "Use Command, Option, or Control with the key."
        case .reservedCombination:
            "That shortcut is reserved by macOS or standard app behavior."
        }
    }
}

enum GlobalShortcutValidator {
    static func validate(_ shortcut: GlobalShortcut) throws {
        let modifierKeyCodes: Set<UInt32> = [54, 55, 56, 57, 58, 59, 60, 61, 62, 63]
        guard shortcut.keyCode <= 127,
              shortcut.keyCode != UInt32(kVK_Escape),
              !modifierKeyCodes.contains(shortcut.keyCode) else {
            throw ShortcutValidationError.unsupportedKey
        }
        guard !shortcut.modifiers.intersection([.command, .option, .control]).isEmpty else {
            throw ShortcutValidationError.missingPrimaryModifier
        }
        let commandOnly = shortcut.modifiers == [.command]
        let standardAppKeys: Set<UInt32> = [
            UInt32(kVK_ANSI_Q), UInt32(kVK_ANSI_W), UInt32(kVK_ANSI_H),
            UInt32(kVK_ANSI_M), UInt32(kVK_Tab),
        ]
        guard !(commandOnly && standardAppKeys.contains(shortcut.keyCode)) else {
            throw ShortcutValidationError.reservedCombination
        }
    }
}

final class ShortcutPreferenceStore {
    static let storageKey = "native.quickPromptShortcut.v1"

    private let defaults: UserDefaults
    private let key: String

    init(defaults: UserDefaults = .standard, key: String = ShortcutPreferenceStore.storageKey) {
        self.defaults = defaults
        self.key = key
    }

    func load() -> GlobalShortcut? {
        guard let data = defaults.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(GlobalShortcut.self, from: data)
    }

    func save(_ shortcut: GlobalShortcut) {
        guard let data = try? JSONEncoder().encode(shortcut) else { return }
        defaults.set(data, forKey: key)
    }
}

final class GlobalHotKeyRegistration {
    fileprivate var carbonReference: EventHotKeyRef?

    init(carbonReference: EventHotKeyRef? = nil) {
        self.carbonReference = carbonReference
    }
}

protocol GlobalHotKeyRegistering: AnyObject {
    func register(_ shortcut: GlobalShortcut, handler: @escaping () -> Void) throws -> GlobalHotKeyRegistration
    func unregister(_ registration: GlobalHotKeyRegistration)
}

enum GlobalHotKeyRegistrationError: LocalizedError {
    case eventHandler(OSStatus)
    case registration(OSStatus)

    var errorDescription: String? {
        switch self {
        case .eventHandler:
            "The macOS shortcut handler is unavailable."
        case .registration:
            "That shortcut could not be registered. It may already be in use."
        }
    }
}

final class CarbonGlobalHotKeyRegistrar: GlobalHotKeyRegistering {
    private var eventHandlerReference: EventHandlerRef?
    private var activeRegistration: GlobalHotKeyRegistration?
    private var invocation: (() -> Void)?

    deinit {
        if let activeRegistration { unregister(activeRegistration) }
        if let eventHandlerReference { RemoveEventHandler(eventHandlerReference) }
    }

    func register(_ shortcut: GlobalShortcut, handler: @escaping () -> Void) throws -> GlobalHotKeyRegistration {
        precondition(activeRegistration == nil, "Only one global shortcut may be registered")
        try installEventHandlerIfNeeded()

        let identifier = EventHotKeyID(signature: OSType(0x534E544C), id: 1)
        var reference: EventHotKeyRef?
        let status = RegisterEventHotKey(
            shortcut.keyCode,
            shortcut.modifiers.carbonValue,
            identifier,
            GetApplicationEventTarget(),
            0,
            &reference
        )
        guard status == noErr, reference != nil else {
            throw GlobalHotKeyRegistrationError.registration(status)
        }
        invocation = handler
        let registration = GlobalHotKeyRegistration(carbonReference: reference)
        activeRegistration = registration
        return registration
    }

    func unregister(_ registration: GlobalHotKeyRegistration) {
        guard activeRegistration === registration else { return }
        if let reference = registration.carbonReference {
            UnregisterEventHotKey(reference)
            registration.carbonReference = nil
        }
        activeRegistration = nil
        invocation = nil
    }

    private func installEventHandlerIfNeeded() throws {
        guard eventHandlerReference == nil else { return }
        var eventType = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: OSType(kEventHotKeyPressed)
        )
        let status = InstallEventHandler(
            GetApplicationEventTarget(),
            { _, _, userData in
                guard let userData else { return noErr }
                let registrar = Unmanaged<CarbonGlobalHotKeyRegistrar>
                    .fromOpaque(userData)
                    .takeUnretainedValue()
                DispatchQueue.main.async { registrar.invocation?() }
                return noErr
            },
            1,
            &eventType,
            Unmanaged.passUnretained(self).toOpaque(),
            &eventHandlerReference
        )
        guard status == noErr else {
            throw GlobalHotKeyRegistrationError.eventHandler(status)
        }
    }
}

final class GlobalShortcutController: ObservableObject {
    @Published private(set) var current: GlobalShortcut
    @Published private(set) var errorMessage: String?
    @Published private(set) var isRecording = false

    private let store: ShortcutPreferenceStore
    private let registrar: GlobalHotKeyRegistering
    private var registration: GlobalHotKeyRegistration?
    private var eventMonitor: Any?
    private var invocation: (() -> Void)?

    init(
        store: ShortcutPreferenceStore = ShortcutPreferenceStore(),
        registrar: GlobalHotKeyRegistering = CarbonGlobalHotKeyRegistrar()
    ) {
        self.store = store
        self.registrar = registrar
        let persisted = store.load()
        if let persisted, (try? GlobalShortcutValidator.validate(persisted)) != nil {
            current = persisted
        } else {
            current = .default
            if persisted != nil { store.save(.default) }
        }
    }

    func start(handler: @escaping () -> Void) {
        invocation = handler
        guard registration == nil else { return }
        restoreCurrentRegistration()
    }

    func stop() {
        removeEventMonitor()
        if let registration { registrar.unregister(registration) }
        registration = nil
        invocation = nil
        isRecording = false
    }

    @discardableResult
    func replace(with shortcut: GlobalShortcut) -> Bool {
        do {
            try GlobalShortcutValidator.validate(shortcut)
        } catch {
            errorMessage = error.localizedDescription
            restoreCurrentRegistration()
            return false
        }
        guard shortcut != current else {
            errorMessage = nil
            restoreCurrentRegistration()
            return true
        }

        let previous = current
        suspendCurrentRegistration()
        do {
            registration = try registrar.register(shortcut, handler: registeredHandler())
            current = shortcut
            store.save(shortcut)
            errorMessage = nil
            return true
        } catch {
            current = previous
            errorMessage = error.localizedDescription
            restoreCurrentRegistration()
            return false
        }
    }

    @discardableResult
    func resetToDefault() -> Bool {
        replace(with: .default)
    }

    func beginRecording() {
        guard !isRecording else {
            cancelRecording()
            return
        }
        isRecording = true
        errorMessage = nil
        suspendCurrentRegistration()
        eventMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.isRecording else { return event }
            if event.keyCode == UInt16(kVK_Escape) {
                self.cancelRecording()
                return nil
            }
            let shortcut = GlobalShortcut(
                keyCode: UInt32(event.keyCode),
                modifiers: ShortcutModifiers(event.modifierFlags)
            )
            self.finishRecording(with: shortcut)
            return nil
        }
    }

    func cancelRecording() {
        guard isRecording else { return }
        removeEventMonitor()
        isRecording = false
        restoreCurrentRegistration()
    }

    private func finishRecording(with shortcut: GlobalShortcut) {
        removeEventMonitor()
        isRecording = false
        _ = replace(with: shortcut)
    }

    private func suspendCurrentRegistration() {
        if let registration { registrar.unregister(registration) }
        registration = nil
    }

    private func restoreCurrentRegistration() {
        guard registration == nil, invocation != nil else { return }
        do {
            registration = try registrar.register(current, handler: registeredHandler())
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func registeredHandler() -> () -> Void {
        { [weak self] in self?.invocation?() }
    }

    private func removeEventMonitor() {
        if let eventMonitor { NSEvent.removeMonitor(eventMonitor) }
        eventMonitor = nil
    }
}
