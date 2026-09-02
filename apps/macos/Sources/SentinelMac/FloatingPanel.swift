import AppKit
import SwiftUI

final class SentinelFloatingPanel: NSPanel {
    var onHide: (() -> Void)?
    private(set) var hasBeenPresented = false

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    convenience init<Content: View>(
        title: String,
        height: CGFloat = 240,
        @ViewBuilder content: () -> Content
    ) {
        self.init(
            contentRect: NSRect(x: 0, y: 0, width: SentinelTokens.panelWidth, height: height),
            styleMask: [.nonactivatingPanel, .fullSizeContentView, .titled],
            backing: .buffered,
            defer: false
        )
        self.title = title
        isFloatingPanel = true
        hidesOnDeactivate = false
        isOpaque = false
        backgroundColor = .clear
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        contentView = NSHostingView(rootView: content())
    }

    override func cancelOperation(_ sender: Any?) {
        hide()
    }

    override func close() {
        hide()
    }

    func hide() {
        guard isVisible else { return }
        orderOut(nil)
        onHide?()
    }

    func markPresented() { hasBeenPresented = true }

    func resizeForQuickPrompt(height: CGFloat) {
        let newHeight = min(max(height, 76), 480)
        guard abs(frame.height - newHeight) > 1 else { return }
        let next = FloatingPanelPlacement.resizedFrameKeepingTop(frame, height: newHeight)
        let changes = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? false : true
        if changes {
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0.16
                animator().setFrame(next, display: true)
            }
        } else {
            setFrame(next, display: true)
        }
    }
}
