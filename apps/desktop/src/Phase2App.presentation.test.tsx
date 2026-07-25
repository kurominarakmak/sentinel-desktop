import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockPhase2Services, createRunDto, createRunEvent, renderPhase2App } from "./test/phase2Harness";

describe("Phase2App safe event presentation", () => {
  it("renders normal message text and event metadata without raw JSON", async () => {
    const mock = createMockPhase2Services(); const run = createRunDto();
    mock.api.listRuns = vi.fn().mockResolvedValue([run]); mock.api.getRun = vi.fn().mockResolvedValue(run); mock.api.listRunEvents = vi.fn().mockResolvedValue([createRunEvent({ sequence_number: 7, payload: { type: "message", text: "Reading repository" } })]);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await userEvent.click(screen.getByRole("button", { name: /run-a/ }));
    expect(await screen.findByText(/#7 message: Reading repository/)).toBeInTheDocument(); expect(screen.queryByText(/\{"type"/)).not.toBeInTheDocument();
  });

  it("redacts sensitive payloads and safely summarizes malformed, unknown, and lifecycle events", async () => {
    const mock = createMockPhase2Services(); const run = createRunDto(); const rawPath = "/Users/example/private/project/config.json /home/example/.ssh/id_rsa C:\\Users\\example\\secret.txt";
    const rawDiagnostic = "stderr: SELECT * FROM credentials\nthread 'main' panicked"; const rawSecret = "API_KEY=super-secret-value Authorization: Bearer abc123 password=hunter2";
    mock.api.listRuns = vi.fn().mockResolvedValue([run]); mock.api.getRun = vi.fn().mockResolvedValue(run); mock.api.listRunEvents = vi.fn().mockResolvedValue([
      createRunEvent({ sequence_number: 1, payload: { type: "message", text: rawPath } }), createRunEvent({ sequence_number: 2, payload: { type: "message", text: rawDiagnostic } }), createRunEvent({ sequence_number: 3, payload: { type: "message", text: rawSecret } }), createRunEvent({ sequence_number: 4, event_type: "unknown", payload: { nested: [rawSecret] } }), createRunEvent({ sequence_number: 5, event_type: "run_completed", payload: { text: rawDiagnostic } }), createRunEvent({ sequence_number: 6, event_type: "phase_changed", payload: { phase: ["wrong"], secret: rawSecret } }),
    ]);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await userEvent.click(screen.getByRole("button", { name: /run-a/ }));
    await screen.findByText(/#1 message: Run message unavailable/); expect(screen.getByText(/#2 message: Run message unavailable/)).toBeInTheDocument(); expect(screen.getByText(/#3 message: \[REDACTED\]/)).toBeInTheDocument(); expect(screen.getByText(/#4 unknown: Additional run information is unavailable/)).toBeInTheDocument(); expect(screen.getByText(/#5 run_completed: Completed/)).toBeInTheDocument(); expect(screen.getByText(/#6 phase_changed: Phase changed/)).toBeInTheDocument();
    expect(screen.queryByText(new RegExp(rawPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")))).not.toBeInTheDocument(); expect(screen.queryByText(/SELECT \* FROM credentials|thread 'main' panicked|super-secret-value|abc123|hunter2|\/home\/|C:\\Users/)).not.toBeInTheDocument();
  });

  it("suppresses newly recognized path and database diagnostics without changing message-event ordering", async () => {
    const mock = createMockPhase2Services(); const run = createRunDto();
    const sensitive = ["c:\\users\\alice\\secret.txt", "C:/Users/alice/secret.txt", "/usr/local/bin/internal-tool", "/private/var/db/app.sqlite", "PRAGMA database_list", "VACUUM", "SQLite error: database is locked"];
    mock.api.listRuns = vi.fn().mockResolvedValue([run]); mock.api.getRun = vi.fn().mockResolvedValue(run); mock.api.listRunEvents = vi.fn().mockResolvedValue([
      ...sensitive.map((text, index) => createRunEvent({ sequence_number: index + 1, payload: { type: "message", text } })),
      createRunEvent({ sequence_number: 8, payload: { type: "message", text: "Completed 4 of 10 steps" } }),
    ]);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await userEvent.click(screen.getByRole("button", { name: /run-a/ }));
    for (let index = 0; index < sensitive.length; index += 1) expect(await screen.findByText(new RegExp(`#${index + 1} message: Run message unavailable`))).toBeInTheDocument();
    expect(screen.getByText(/#8 message: Completed 4 of 10 steps/)).toBeInTheDocument(); expect(mock.api.getRun).toHaveBeenCalledTimes(1);
    for (const raw of sensitive) expect(screen.queryByText(raw, { exact: false })).not.toBeInTheDocument();
  });

  it("suppresses bare roots and separator-obfuscated paths through the real message presentation path", async () => {
    const mock = createMockPhase2Services(); const run = createRunDto();
    const sensitive = ["cwd: /usr", "path: /private", "C:\\\nUsers\\alice\\secret.txt", "C:/\u200BUsers/alice/file", "/usr/\nlocal/bin/tool", "\\\nserver\\share\\secret"];
    mock.api.listRuns = vi.fn().mockResolvedValue([run]); mock.api.getRun = vi.fn().mockResolvedValue(run); mock.api.listRunEvents = vi.fn().mockResolvedValue([
      ...sensitive.map((text, index) => createRunEvent({ sequence_number: index + 1, payload: { type: "message", text } })),
      createRunEvent({ sequence_number: 7, payload: { type: "message", text: "Waiting for confirmation" } }),
    ]);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await userEvent.click(screen.getByRole("button", { name: /run-a/ }));
    for (let index = 0; index < sensitive.length; index += 1) expect(await screen.findByText(new RegExp(`#${index + 1} message: Run message unavailable`))).toBeInTheDocument();
    expect(screen.getByText(/#7 message: Waiting for confirmation/)).toBeInTheDocument(); expect(mock.api.getRun).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(/Users\\alice|private|local\/bin|server\\share/)).not.toBeInTheDocument();
  });
});
