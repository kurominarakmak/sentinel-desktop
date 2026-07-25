import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockPhase2Services, createRunDto, createRunEvent, deferred, renderPhase2App } from "./test/phase2Harness";
import type { RunStatus } from "./phase2";

const selectedRun = (id: string, task_text: string, status: RunStatus = "running") => createRunDto({ id, task_text, status });
const select = async (id: string) => { await userEvent.click(screen.getByRole("button", { name: new RegExp(id) })); };

describe("Phase2App explicit selection", () => {
  it("does not let a terminal refresh reselect A after B is selected", async () => {
    const mock = createMockPhase2Services(); const a = selectedRun("run-a", "task A"); const b = selectedRun("run-b", "task B", "completed");
    const terminalEvents = deferred<ReturnType<typeof createRunEvent>[]>(); const terminalRun = deferred<typeof a>(); let aEventCalls = 0; let aRunCalls = 0;
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b]);
    mock.api.listRunEvents = vi.fn().mockImplementation((id) => id === "run-a" && ++aEventCalls > 1 ? terminalEvents.promise : Promise.resolve([createRunEvent({ run_id: id, payload: { text: `${id} event` } })]));
    mock.api.getRun = vi.fn().mockImplementation((id) => id === "run-a" && ++aRunCalls > 1 ? terminalRun.promise : Promise.resolve(id === "run-b" ? b : a));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select("run-a");
    mock.emit(createRunEvent({ run_id: "run-a", event_type: "run_completed", sequence_number: 2 }));
    await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledTimes(2)); await select("run-b");
    await screen.findByText(`completed · ${b.id}`); terminalEvents.resolve([createRunEvent({ run_id: "run-a", event_type: "run_completed", sequence_number: 2 })]);
    await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(3)); terminalRun.resolve({ ...a, status: "completed" });
    await waitFor(() => expect(screen.getByText(`completed · ${b.id}`)).toBeInTheDocument());
    expect(screen.getByText(/run-b event/)).toBeInTheDocument(); expect(screen.queryByText(/run-a event/)).not.toBeInTheDocument();
  });

  it("does not select B when its background terminal refresh completes", async () => {
    const mock = createMockPhase2Services(); const a = selectedRun("run-a", "task A"); const b = selectedRun("run-b", "task B", "completed");
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockImplementation((id) => Promise.resolve(id === "run-b" ? b : a));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select("run-a"); await screen.findByText(`running · ${a.id}`);
    mock.emit(createRunEvent({ run_id: "run-b", event_type: "run_completed" }));
    await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledWith("run-b"));
    expect(screen.getByText(`running · ${a.id}`)).toBeInTheDocument(); expect(screen.queryByText(`completed · ${b.id}`)).not.toBeInTheDocument();
  });

  it("does not let a cancellation refresh restore A after B is selected", async () => {
    const mock = createMockPhase2Services(); const a = selectedRun("run-a", "task A"); const b = selectedRun("run-b", "task B", "completed");
    const cancellationEvents = deferred<ReturnType<typeof createRunEvent>[]>(); const cancellationRun = deferred<typeof a>(); let aEventCalls = 0; let aRunCalls = 0;
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b]); mock.api.listRunEvents = vi.fn().mockImplementation((id) => id === "run-a" && ++aEventCalls > 1 ? cancellationEvents.promise : Promise.resolve([])); mock.api.cancelRun = vi.fn().mockResolvedValue("cancellation_requested");
    mock.api.getRun = vi.fn().mockImplementation((id) => id === "run-a" && ++aRunCalls > 1 ? cancellationRun.promise : Promise.resolve(id === "run-b" ? b : a));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select("run-a"); await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledTimes(2)); await select("run-b"); await screen.findByText(`completed · ${b.id}`); cancellationEvents.resolve([]);
    await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(3)); cancellationRun.resolve({ ...a, status: "cancelled" });
    await waitFor(() => expect(screen.getByText(`completed · ${b.id}`)).toBeInTheDocument());
  });
});
