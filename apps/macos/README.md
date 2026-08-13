# SentinelMac (Phase 1)

This is the parallel native macOS presentation shell. SwiftUI/AppKit owns only
menu-bar, window, keyboard, and rendering concerns. The `sentinel-native-bridge`
sidecar is the sole Swift-to-Rust boundary: line-delimited JSON over stdio.

The bridge reads durable V3 task snapshots, emits task-update snapshots, and
forwards the existing final-approval decision to `FinalApprovalSupervisor`.
It never exposes Git, provider, SQLite, or workflow-transition APIs to Swift.

For development, build the bridge with `cargo build -p sentinel-native-bridge`
and set `SENTINEL_NATIVE_BRIDGE` to its executable path before launching the
Swift package.
