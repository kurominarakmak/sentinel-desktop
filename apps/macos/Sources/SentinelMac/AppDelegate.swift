import AppKit
import Carbon.HIToolbox
import Combine
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
    private var surfaceRegistry = NativeSurfaceRegistry()
    private var previousApplicationFocus = PreviousApplicationFocus()
    private var bridgeSubscriptions = Set<AnyCancellable>()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        bridge.start()
        installStatusItem()
        observeStatusItem()
        installPanels()
        installGlobalShortcut()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        .terminateCancel
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = codexTrayImage() ?? NSImage(systemSymbolName: "chevron.left.forwardslash.chevron.right", accessibilityDescription: "Codex")
        item.button?.image?.size = NSSize(width: 18, height: 18)
        item.button?.imagePosition = .imageLeft
        item.button?.title = "—"
        item.button?.action = #selector(showQuickPrompt)
        item.button?.target = self
        let menu = NSMenu()
        let promptItem = menu.addItem(withTitle: "Quick Prompt", action: #selector(showQuickPrompt), keyEquivalent: "")
        promptItem.target = self
        let statusMenuItem = menu.addItem(withTitle: "Status", action: #selector(showStatus), keyEquivalent: "")
        statusMenuItem.target = self
        let attentionMenuItem = menu.addItem(withTitle: "Attention", action: #selector(showAttention), keyEquivalent: "")
        attentionMenuItem.target = self
        let settingsMenuItem = menu.addItem(withTitle: "Settings…", action: #selector(showSettings), keyEquivalent: ",")
        settingsMenuItem.target = self
        item.menu = menu
        statusItem = item
    }

    private func observeStatusItem() {
        bridge.$status
            .receive(on: RunLoop.main)
            .sink { [weak self] status in self?.updateStatusItem(status) }
            .store(in: &bridgeSubscriptions)
    }

    private func updateStatusItem(_ status: NativeStatus?) {
        statusItem?.button?.title = CodexTrayTitle.make(status)
    }

    private func codexTrayImage() -> NSImage? {
        let sourceRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let candidates = [
            Bundle.main.url(forResource: "codex-tray", withExtension: "png"),
            ProcessInfo.processInfo.environment["SENTINEL_CODEX_TRAY_ICON"].map(URL.init(fileURLWithPath:)),
            sourceRoot.appendingPathComponent("apps/desktop/src-tauri/icons/codex-tray.png"),
        ].compactMap { $0 }
        return candidates.lazy.compactMap(NSImage.init(contentsOf:)).first
    }

    private func installPanels() {
        _ = surfaceRegistry.requestOpen(.quickPrompt)
        quickPrompt = SentinelFloatingPanel(title: "Quick Prompt") {
            QuickPromptView(bridge: self.bridge) { [weak self] in
                self?.quickPrompt?.hide()
            }
        }
        _ = surfaceRegistry.requestOpen(.attention)
        attention = SentinelFloatingPanel(title: "Sentinel") { AttentionWidgetView(bridge: self.bridge, openDetail: { self.showTaskDetail() }) }
        quickPrompt?.onHide = { [weak self] in self?.restorePreviousApplication() }
        attention?.onHide = { [weak self] in self?.restorePreviousApplication() }
    }

    @objc func showQuickPrompt() { present(quickPrompt) }
    @objc func showAttention() { present(attention) }

    @objc func showStatus() {
        if statusWindow == nil {
            _ = surfaceRegistry.requestOpen(.status)
            statusWindow = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 360, height: 300),
                styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false
            )
            statusWindow?.title = "Sentinel Status"
            statusWindow?.isReleasedWhenClosed = false
            statusWindow?.contentView = NSHostingView(rootView: StatusView(bridge: bridge))
        }
        statusWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc func showSettings() {
        if settingsWindow == nil {
            _ = surfaceRegistry.requestOpen(.settings)
            settingsWindow = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 520, height: 470),
                styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
            )
            settingsWindow?.title = "Sentinel Settings"
            settingsWindow?.isReleasedWhenClosed = false
            settingsWindow?.contentView = NSHostingView(rootView: SettingsView(bridge: bridge))
        }
        settingsWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func present(_ panel: NSPanel?) {
        guard let panel else { return }
        let frontmost = NSWorkspace.shared.frontmostApplication
        if !panel.isVisible {
            previousApplicationFocus.capture(
                frontmost: frontmost,
                sentinelPID: ProcessInfo.processInfo.processIdentifier
            )
            place(panel: panel)
        }
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        if panel === quickPrompt {
            NotificationCenter.default.post(name: .sentinelQuickPromptShown, object: nil)
        }
    }

    private func restorePreviousApplication() {
        let sentinelPID = ProcessInfo.processInfo.processIdentifier
        let running = Set(NSWorkspace.shared.runningApplications.map(\.processIdentifier))
        guard let pid = previousApplicationFocus.takeRestoreCandidate(
            runningProcessIdentifiers: running,
            sentinelPID: sentinelPID
        ), let application = NSRunningApplication(processIdentifier: pid), !application.isTerminated else { return }
        application.activate(options: [])
    }

    private func place(panel: NSPanel) {
        let preferred = NSScreen.main?.visibleFrame ?? NSScreen.screens.first?.visibleFrame ?? panel.frame
        let visible = NSScreen.screens.map(\.visibleFrame)
        if let floating = panel as? SentinelFloatingPanel, !floating.hasBeenPresented {
            panel.setFrame(FloatingPanelPlacement.centeredFrame(size: panel.frame.size, preferred: preferred), display: false)
            floating.markPresented()
            return
        }
        if visible.contains(where: { $0.intersects(panel.frame) }) {
            return
        }
        panel.setFrame(FloatingPanelPlacement.correctedFrame(panel.frame, visibleFrames: visible, preferred: preferred), display: false)
    }

    func showTaskDetail() {
        if detailWindow == nil {
            _ = surfaceRegistry.requestOpen(.taskDetail)
            detailWindow = NSWindow(
                contentRect: NSRect(x: 0, y: 0, width: 620, height: 520),
                styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false
            )
            detailWindow?.title = "Sentinel Task Detail"
            detailWindow?.isReleasedWhenClosed = false
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
