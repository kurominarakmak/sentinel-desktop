import { describe, expect, it, vi } from "vitest";
import { containsDatabaseDiagnostic, containsSensitiveAbsolutePath, createApi, formatEventSummary, isRunLifecycleEvent, isScenario, isTerminalRunEvent, MAX_TASK_BYTES, mergeRunEvents, patchRunInHistory, reconcile, scenarios, sanitizeEventText, statusText, upsertRunInHistory, validateTask, type RunDto, type RunEvent } from "./phase2";

const event = (sequence_number: number, run_id = "run"): RunEvent => ({ schema_version: 1, run_id, sequence_number, event_type: "message", timestamp_ms: 1, payload: { type: "message", text: `event ${sequence_number}` } });
const run = (id: string, task_text: string): RunDto => ({ id, task_text, agent_kind: "fake", status: "running", schema_version: 1, created_at_ms: 1, started_at_ms: null, finished_at_ms: null, exit_code: null, error_category: null, error_message: null, cancellable: true });
describe("Phase 2 frontend helpers", () => {
  it("uses typed command names and a narrow submit payload", async () => {
    const call = vi.fn().mockResolvedValue({ id: "run" }); const api = createApi(call);
    await api.submitFakeRun("task", "success");
    expect(call).toHaveBeenCalledWith("submit_fake_run", { request: { task_text: "task", scenario: "success" } });
  });
  it("keeps the Rust scenario allowlist and validates UTF-8 task bytes", () => {
    expect(scenarios).toContain("cancellation-child"); expect(isScenario("nope")).toBe(false);
    expect(validateTask(" ")).toBeTruthy(); expect(validateTask("a".repeat(MAX_TASK_BYTES))).toBeNull();
    expect(validateTask("😀".repeat(2_001))).toBeTruthy();
    expect(validateTask("ก".repeat(2_666))).toBeNull(); expect(validateTask("日".repeat(2_667))).toBeTruthy();
  });
  it("normalizes errors and keeps event reconciliation ordered", async () => {
    const api = createApi(vi.fn().mockRejectedValue({ code: "sql", message: 3 }));
    await expect(api.listRuns()).rejects.toEqual({ code: "sql", message: "The request could not be completed." });
    expect(reconcile(reconcile([event(2)], event(1)), event(2)).map((item) => item.sequence_number)).toEqual([1, 2]);
  });
  it("mirrors the explicit production RunDto fixture", () => {
    const fixture: RunDto = { id: "018f1234-5678-7abc-8def-123456789abc", task_text: "inspect fixture", agent_kind: "fake", status: "running", schema_version: 1, created_at_ms: 10, started_at_ms: 20, finished_at_ms: null, exit_code: null, error_category: null, error_message: null, cancellable: true };
    expect(fixture).toEqual({ id: "018f1234-5678-7abc-8def-123456789abc", task_text: "inspect fixture", agent_kind: "fake", status: "running", schema_version: 1, created_at_ms: 10, started_at_ms: 20, finished_at_ms: null, exit_code: null, error_category: null, error_message: null, cancellable: true });
    expect("agent" in fixture).toBe(false); expect("error" in fixture).toBe(false);
  });
  it("formats only allowlisted event payload fields", () => {
    expect(formatEventSummary(event(1))).toBe("event 1"); expect(formatEventSummary({ ...event(2), event_type: "phase_changed", payload: { type: "phase_changed", phase: "Planning", secret: "API_KEY=hidden" } })).toBe("Phase: Planning");
    expect(formatEventSummary({ ...event(3), event_type: "command_completed", payload: { type: "command_completed", exit_code: 0, command: "/private/tool" } })).toBe("Command completed (exit code 0)");
    expect(formatEventSummary({ ...event(4), event_type: "future_event", payload: { deeply: ["nested", { secret: "hidden" }] } })).toBe("Additional run information is unavailable");
    expect(formatEventSummary({ ...event(5), event_type: "phase_changed", payload: { phase: ["wrong"] } })).toBe("Phase changed");
    expect(statusText("running")).toBe("running"); expect(statusText("future")).toBe("Unknown status: future");
    expect(reconcile([event(1, "one")], event(2, "two")).map((item) => item.run_id)).toEqual(["one"]);
  });
  it("rejects absolute filesystem paths before rendering message text", () => {
    for (const path of ["c:\\users\\alice\\secret.txt", "C:/Users/alice/secret.txt", "d:\\Internal\\bin\\agent.exe", "D:/Internal/bin/agent.exe", "C:\\Users/alice/private/file", "C:\\", "c:/", "path: D:\\", "\\\\server\\share\\private\\file.txt", "//server/share/private/file", "/usr/local/bin/internal-tool", "/private/var/db/app.sqlite", "/etc/agent/config.toml", "/var/log/agent/error.log", "/opt/internal/tool", "/tmp/debug-output", "/Applications/Internal.app/Contents/MacOS/tool", "path: '/mnt/internal/data'", "executable: /srv/agent/bin/run"]) {
      expect(containsSensitiveAbsolutePath(path)).toBe(true); expect(sanitizeEventText(`message ${path}`)).toBe("Run message unavailable");
    }
    for (const path of ["cwd: /usr", "cwd: /private", "path: /etc", "path=\"/var\"", "executable: /bin", "directory /tmp", "/Applications", "/Library", "/Volumes", "/root", "/mnt", "/srv", "/sbin", "/Users", "/home", "cwd: /usr.", "path='/private'", "using (/etc)"]) expect(sanitizeEventText(path)).toBe("Run message unavailable");
    for (const path of ["C:\\\nUsers\\alice\\secret.txt", "C:/\rUsers/alice/secret.txt", "c:\\tusers\\alice\\secret.txt", "C:\\\u0000Users\\alice\\secret.txt", "C:\\\u200BUsers\\alice\\secret.txt", "C:\\\u2060Users\\alice\\secret.txt", "/usr/\nlocal/bin/tool", "/\u200Bprivate/var/db", "\\\nserver\\share\\secret", "//\u2060server/share/secret"]) expect(sanitizeEventText(path)).toBe("Run message unavailable");
    for (const text of ["Processing repository files", "Completed 4 of 10 steps", "Waiting for confirmation", "Updated the selected source files", "Updated /username documentation", "Waiting on /variable input", "Selected /optional mode"]) expect(sanitizeEventText(text)).toBe(text);
  });
  it("rejects SQL statements and database diagnostics before rendering message text", () => {
    for (const diagnostic of ["PRAGMA database_list", "pragma journal_mode", "VACUUM", "ATTACH DATABASE '/private/db' AS internal", "REINDEX internal_index", "BEGIN TRANSACTION", "ROLLBACK", "EXPLAIN QUERY PLAN SELECT 1", "SQLite error: database is locked", "sqlite3 error: unable to open database file", "database disk image is malformed", "no such table: credentials", "UNIQUE constraint failed: users.email", "disk I/O error", "near \"FROM\": syntax error"]) {
      expect(containsDatabaseDiagnostic(diagnostic)).toBe(true); expect(sanitizeEventText(diagnostic)).toBe("Run message unavailable");
    }
  });
  it("redacts diagnostics, credentials, controls, and oversized message text", () => {
    for (const diagnostic of ["SELECT * FROM credentials", "stderr: failed to execute /usr/local/bin/tool", "thread 'main' panicked", "stack trace at app (main.ts:1:2)"]) expect(sanitizeEventText(diagnostic)).toBe("Run message unavailable");
    const secrets = sanitizeEventText("API_KEY=super-secret-value Authorization: Bearer abc123 password=hunter2"); expect(secrets).not.toMatch(/super-secret-value|abc123|hunter2/); expect(secrets).toContain("[REDACTED]");
    const bounded = sanitizeEventText(`hello\u0000\u001b${"x".repeat(300)}`); expect(bounded.length).toBeLessThanOrEqual(241); expect(bounded).not.toMatch(/[\u0000\u001b]/); expect(bounded).not.toContain("x".repeat(241));
  });
  it("uses lifecycle labels while ignoring their payloads", () => {
    expect(formatEventSummary({ ...event(6), event_type: "run_completed", payload: { text: "stderr: SELECT * FROM credentials" } })).toBe("Completed");
  });
  it("merges persisted snapshots without discarding newer live events", () => {
    const live = [event(2)]; const snapshot = [event(1), { ...event(2), payload: { text: "persisted" } }];
    expect(mergeRunEvents("run", live, snapshot).map((item) => item.sequence_number)).toEqual([1, 2]);
    expect(mergeRunEvents("run", live, snapshot)[1].payload).toEqual({ text: "persisted" });
    expect(live).toHaveLength(1); expect(snapshot).toHaveLength(2);
  });
  it("patches a history row immutably without reordering it", () => {
    const runs = [
      run("a", "A"),
      run("b", "B old"),
      run("c", "C"),
    ];
    const updated = { ...runs[1], task_text: "B new" };
    const result = patchRunInHistory(runs, updated);
    expect(result.map((run) => run.id)).toEqual(["a", "b", "c"]);
    expect(result[1]).toBe(updated); expect(result[0]).toBe(runs[0]); expect(result).not.toBe(runs);
  });
  it("upserts submitted history without duplication or mutation", () => {
    const a = run("a", "A"); const b = run("b", "B"); const c = run("c", "C");
    const history = [a, b]; const existing = upsertRunInHistory(history, { ...a, task_text: "A updated" });
    expect(existing.map((run) => run.id)).toEqual(["a", "b"]); expect(existing[0].task_text).toBe("A updated"); expect(existing).not.toBe(history);
    const original = [a, b]; const inserted = upsertRunInHistory(original, c);
    expect(inserted.map((run) => run.id)).toEqual(["c", "a", "b"]); expect(original).toEqual([a, b]); expect(inserted[1]).toBe(a); expect(inserted[2]).toBe(b);
  });
  it("classifies the Rust lifecycle contract and only its terminal subset", () => {
    for (const eventType of ["run_preparing", "run_running", "run_cancelling", "run_cancelled", "run_completed", "run_failed"]) expect(isRunLifecycleEvent(eventType)).toBe(true);
    for (const eventType of ["run_cancelled", "run_completed", "run_failed"]) expect(isTerminalRunEvent(eventType)).toBe(true);
    for (const eventType of ["message", "session_started", "phase_changed", "agent_failed", "parser_error", "unknown_event"]) {
      expect(isRunLifecycleEvent(eventType)).toBe(false); expect(isTerminalRunEvent(eventType)).toBe(false);
    }
  });
});
