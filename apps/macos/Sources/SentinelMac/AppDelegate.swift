import AppKit
import Carbon.HIToolbox
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let bridge = NativeBridge()
    private var statusItem: NSStatusItem?
    private var quickPrompt: SentinelFloatingPanel?
    private var attention: SentinelFloatingPanel?
    private var detailWindow: NSWindow?
    private var statusWindow: NSWindow?
    private var settingsWindow: NSWindow?
    private var hotKeyRef: EventHotKeyRef?
    private var previousApplication: NSRunningApplication?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        bridge.start()
        installStatusItem()
        installPanels()
        installGlobalShortcut()
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = NSImage(systemSymbolName: "scope", accessibilityDescription: "Sentinel")
        item.button?.action = #selector(showQuickPrompt)
        item.button?.target = self
        let menu = NSMenu()
        let promptItem = menu.addItem(withTitle: "Quick Prompt", action: #selector(showQuickPrompt), keyEquivalent: "")
        promptItem.target = self
        let statusMenuItem = menu.addItem(withTitle: "Status", action: #selector(showStatus), keyEquivalent: "")
        statusMenuItem.target = self
        let settingsMenuItem = menu.addItem(withTitle: "Settings…", action: #selector(showSettings), keyEquivalent: ",")
        settingsMenuItem.target = self
        item.menu = menu
        statusItem = item
    }

    private func installPanels() {
        quickPrompt = SentinelFloatingPanel(title: "Quick Prompt") {
            QuickPromptView(bridge: self.bridge) { [weak self] in
                self?.quickPrompt?.orderOut(nil)
                self?.restorePreviousApplication()
            }
        }
        attention = SentinelFloatingPanel(title: "Sentinel") { AttentionWidgetView(bridge: self.bridge, openDetail: { self.showTaskDetail() }) }
        quickPrompt?.onHide = { [weak self] in self?.restorePreviousApplication() }
        attention?.onHide = { [weak self] in self?.restorePreviousApplication() }
    }

    @objc func showQuickPrompt() { present(quickPrompt) }
    @objc func showAttention() { present(attention) }

    @objc func showStatus() {
        if statusWindow == nil {
            statusWindow = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 360, height: 300),
                styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false
            )
            statusWindow?.title = "Sentinel Status"
            statusWindow?.contentView = NSHostingView(rootView: StatusView(bridge: bridge))
        }
        statusWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc func showSettings() {
        if settingsWindow == nil {
            settingsWindow = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 520, height: 470),
                styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
            )
            settingsWindow?.title = "Sentinel Settings"
            settingsWindow?.contentView = NSHostingView(rootView: SettingsView(bridge: bridge))
        }
        settingsWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func present(_ panel: NSPanel?) {
        guard let panel else { return }
        let frontmost = NSWorkspace.shared.frontmostApplication
        if frontmost?.processIdentifier != ProcessInfo.processInfo.processIdentifier {
            previousApplication = frontmost
        }
        panel.center()
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        if panel === quickPrompt {
            NotificationCenter.default.post(name: .sentinelQuickPromptShown, object: nil)
        }
    }

    private func restorePreviousApplication() {
        previousApplication?.activate(options: [])
    }

    func showTaskDetail() {
        if detailWindow == nil {
            detailWindow = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 620, height: 520),
                styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
            )
            detailWindow?.title = "Sentinel Task Detail"
            detailWindow?.contentView = NSHostingView(rootView: TaskDetailView(bridge: bridge))
        }
        detailWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func installGlobalShortcut() {
        var eventType = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: OSType(kEventHotKeyPressed))
        InstallEventHandler(GetApplicationEventTarget(), { _, _, userData in
            guard let userData else { return noErr }
            let delegate = Unmanaged<AppDelegate>.fromOpaque(userData).takeUnretainedValue()
            DispatchQueue.main.async { delegate.showQuickPrompt() }
            return noErr
        }, 1, &eventType, Unmanaged.passUnretained(self).toOpaque(), nil)
        let identifier = EventHotKeyID(signature: OSType(0x534E544C), id: 1)
        RegisterEventHotKey(UInt32(kVK_Space), UInt32(cmdKey | shiftKey), identifier, GetApplicationEventTarget(), 0, &hotKeyRef)
    }
}
