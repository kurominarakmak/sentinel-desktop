# Platform Support

macOS is implemented and validated first while the architecture remains cross-platform. macOS uses `Command+Shift+Space`, a menu-bar surface, `.app` and DMG distribution, and requires signing/notarization outside the App Store. Validate Apple Silicon and Intel/universal builds, accessibility, menu-bar behavior, and PATH discovery from GUI processes.

Windows uses `Control+Shift+Space`, system tray behavior, process-tree cancellation, Git for Windows semantics, Windows path handling, executable discovery, NSIS or MSI packaging, and code signing.

Linux uses `Control+Shift+Space`, system tray/status notifier integration, process-group cancellation, login-shell path discovery, AppImage first then deb/rpm. Tray and global shortcut behavior vary by desktop environment and must be documented as limitations rather than assumed universal.

Global shortcuts, tray/menu-bar APIs, cancellation, and executable discovery require platform adapters. Packaging order is macOS DMG, Windows installer, then Linux AppImage/deb/rpm. See [architecture](architecture.md).
