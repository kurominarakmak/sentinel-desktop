import AppKit
import Combine
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    let bridge = NativeBridge()
    private var statusItem: NSStatusItem?
    private var quickPrompt: SentinelFloatingPanel?
    private var detailWindow: NSWindow?
    private var statusWindow: NSWindow?
    private var settingsWindow: NSWindow?
    private var statusHasReceivedInitialPlacement = false
    private var settingsHasReceivedInitialPlacement = false
    private var taskDetailHasReceivedInitialPlacement = false
    private var lastQuickPromptScreen: NSScreen?
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
    private var doneStatusExpiresAt: Date?
    private var doneStatusWorkItem: DispatchWorkItem?

    init(e2eQuickPromptLaunchConfiguration: E2EQuickPromptLaunchConfiguration = E2EQuickPromptLaunchConfiguration(), e2eApprovalLaunchConfiguration: E2EApprovalLaunchConfiguration = E2EApprovalLaunchConfiguration()) {
        self.e2eQuickPromptLaunchConfiguration = e2eQuickPromptLaunchConfiguration
        self.e2eApprovalLaunchConfiguration = e2eApprovalLaunchConfiguration
        super.init()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        installMainMenu()
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
        item.button?.image = codexTrayImage() ?? NSImage(systemSymbolName: "sparkle", accessibilityDescription: "Sentinel")
        item.button?.image?.size = NSSize(width: 18, height: 18)
        item.button?.imagePosition = .imageLeft
        item.button?.title = "—"
        item.button?.action = #selector(showQuickPromptFromStatusItem)
        item.button?.target = self
        let menu = NSMenu()
        let promptItem = menu.addItem(withTitle: "Open Sentinel", action: #selector(showQuickPromptFromStatusItem), keyEquivalent: "")
        promptItem.target = self
        quickPromptMenuItem = promptItem
        let statusMenuItem = menu.addItem(withTitle: "Status", action: #selector(showStatus), keyEquivalent: "")
        statusMenuItem.target = self
        let attentionMenuItem = menu.addItem(withTitle: "Needs Attention", action: #selector(showAttention), keyEquivalent: "")
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
        Publishers.CombineLatest(
            Publishers.CombineLatest4(bridge.$status, bridge.$attention, bridge.$quickPromptPreferences, bridge.$modelCatalogs),
            bridge.$lastUsedModel
        )
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateStatusItem() }
            .store(in: &bridgeSubscriptions)
    }

    private func updateStatusItem() {
        updateDoneCooldown()
        let title = StatusBarPresentation.make(
            status: bridge.status,
            attention: bridge.attention,
            idleTitle: bridge.usagePresentation.idleItem(preferred: bridge.preferredIdleModel),
            showDone: doneStatusExpiresAt.map { $0 > Date() } ?? true
        )
        statusItem?.button?.title = title
        statusItem?.button?.toolTip = title == "✦" ? "Sentinel" : title
    }

    private func updateDoneCooldown() {
        let isDone = ["completed", "done", "complete"].contains(bridge.status?.activeTask?.lifecycle.lowercased() ?? "")
        guard isDone else {
            doneStatusExpiresAt = nil
            doneStatusWorkItem?.cancel()
            doneStatusWorkItem = nil
            return
        }
        guard doneStatusExpiresAt == nil else { return }
        doneStatusExpiresAt = Date().addingTimeInterval(2)
        let work = DispatchWorkItem { [weak self] in self?.updateStatusItem() }
        doneStatusWorkItem = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 2, execute: work)
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
            image.accessibilityDescription = "Sentinel"
            return image
        }
    }

    private func installPanels() {
        _ = surfaceRegistry.requestOpen(.quickPrompt)
        quickPrompt = SentinelFloatingPanel(title: "Sentinel", height: 76) {
            QuickPromptView(
                bridge: self.bridge,
                accepted: { [weak self] in self?.quickPrompt?.hide() },
                openTaskDetail: { [weak self] in self?.showTaskDetail() }
            )
        }
        quickPrompt?.onHide = { [weak self] in self?.restorePreviousApplication() }
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(resizeQuickPrompt(_:)),
            name: .sentinelQuickPromptPreferredHeight,
            object: nil
        )
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

    @objc func showQuickPrompt() { present(quickPrompt, preferredScreen: contextualScreen()) }
    @objc private func showQuickPromptFromStatusItem() {
        present(quickPrompt, preferredScreen: statusItem?.button?.window?.screen ?? contextualScreen())
    }
    @objc private func resizeQuickPrompt(_ notification: Notification) {
        guard let height = notification.object as? CGFloat else { return }
        quickPrompt?.resizeForQuickPrompt(height: height)
    }
    @objc func showAttention() {
        present(quickPrompt, preferredScreen: statusItem?.button?.window?.screen ?? contextualScreen())
        NotificationCenter.default.post(name: .sentinelNeedsAttentionShown, object: nil)
    }
    @objc private func quit(_ sender: Any?) {
        // Go through AppKit so applicationShouldTerminate owns the one safe,
        // bounded shutdown path for menu, Dock, and system quit requests.
        NSApp.terminate(sender)
    }

    @objc func showStatus() {
        if statusWindow == nil {
            statusWindow = SentinelStatusWindow(
                contentRect: NSRect(x: 0, y: 0, width: 360, height: 300),
                styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false
            )
            statusWindow?.title = "Sentinel Status"
            statusWindow?.isReleasedWhenClosed = false
            statusWindow?.contentView = NSHostingView(rootView: StatusView(bridge: bridge))
            configureInspectionWindow(statusWindow)
        }
        // Status is a conventional utility window, not the menu-bar panel,
        // but it must never inherit AppKit's `(0, 0)` creation origin.
        placeStandardWindowIfNeeded(statusWindow, hasReceivedInitialPlacement: &statusHasReceivedInitialPlacement)
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
        placeStandardWindowIfNeeded(settingsWindow, hasReceivedInitialPlacement: &settingsHasReceivedInitialPlacement)
        settingsWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func present(_ panel: NSPanel?, preferredScreen: NSScreen? = nil) {
        guard let panel else { return }
        let frontmost = NSWorkspace.shared.frontmostApplication
        if !panel.isVisible {
            previousApplicationFocus.capture(
                frontmost: frontmost,
                sentinelPID: ProcessInfo.processInfo.processIdentifier
            )
            place(panel: panel, preferredScreen: preferredScreen)
        } else if panel === quickPrompt, let preferredScreen,
                  panel.screen !== preferredScreen {
            // Reuse the one production panel, but follow an invocation from a
            // different display rather than leaving the utility stranded.
            place(panel: panel, preferredScreen: preferredScreen)
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

    private func contextualScreen() -> NSScreen? {
        if let activeScreen = frontmostApplicationScreen() { return activeScreen }
        let mouse = NSEvent.mouseLocation
        return NSScreen.screens.first(where: { $0.frame.contains(mouse) })
            ?? lastQuickPromptScreen.flatMap { screen in NSScreen.screens.contains(screen) ? screen : nil }
            ?? NSScreen.main
            ?? NSScreen.screens.first
    }

    /// AppKit does not expose other apps' key windows. Window Server metadata
    /// gives us a best-effort active-window rectangle; permission or a missing
    /// rectangle simply falls through to the pointer screen below.
    private func frontmostApplicationScreen() -> NSScreen? {
        guard let process = NSWorkspace.shared.frontmostApplication else { return nil }
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
            as? [[String: Any]] ?? []
        guard let boundsValue = windows.first(where: {
            ($0[kCGWindowOwnerPID as String] as? pid_t) == process.processIdentifier
                && ($0[kCGWindowLayer as String] as? Int ?? 1) == 0
        })?[kCGWindowBounds as String] else { return nil }
        let bounds = boundsValue as! CFDictionary
        guard let windowFrame = CGRect(dictionaryRepresentation: bounds) else { return nil }
        return NSScreen.screens.max { lhs, rhs in
            lhs.frame.intersection(windowFrame).area < rhs.frame.intersection(windowFrame).area
        }
    }

    private func place(panel: NSPanel, preferredScreen: NSScreen? = nil) {
        let preferred = preferredScreen?.visibleFrame
            ?? contextualScreen()?.visibleFrame
            ?? NSScreen.main?.visibleFrame
            ?? NSScreen.screens.first?.visibleFrame
            ?? panel.frame
        let visible = NSScreen.screens.map(\.visibleFrame)
        if let floating = panel as? SentinelFloatingPanel, panel === quickPrompt {
            panel.setFrame(FloatingPanelPlacement.topRightFrame(size: panel.frame.size, preferred: preferred), display: false)
            floating.markPresented()
            lastQuickPromptScreen = preferredScreen
                ?? NSScreen.screens.first(where: { $0.visibleFrame == preferred })
            return
        }
        if visible.contains(where: { $0.intersects(panel.frame) }) {
            return
        }
        panel.setFrame(FloatingPanelPlacement.correctedFrame(panel.frame, visibleFrames: visible, preferred: preferred), display: false)
    }

    private func placeStandardWindowIfNeeded(
        _ window: NSWindow?,
        hasReceivedInitialPlacement: inout Bool
    ) {
        guard let window else { return }
        // An AppKit-created window can retain its literal `(0, 0)` creation
        // origin if presentation races activation. Treat only that fallback as
        // unplaced; otherwise retain the position the person chose.
        let needsPlacement = !hasReceivedInitialPlacement || window.frame.origin == .zero
        guard needsPlacement else { return }
        let preferred = contextualScreen()?.visibleFrame
            ?? NSScreen.main?.visibleFrame
            ?? window.screen?.visibleFrame
            ?? window.frame
        window.setFrame(NativeWindowPlacement.initialFrame(size: window.frame.size, preferred: preferred), display: false)
        hasReceivedInitialPlacement = true
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
        placeStandardWindowIfNeeded(detailWindow, hasReceivedInitialPlacement: &taskDetailHasReceivedInitialPlacement)
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
                self?.quickPromptMenuItem?.title = "Open Sentinel  \(shortcut.displayName)"
            }
            .store(in: &bridgeSubscriptions)
    }


    /// Accessory apps get no main menu, which kills every keyboard equivalent
    /// that AppKit routes through the menu system (⌘W in particular). Install
    /// a hidden one so window-close keys reach `performClose(_:)` on the key
    /// window; panels hide through their `close()` override.
    private func installMainMenu() {
        let windowMenu = NSMenu(title: "Window")
        windowMenu.addItem(
            withTitle: "Close",
            action: #selector(NSWindow.performClose(_:)),
            keyEquivalent: "w"
        )
        let windowMenuItem = NSMenuItem()
        windowMenuItem.submenu = windowMenu
        let mainMenu = NSMenu()
        mainMenu.addItem(windowMenuItem)
        NSApp.mainMenu = mainMenu
    }
    private func configureInspectionWindow(_ window: NSWindow?) {
        let reduceTransparency = NSWorkspace.shared.accessibilityDisplayShouldReduceTransparency
        window?.isOpaque = reduceTransparency
        // A fully clear background on a non-opaque window makes every
        // zero-alpha pixel click-through — including the titlebar, where the
        // traffic lights live, so the close button stopped responding to real
        // clicks. Keep alpha barely above zero instead of zero.
        window?.backgroundColor = reduceTransparency
            ? .windowBackgroundColor
            : NSColor.black.withAlphaComponent(0.02)
        window?.titlebarAppearsTransparent = true
        window?.titlebarSeparatorStyle = .none
    }

    @objc private func accessibilityDisplayOptionsChanged() {
        configureInspectionWindow(detailWindow)
        configureInspectionWindow(statusWindow)
        configureInspectionWindow(settingsWindow)
    }
}

private extension NSRect {
    var area: CGFloat { max(0, width) * max(0, height) }
}

/// Plain `NSWindow`s ignore Esc (`cancelOperation` does nothing), so the
/// status surface would stay open while the Quick Prompt panel closes. Mirror
/// the panel behavior: Esc dismisses.
final class SentinelStatusWindow: NSWindow {
    override func cancelOperation(_ sender: Any?) {
        close()
    }
}
