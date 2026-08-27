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
    private let e2eApprovalLaunchConfiguration: E2EApprovalLaunchConfiguration
    private var e2eQuickPromptLaunchCoordinator = E2EQuickPromptLaunchCoordinator()
    private var e2eQuickPromptBridgeSubscription: AnyCancellable?
    private var terminationGate = ApplicationTerminationGate()

    init(e2eQuickPromptLaunchConfiguration: E2EQuickPromptLaunchConfiguration = E2EQuickPromptLaunchConfiguration(), e2eApprovalLaunchConfiguration: E2EApprovalLaunchConfiguration = E2EApprovalLaunchConfiguration()) {
        self.e2eQuickPromptLaunchConfiguration = e2eQuickPromptLaunchConfiguration
        self.e2eApprovalLaunchConfiguration = e2eApprovalLaunchConfiguration
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        bridge.start()
        installStatusItem()
        observeStatusItem()
        installPanels()
        openQuickPromptForE2ELaunchIfRequested()
        openApprovalForE2ELaunchIfRequested()
        if CommandLine.arguments.contains("--e2e-open-settings") {
            showSettings()
        }
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

    private func openApprovalForE2ELaunchIfRequested() {
        guard let taskID = e2eApprovalLaunchConfiguration.taskID, !taskID.isEmpty else { return }
        bridge.traceE2E("e2e-approval.begin", fields: ["task": taskID])
        // Register before sending: a local bridge can answer synchronously at
        // startup, and this test-only navigator must not lose that detail.
        bridge.$taskDetail
            .handleEvents(receiveSubscription: { [weak bridge] _ in
                bridge?.traceE2E("e2e-approval.observer.installed", fields: ["task": taskID])
            }, receiveOutput: { [weak bridge] detail in
                let matches = detail?.task.id == taskID
                let ready = detail?.task.isReadyForHuman == true
                let pending = detail?.actions.approve == true && detail?.actions.approvalID != nil
                bridge?.traceE2E("e2e-approval.observer.value", fields: [
                    "task": detail?.task.id ?? "nil",
                    "matches": String(matches),
                    "ready": String(ready),
                    "pending": String(pending),
                ])
            }, receiveCancel: { [weak bridge] in
                bridge?.traceE2E("e2e-approval.observer.cancelled")
            })
            .compactMap { $0 }
            .filter { detail in
                detail.task.id == taskID
                    && detail.task.isReadyForHuman
                    && detail.actions.approve
                    && detail.actions.approvalID != nil
            }
            .prefix(1)
            .receive(on: RunLoop.main)
            .sink { [weak self] detail in
                self?.bridge.traceE2E("e2e-approval.observer.matched", fields: ["task": detail.task.id])
                self?.showTaskDetail()
            }
            .store(in: &bridgeSubscriptions)
        let requestDetail = { [weak bridge] in
            bridge?.traceE2E("e2e-approval.requesting", fields: ["task": taskID])
            bridge?.loadTaskDetail(taskID: taskID)
        }
        if bridge.bridgeConnected {
            bridge.traceE2E("e2e-approval.bridge.ready", fields: ["task": taskID])
            requestDetail()
        }
        else {
            bridge.traceE2E("e2e-approval.bridge.waiting", fields: ["task": taskID])
            bridge.$bridgeConnected.filter { $0 }.prefix(1).sink { [weak bridge] _ in
                bridge?.traceE2E("e2e-approval.bridge.ready", fields: ["task": taskID])
                requestDetail()
            }.store(in: &bridgeSubscriptions)
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard terminationGate.begin() else { return .terminateLater }
        bridge.prepareForHostShutdown { [weak self] in
            guard let self, self.terminationGate.finish() else { return }
            NSApp.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
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
        let quitItem = menu.addItem(withTitle: "Quit Agent Sentinel", action: #selector(quit(_:)), keyEquivalent: "q")
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
    @objc private func quit(_ sender: Any?) {
        // Go through AppKit so applicationShouldTerminate owns the one safe,
        // bounded shutdown path for menu, Dock, and system quit requests.
        NSApp.terminate(sender)
    }

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
        bridge.traceE2E("task-detail.presentation.requested", fields: ["existing": String(detailWindow != nil)])
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
            bridge.traceE2E("task-detail.presentation.created", fields: ["windows": String(NSApp.windows.count)])
        }
        detailWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        bridge.traceE2E("task-detail.presentation.ordered", fields: [
            "visible": String(detailWindow?.isVisible == true),
            "key": String(detailWindow?.isKeyWindow == true),
            "main": String(detailWindow?.isMainWindow == true),
            "miniaturized": String(detailWindow?.isMiniaturized == true),
            "screen": String(detailWindow?.screen != nil),
            "windows": String(NSApp.windows.count),
        ])
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
