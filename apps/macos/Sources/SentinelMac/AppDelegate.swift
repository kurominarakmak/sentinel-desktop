import AppKit
import Combine
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    let bridge = NativeBridge()
    private var statusItem: NSStatusItem?
    private var quickPrompt: SentinelFloatingPanel?
    private var attention: SentinelFloatingPanel?
    private var detailWindow: NSWindow?
    private var statusWindow: NSWindow?
    private var settingsWindow: NSWindow?
    private var quickPromptMenuItem: NSMenuItem?
    private var surfaceRegistry = NativeSurfaceRegistry()
    private var previousApplicationFocus = PreviousApplicationFocus()
    private var bridgeSubscriptions = Set<AnyCancellable>()
    private let shortcutController = GlobalShortcutController()
    private let e2eQuickPromptLaunchConfiguration: E2EQuickPromptLaunchConfiguration
    private var e2eQuickPromptLaunchCoordinator = E2EQuickPromptLaunchCoordinator()
    private var e2eQuickPromptBridgeSubscription: AnyCancellable?

    init(e2eQuickPromptLaunchConfiguration: E2EQuickPromptLaunchConfiguration = E2EQuickPromptLaunchConfiguration()) {
        self.e2eQuickPromptLaunchConfiguration = e2eQuickPromptLaunchConfiguration
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        bridge.start()
        installStatusItem()
        observeStatusItem()
        installPanels()
        openQuickPromptForE2ELaunchIfRequested()
        installGlobalShortcut()
        observeGlobalShortcut()
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(attentionActionAccepted(_:)),
            name: .sentinelAttentionActionAccepted,
            object: nil
        )
        NSWorkspace.shared.notificationCenter.addObserver(
            self,
            selector: #selector(accessibilityDisplayOptionsChanged),
            name: NSWorkspace.accessibilityDisplayOptionsDidChangeNotification,
            object: nil
        )
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        .terminateNow
    }

    func applicationWillTerminate(_ notification: Notification) {
        shortcutController.stop()
        bridge.stop()
        NotificationCenter.default.removeObserver(self)
        NSWorkspace.shared.notificationCenter.removeObserver(self)
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
        quickPromptMenuItem = promptItem
        let statusMenuItem = menu.addItem(withTitle: "Status", action: #selector(showStatus), keyEquivalent: "")
        statusMenuItem.target = self
        let attentionMenuItem = menu.addItem(withTitle: "Attention", action: #selector(showAttention), keyEquivalent: "")
        attentionMenuItem.target = self
        let settingsMenuItem = menu.addItem(withTitle: "Settings…", action: #selector(showSettings), keyEquivalent: ",")
        settingsMenuItem.target = self
        menu.addItem(.separator())
        let quitItem = menu.addItem(withTitle: "Quit Agent Sentinel", action: #selector(quit), keyEquivalent: "q")
        quitItem.target = self
        item.menu = menu
        menu.delegate = self
        statusItem = item
    }

    private func observeStatusItem() {
        bridge.$status
            .receive(on: RunLoop.main)
            .sink { [weak self] status in self?.updateStatusItem(status) }
            .store(in: &bridgeSubscriptions)
    }

    private func updateStatusItem(_ status: NativeStatus?) {
        let title = CodexTrayTitle.make(status)
        statusItem?.button?.title = title
        statusItem?.button?.toolTip = title == "—" ? "Codex usage unavailable" : "Codex usage \(title)"
    }

    func menuWillOpen(_ menu: NSMenu) {
        bridge.loadStatus()
    }

    private func codexTrayImage() -> NSImage? {
        let candidates = [
            Bundle.main.url(forResource: "codex-tray", withExtension: "png"),
            Bundle.main.url(forResource: "codex-mark", withExtension: "svg"),
            ProcessInfo.processInfo.environment["SENTINEL_CODEX_TRAY_ICON"].map(URL.init(fileURLWithPath:)),
        ].compactMap { $0 }
        return candidates.lazy.compactMap(NSImage.init(contentsOf:)).first.map { image in
            image.isTemplate = true
            image.accessibilityDescription = "Codex"
            return image
        }
    }

    private func installPanels() {
        _ = surfaceRegistry.requestOpen(.quickPrompt)
        quickPrompt = SentinelFloatingPanel(title: "Quick Prompt", height: 470) {
            QuickPromptView(bridge: self.bridge) { [weak self] in
                self?.quickPrompt?.hide()
            }
        }
        _ = surfaceRegistry.requestOpen(.attention)
        attention = SentinelFloatingPanel(title: "Sentinel") { AttentionWidgetView(bridge: self.bridge, openDetail: { self.showTaskDetail() }) }
        quickPrompt?.onHide = { [weak self] in self?.restorePreviousApplication() }
        attention?.onHide = { [weak self] in self?.restorePreviousApplication() }
    }

    private func openQuickPromptForE2ELaunchIfRequested() {
        switch e2eQuickPromptLaunchCoordinator.request(
            configuration: e2eQuickPromptLaunchConfiguration,
            bridgeConnected: bridge.bridgeConnected
        ) {
        case .none:
            break
        case .presentQuickPrompt:
            // Defer until startup has installed the existing panel and bridge
            // snapshots. This is exactly the same controller route as the
            // status item and global shortcut.
            DispatchQueue.main.async { [weak self] in self?.showQuickPrompt() }
        case .waitForBridge:
            e2eQuickPromptBridgeSubscription = bridge.$bridgeConnected
                .filter { $0 }
                .prefix(1)
                .receive(on: RunLoop.main)
                .sink { [weak self] _ in
                    guard let self else { return }
                    if self.e2eQuickPromptLaunchCoordinator.bridgeDidConnect() == .presentQuickPrompt {
                        self.showQuickPrompt()
                    }
                    self.e2eQuickPromptBridgeSubscription = nil
                }
        }
    }

    @objc func showQuickPrompt() { present(quickPrompt) }
    @objc func showAttention() { present(attention) }
    @objc private func attentionActionAccepted(_ notification: Notification) {
        if notification.object as? Bool == true {
            attention?.hide()
        } else {
            attention?.orderOut(nil)
        }
    }
    @objc private func quit() { NSApp.terminate(nil) }

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
            configureInspectionWindow(statusWindow)
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
            settingsWindow?.contentView = NSHostingView(
                rootView: SettingsView(bridge: bridge, shortcutController: shortcutController)
            )
            configureInspectionWindow(settingsWindow)
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
            configureInspectionWindow(detailWindow)
        }
        detailWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func installGlobalShortcut() {
        shortcutController.start { [weak self] in self?.showQuickPrompt() }
    }

    private func observeGlobalShortcut() {
        shortcutController.$current
            .receive(on: RunLoop.main)
            .sink { [weak self] shortcut in
                self?.quickPromptMenuItem?.title = "Quick Prompt  \(shortcut.displayName)"
            }
            .store(in: &bridgeSubscriptions)
    }

    private func configureInspectionWindow(_ window: NSWindow?) {
        let reduceTransparency = NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency
        window?.isOpaque = reduceTransparency
        window?.backgroundColor = reduceTransparency ? .windowBackgroundColor : .clear
        window?.titlebarAppearsTransparent = true
        window?.titlebarSeparatorStyle = .none
    }

    @objc private func accessibilityDisplayOptionsChanged() {
        configureInspectionWindow(detailWindow)
        configureInspectionWindow(statusWindow)
        configureInspectionWindow(settingsWindow)
    }
}
