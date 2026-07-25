import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockPhase2Services, createRunDto, createRunEvent, deferred, renderPhase2App } from "./test/phase2Harness";

const submit = async (task = "new task") => {
  await userEvent.type(screen.getByRole("textbox", { name: "Task" }), task);
  await userEvent.click(screen.getByRole("button", { name: "Run fake agent" }));
};

describe("Phase2App recent history synchronization", () => {
  it("discards an older initial snapshot after a newer post-submit snapshot", async () => {
    const mock = createMockPhase2Services(); const older = createRunDto({ id: "run-old", task_text: "old" }); const created = createRunDto({ id: "run-new", task_text: "new task" });
    const initial = deferred<(typeof older)[]>(); const postSubmit = deferred<(typeof created)[]>(); let calls = 0;
    mock.api.listRuns = vi.fn().mockImplementation(() => ++calls === 1 ? initial.promise : postSubmit.promise);
    mock.api.submitFakeRun = vi.fn().mockResolvedValue(created); mock.api.listRunEvents = vi.fn().mockResolvedValue([createRunEvent({ run_id: created.id })]); mock.api.getRun = vi.fn().mockResolvedValue(created);
    renderPhase2App(mock.services); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(1)); await submit();
    await screen.findByText(`running · ${created.id}`); expect(screen.getByRole("button", { name: /run-new/ })).toBeInTheDocument(); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(2));
    postSubmit.resolve([created, older]); await screen.findByRole("button", { name: /run-old/ }); initial.resolve([older]);
    await waitFor(() => expect(screen.getByRole("button", { name: /run-new/ })).toBeInTheDocument()); expect(screen.getByText(`running · ${created.id}`)).toBeInTheDocument(); expect(screen.queryByText("The run could not be submitted.")).not.toBeInTheDocument();
  });

  it("discards an older terminal-history snapshot after a submission refresh", async () => {
    const mock = createMockPhase2Services(); const terminal = createRunDto({ id: "run-terminal", task_text: "terminal", status: "completed", cancellable: false }); const created = createRunDto({ id: "run-new", task_text: "new task" });
    const terminalSnapshot = deferred<(typeof terminal)[]>(); const postSubmit = deferred<(typeof created)[]>(); let calls = 0;
    mock.api.listRuns = vi.fn().mockImplementation(() => { calls += 1; return calls === 1 ? Promise.resolve([terminal]) : calls === 2 ? terminalSnapshot.promise : postSubmit.promise; });
    mock.api.submitFakeRun = vi.fn().mockResolvedValue(created); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockImplementation((id) => Promise.resolve(id === created.id ? created : terminal));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-term/ }); await waitFor(() => expect(mock.services.listenToRunEvents).toHaveBeenCalledOnce());
    mock.emit(createRunEvent({ run_id: terminal.id, event_type: "run_completed", sequence_number: 2 })); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(2)); await submit();
    await screen.findByText(`running · ${created.id}`); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(3)); postSubmit.resolve([created, terminal]); await screen.findByRole("button", { name: /run-new/ }); terminalSnapshot.resolve([terminal]);
    await waitFor(() => expect(screen.getByRole("button", { name: /run-new/ })).toBeInTheDocument()); expect(screen.getByText(`running · ${created.id}`)).toBeInTheDocument();
  });

  it("keeps a created run and scoped history error when post-submit synchronization fails", async () => {
    const mock = createMockPhase2Services(); const old = createRunDto({ id: "run-old" }); const created = createRunDto({ id: "run-new", task_text: "new task" });
    const initial = deferred<(typeof old)[]>(); const retry = deferred<(typeof created)[]>(); let calls = 0;
    mock.api.listRuns = vi.fn().mockImplementation(() => { calls += 1; return calls === 1 ? initial.promise : calls === 2 ? Promise.reject(new Error("/private/history.sqlite: SQL failure")) : retry.promise; });
    mock.api.submitFakeRun = vi.fn().mockResolvedValue(created); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValue(created);
    renderPhase2App(mock.services); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(1)); await submit(); await screen.findByText(`running · ${created.id}`); await screen.findByText("Run history could not be refreshed. Retry.");
    expect(screen.getByRole("button", { name: /run-new/ })).toBeInTheDocument(); expect(screen.queryByText("The run could not be submitted.")).not.toBeInTheDocument(); expect(screen.queryByText(/private\/history|SQL failure/)).not.toBeInTheDocument(); expect(mock.api.listRunEvents).toHaveBeenCalledWith(created.id); expect(mock.api.getRun).toHaveBeenCalledWith(created.id);
    initial.resolve([old]); await waitFor(() => expect(screen.getByText("Run history could not be refreshed. Retry.")).toBeInTheDocument()); expect(screen.getByRole("button", { name: /run-new/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry history" })); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(3)); retry.resolve([created, old]); await waitFor(() => expect(screen.queryByText("Run history could not be refreshed. Retry.")).not.toBeInTheDocument());
  });

  it("reports only an actual submission failure and preserves the task", async () => {
    const mock = createMockPhase2Services(); mock.api.listRuns = vi.fn().mockResolvedValue([]); mock.api.submitFakeRun = vi.fn().mockRejectedValue(new Error("backend unavailable"));
    renderPhase2App(mock.services); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(1)); await submit("keep this task");
    await screen.findByText("The run could not be submitted."); expect(screen.getByRole("textbox", { name: "Task" })).toHaveValue("keep this task"); expect(screen.queryByText(/running · run-new/)).not.toBeInTheDocument(); expect(mock.api.listRuns).toHaveBeenCalledTimes(1); expect(screen.getByRole("button", { name: "Run fake agent" })).toBeEnabled();
  });
});
