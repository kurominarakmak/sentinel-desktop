# Phase 3 Claude Code Adapter Provenance

Sentinel uses only Claude Code's supported non-interactive `--print
--output-format stream-json` interface and its documented opaque `--resume`
session identifier. Sentinel persists and owns task/session state; it does not
scrape transcripts or discover arbitrary Claude sessions.

| Read-only OSS reference | What informed Sentinel | Sentinel-specific adaptation |
| --- | --- | --- |
| OpenCovibe `src-tauri/src/agent/claude_protocol.rs` and `session_actor.rs` | Treat stream-json init/session metadata and structured events as protocol input; process ownership is explicit. | Reimplemented a minimal independent JSONL reader that persists V3 envelopes; no source, actor, transcript handling, or UI was copied. |
| Jean/agetor `claude-followup-restart.test.ts` and `claude-tmux-death.test.ts` | Resume must use the persisted provider ID; lost process state must not imply completion. | Sentinel does not use tmux or reattach external sessions. It exposes only Sentinel-owned durable sessions for recovery. |
