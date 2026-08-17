# Agent Sentinel for macOS

This is the parallel native SwiftUI/AppKit presentation. Swift owns menu-bar,
window, focus, keyboard, and rendering concerns only. The bundled
`sentinel-native-bridge` sidecar is the sole Swift-to-Rust boundary, using
typed newline-delimited JSON over private stdio.

The bridge hosts `SentinelSupervisor`. A task-start intent therefore follows:

`Swift intent → Rust supervisor → owned worktree → provider → validation → read-only review → bounded repair → human approval`

Rust reloads every snapshot from durable V3 state and derives available
actions. Mutation request IDs and responses are persisted for replay safety.
Swift never opens SQLite, invokes Git/providers/validation, advances workflow
state, or grants approval.

Build the runnable menu-bar app with:

```sh
apps/macos/build-app.sh
open "target/native-app/Agent Sentinel.app"
```

`swift run --package-path apps/macos` is supported for development when
`SENTINEL_NATIVE_BRIDGE` points to a built bridge executable. The Tauri app is
retained as the previous frontend/reference until native parity is accepted.
