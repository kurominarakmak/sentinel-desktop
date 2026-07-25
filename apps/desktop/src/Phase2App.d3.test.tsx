import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockPhase2Services, createRunDto, createRunEvent, createRuntimeEnvironment, deferred, renderPhase2App } from "./test/phase2Harness";
import type { CancellationResult, RunDto } from "./phase2";

const typeTask = async (task: string) => userEvent.type(screen.getByRole("textbox", { name: "Task" }), task);
const select = async (id: string) => userEvent.click(screen.getByRole("button", { name: new RegExp(id) }));
const run = (id: string, overrides: Partial<RunDto> = {}) => createRunDto({ id, task_text: id, ...overrides });

describe("Phase2App D3 submission, cancellation, and availability", () => {
  it("submits only typed task/scenario fields, upserts/selects immediately, clears the task, and synchronizes independently", async () => {
    const mock = createMockPhase2Services(); const created = run("run-created", { task_text: "inspect π", status: "queued", cancellable: false });
    mock.api.listRuns = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([created]); mock.api.submitFakeRun = vi.fn().mockResolvedValue(created); mock.api.listRunEvents = vi.fn().mockResolvedValue([createRunEvent({ run_id: created.id, payload: { text: "persisted" } })]); mock.api.getRun = vi.fn().mockResolvedValue({ ...created, status: "running", cancellable: true });
    renderPhase2App(mock.services); await typeTask("inspect π"); await userEvent.selectOptions(screen.getByRole("combobox"), "delayed"); await userEvent.click(screen.getByRole("button", { name: "Run fake agent" }));
    await screen.findByText(`running · ${created.id}`); expect(mock.api.submitFakeRun).toHaveBeenCalledWith("inspect π", "delayed"); expect(screen.getByRole("textbox", { name: "Task" })).toHaveValue(""); expect(screen.getByRole("button", { name: /inspect π/ })).toBeInTheDocument(); expect(screen.getByText(/persisted/)).toBeInTheDocument(); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(2)); expect(screen.queryByText("The run could not be submitted.")).not.toBeInTheDocument(); expect(screen.getByRole("button", { name: "Run fake agent" })).toBeDisabled();
  });

  it("prevents duplicate submission while the first typed command is pending", async () => {
    const mock = createMockPhase2Services(); const submitted = deferred<RunDto>(); const created = run("run-created");
    mock.api.listRuns = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([created]); mock.api.submitFakeRun = vi.fn().mockReturnValue(submitted.promise); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValue(created);
    renderPhase2App(mock.services); await typeTask("once"); const button = screen.getByRole("button", { name: "Run fake agent" }); await userEvent.click(button); await userEvent.keyboard("{Enter}"); expect(mock.api.submitFakeRun).toHaveBeenCalledOnce(); expect(screen.getByRole("button", { name: "Submitting…" })).toBeDisabled();
    submitted.resolve(created); await screen.findByText(`running · ${created.id}`); expect(screen.getAllByRole("button", { name: /run-crea/ })).toHaveLength(1); expect(screen.getByRole("button", { name: "Run fake agent" })).toBeDisabled(); await typeTask("next"); expect(screen.getByRole("button", { name: "Run fake agent" })).toBeEnabled();
  });

  it("keeps genuine submission failure safe and allows a later success", async () => {
    const mock = createMockPhase2Services(); const created = run("run-created");
    mock.api.listRuns = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([created]); mock.api.submitFakeRun = vi.fn().mockRejectedValueOnce(new Error("/private/db: SELECT stderr stack token=hunter2")).mockResolvedValueOnce(created); mock.api.listRunEvents = vi.fn().mockRejectedValueOnce(new Error("events failed")); mock.api.getRun = vi.fn().mockRejectedValueOnce(new Error("metadata failed"));
    renderPhase2App(mock.services); await typeTask("recoverable"); await userEvent.click(screen.getByRole("button", { name: "Run fake agent" })); await screen.findByText("The run could not be submitted."); expect(screen.getByRole("textbox", { name: "Task" })).toHaveValue("recoverable"); expect(screen.queryByRole("button", { name: /run-created/ })).not.toBeInTheDocument(); expect(screen.queryByText(/private\/db|SELECT|hunter2/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Run fake agent" })); await screen.findByText(`running · ${created.id}`); expect(screen.getByRole("button", { name: /run-created/ })).toBeInTheDocument(); expect(screen.getByText("Event history could not be loaded. Retry.")).toBeInTheDocument(); expect(screen.getByText("Run details could not be refreshed. Retry.")).toBeInTheDocument(); expect(screen.queryByText("The run could not be submitted.")).not.toBeInTheDocument();
  });

  it("enforces UI validation before invoking submit", async () => {
    const mock = createMockPhase2Services(); renderPhase2App(mock.services);
    expect(screen.getByRole("button", { name: "Run fake agent" })).toBeDisabled(); expect(mock.api.submitFakeRun).not.toHaveBeenCalled();
    fireEvent.change(screen.getByRole("textbox", { name: "Task" }), { target: { value: "😀".repeat(2_001) } }); await userEvent.click(screen.getByRole("button", { name: "Run fake agent" })); expect(screen.getByText("Task is too long.")).toBeInTheDocument(); expect(mock.api.submitFakeRun).not.toHaveBeenCalled();
    const scenario = screen.getByRole("combobox"); const unsupported = document.createElement("option"); unsupported.value = "unsupported"; unsupported.text = "unsupported"; scenario.append(unsupported); fireEvent.change(scenario, { target: { value: "unsupported" } }); fireEvent.change(screen.getByRole("textbox", { name: "Task" }), { target: { value: "valid unicode ก" } }); await userEvent.click(screen.getByRole("button", { name: "Run fake agent" })); expect(screen.getByText("Select a supported scenario.")).toBeInTheDocument(); expect(mock.api.submitFakeRun).not.toHaveBeenCalled();
  });

  it("accepts a task at the exact UTF-8 byte boundary", async () => {
    const mock = createMockPhase2Services(); const created = run("run-boundary"); mock.api.listRuns = vi.fn().mockResolvedValueOnce([]).mockResolvedValueOnce([created]); mock.api.submitFakeRun = vi.fn().mockResolvedValue(created); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValue(created);
    renderPhase2App(mock.services); fireEvent.change(screen.getByRole("textbox", { name: "Task" }), { target: { value: "a".repeat(8_000) } }); await userEvent.click(screen.getByRole("button", { name: "Run fake agent" })); await screen.findByText(`running · ${created.id}`); expect(mock.api.submitFakeRun).toHaveBeenCalledWith("a".repeat(8_000), "success");
  });

  it.each<[CancellationResult, string, RunDto]>([
    ["cancellation_requested", "Cancellation was requested.", run("run-a", { status: "cancelling", cancellable: false })],
    ["already_cancelling", "Cancellation is already in progress.", run("run-a", { status: "cancelling", cancellable: false })],
    ["already_terminal", "This run has already finished.", run("run-a", { status: "completed", cancellable: false })],
    ["run_not_active", "This run is no longer active in this desktop session.", run("run-a", { status: "running", cancellable: false })],
    ["termination_failed", "Cancellation could not be completed.", run("run-a", { status: "running", cancellable: false })],
  ])("handles %s with authoritative refresh and no invented terminal state", async (result, message, refreshed) => {
    const mock = createMockPhase2Services(); const active = run("run-a", { cancellable: true });
    mock.api.listRuns = vi.fn().mockResolvedValue([active]); mock.api.listRunEvents = vi.fn().mockResolvedValue([createRunEvent({ payload: { text: "kept" } })]); mock.api.getRun = vi.fn().mockResolvedValueOnce(active).mockResolvedValueOnce(refreshed); mock.api.cancelRun = vi.fn().mockResolvedValue(result);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(active.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); await screen.findByText(message);
    await screen.findByText(`${refreshed.status} · run-a`); expect(screen.getByText(/kept/)).toBeInTheDocument(); expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument(); if (result === "run_not_active" || result === "termination_failed") expect(screen.queryByText("cancelled · run-a")).not.toBeInTheDocument();
  });

  it("guards pending cancellation and cannot restore A after A→B→C selection changes", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a", { cancellable: true }); const b = run("run-b", { cancellable: false }); const c = run("run-c", { cancellable: false }); const pending = deferred<CancellationResult>(); const refresh = deferred<RunDto>(); let aGetCalls = 0;
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b, c]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockImplementation((id) => id === a.id && ++aGetCalls > 1 ? refresh.promise : Promise.resolve(id === a.id ? a : id === b.id ? b : c)); mock.api.cancelRun = vi.fn().mockReturnValue(pending.promise);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); await userEvent.click(screen.getByRole("button", { name: "Cancelling…" })); expect(mock.api.cancelRun).toHaveBeenCalledOnce(); await select(b.id); await screen.findByText("running · run-b"); await select(c.id); await screen.findByText("running · run-c"); pending.resolve("cancellation_requested"); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledTimes(4)); refresh.resolve(run(a.id, { status: "cancelling", cancellable: false })); await waitFor(() => expect(screen.getByText("running · run-c")).toBeInTheDocument());
  });

  it("handles cancellation command rejection safely without changing selection", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a", { cancellable: true });
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockResolvedValue([createRunEvent({ payload: { text: "kept" } })]); mock.api.getRun = vi.fn().mockResolvedValue(a); mock.api.cancelRun = vi.fn().mockRejectedValue(new Error("/private/process stderr token=hunter2"));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); await screen.findByText("Cancellation could not be requested."); expect(screen.getByText("running · run-a")).toBeInTheDocument(); expect(screen.getByText(/kept/)).toBeInTheDocument(); expect(screen.queryByText(/private\/process|hunter2/)).not.toBeInTheDocument(); expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
  });

  it("keeps history usable when availability fails and retries only the environment command", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a", { cancellable: false });
    mock.api.environment = vi.fn().mockRejectedValueOnce(new Error("/private/config: SQL stderr stack")).mockResolvedValueOnce(createRuntimeEnvironment({ fake_agent_available: true })); mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValue(a);
    renderPhase2App(mock.services); await screen.findByText("Backend is unavailable."); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await screen.findByText("running · run-a"); expect(screen.getByRole("button", { name: "Run fake agent" })).toBeDisabled(); expect(screen.queryByText(/private\/config|SQL/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry availability" })); await waitFor(() => expect(mock.api.environment).toHaveBeenCalledTimes(2)); await waitFor(() => expect(screen.queryByText("Backend is unavailable.")).not.toBeInTheDocument()); expect(screen.getByText("Fake Agent — available")).toBeInTheDocument(); expect(screen.getByText("running · run-a")).toBeInTheDocument(); expect(mock.api.submitFakeRun).not.toHaveBeenCalled();
  });

  it("ignores pending availability completion after unmount and cleans listeners", async () => {
    const mock = createMockPhase2Services(); const availability = deferred<ReturnType<typeof createRuntimeEnvironment>>(); mock.api.environment = vi.fn().mockReturnValue(availability.promise);
    const view = renderPhase2App(mock.services); await waitFor(() => expect(mock.api.environment).toHaveBeenCalledOnce()); view.unmount(); availability.resolve(createRuntimeEnvironment()); await waitFor(() => expect(mock.activeRunListeners()).toBe(0)); expect(mock.unlisten).toHaveBeenCalledOnce();
  });
});
