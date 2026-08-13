import AppKit
import SwiftUI

final class SentinelFloatingPanel: NSPanel {
    var onHide: (() -> Void)?

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    convenience init<Content: View>(title: String, @ViewBuilder content: () -> Content) {
        self.init(
            contentRect: NSRect(x: 0, y: 0, width: SentinelTokens.panelWidth, height: 240),
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
        orderOut(nil)
        onHide?()
    }
}
