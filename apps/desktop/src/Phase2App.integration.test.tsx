import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockPhase2Services, createRunDto, createRunEvent, deferred, renderPhase2App } from "./test/phase2Harness";
import type { RunEvent, RunStatus } from "./phase2";

const run = (id: string, status: RunStatus = "running") => createRunDto({ id, task_text: id, status });
const event = (run_id: string, sequence_number: number, text: string, event_type = "message"): RunEvent => createRunEvent({ run_id, sequence_number, event_type, payload: event_type === "message" ? { text } : {} });
const select = async (id: string) => userEvent.click(screen.getByRole("button", { name: new RegExp(id) }));
const expectSequence = async (sequence: number, text: string) => expect(await screen.findByText(new RegExp(`#${sequence} message: ${text}`))).toBeInTheDocument();

describe("Phase2App persisted/live integration", () => {
  it("keeps persisted-before-live events ordered", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a");
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockResolvedValue([event(a.id, 1, "one"), event(a.id, 2, "two")]); mock.api.getRun = vi.fn().mockResolvedValue(a);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await expectSequence(2, "two");
    mock.emit(event(a.id, 3, "three")); await expectSequence(3, "three");
    expect(screen.getAllByRole("listitem").map((item) => item.textContent).filter((text) => text?.startsWith("#"))).toEqual(["#1 message: one", "#2 message: two", "#3 message: three"]);
  });

  it("merges live-before-persisted events, retains live tails, and gives persisted conflicts precedence", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const replay = deferred<RunEvent[]>();
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockReturnValue(replay.promise); mock.api.getRun = vi.fn().mockResolvedValue(a);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledWith(a.id));
    mock.emit(event(a.id, 2, "live two")); mock.emit(event(a.id, 3, "live three")); replay.resolve([event(a.id, 1, "persisted one"), event(a.id, 2, "persisted two")]);
    await expectSequence(1, "persisted one"); await expectSequence(2, "persisted two"); await expectSequence(3, "live three");
    expect(screen.queryByText(/live two/)).not.toBeInTheDocument(); expect(screen.getAllByText(/#2 message/)).toHaveLength(1);
  });

  it("sorts out-of-order live delivery without duplicating rows", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a");
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValue(a);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id);
    mock.emit(event(a.id, 4, "four")); mock.emit(event(a.id, 2, "two")); mock.emit(event(a.id, 3, "three")); mock.emit(event(a.id, 3, "duplicate"));
    await expectSequence(2, "two"); await expectSequence(3, "three"); await expectSequence(4, "four"); expect(screen.queryByText(/duplicate/)).not.toBeInTheDocument();
    expect(screen.getAllByRole("listitem").map((item) => item.textContent).filter((text) => text?.startsWith("#"))).toEqual(["#2 message: two", "#3 message: three", "#4 message: four"]);
  });

  it("does not remove a live terminal event when terminal replay is an older snapshot", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const completed = run("run-a", "completed"); let eventCalls = 0;
    mock.api.listRuns = vi.fn().mockResolvedValueOnce([a]).mockResolvedValueOnce([completed]); mock.api.listRunEvents = vi.fn().mockImplementation(() => ++eventCalls === 1 ? Promise.resolve([event(a.id, 1, "one")]) : Promise.resolve([event(a.id, 1, "one"), event(a.id, 2, "two"), event(a.id, 3, "three")])); mock.api.getRun = vi.fn().mockResolvedValueOnce(a).mockResolvedValueOnce(completed);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await expectSequence(1, "one");
    mock.emit(event(a.id, 4, "", "run_completed")); await screen.findByText("completed · run-a");
    expect(screen.getByText(/#4 run_completed: Completed/)).toBeInTheDocument(); expect(screen.getAllByText(/#4 run_completed/)).toHaveLength(1); expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument(); expect(screen.getByRole("button", { name: /completed.*run-a/ })).toBeInTheDocument();
  });

  it("isolates ordinary unselected live events from the selected panel", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const b = run("run-b");
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b]); mock.api.listRunEvents = vi.fn().mockResolvedValue([event(a.id, 1, "A only")]); mock.api.getRun = vi.fn().mockResolvedValue(a);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await expectSequence(1, "A only");
    mock.emit(event(b.id, 2, "B hidden")); await Promise.resolve();
    expect(screen.getByText(/A only/)).toBeInTheDocument(); expect(screen.queryByText(/B hidden/)).not.toBeInTheDocument();
  });

  it("keeps C selected when A then B replays complete after C", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const b = run("run-b"); const c = run("run-c"); const aReplay = deferred<RunEvent[]>(); const bReplay = deferred<RunEvent[]>();
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b, c]); mock.api.listRunEvents = vi.fn().mockImplementation((id) => id === a.id ? aReplay.promise : id === b.id ? bReplay.promise : Promise.resolve([event(c.id, 1, "C event")])); mock.api.getRun = vi.fn().mockImplementation((id) => Promise.resolve(id === a.id ? a : id === b.id ? b : c));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledWith(a.id)); await select(b.id); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledWith(b.id)); await select(c.id);
    await screen.findByText("running · run-c"); await expectSequence(1, "C event"); bReplay.resolve([event(b.id, 1, "B event")]); aReplay.resolve([event(a.id, 1, "A event")]);
    await waitFor(() => expect(screen.getByText("running · run-c")).toBeInTheDocument()); expect(screen.getByText(/C event/)).toBeInTheDocument(); expect(screen.queryByText(/A event|B event/)).not.toBeInTheDocument();
  });

  it("recovers a missed terminal live event through selected-run Retry", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const completed = run("run-a", "completed"); const terminal = event(a.id, 9, "", "run_completed");
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockRejectedValueOnce(new Error("history unavailable")).mockResolvedValueOnce([terminal]); mock.api.getRun = vi.fn().mockResolvedValueOnce(a).mockResolvedValueOnce(completed);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await screen.findByText("Event history could not be loaded. Retry.");
    await userEvent.click(screen.getByRole("button", { name: "Retry" })); await screen.findByText("completed · run-a"); expect(screen.getByText(/#9 run_completed: Completed/)).toBeInTheDocument(); expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });

  it("keeps history Retry independent while selected replay remains pending", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const completed = run("run-a", "completed"); const selectedReplay = deferred<RunEvent[]>(); let eventCalls = 0; let historyCalls = 0;
    mock.api.listRuns = vi.fn().mockImplementation(() => ++historyCalls === 1 ? Promise.resolve([a]) : historyCalls === 2 ? Promise.reject(new Error("history failed")) : Promise.resolve([completed]));
    mock.api.listRunEvents = vi.fn().mockImplementation(() => ++eventCalls === 1 ? selectedReplay.promise : Promise.resolve([])); mock.api.getRun = vi.fn().mockResolvedValueOnce(completed).mockResolvedValueOnce(a);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledTimes(1));
    mock.emit(event(a.id, 2, "", "run_completed")); await screen.findByText("Run history could not be refreshed. Retry."); await screen.findByText("completed · run-a");
    await userEvent.click(screen.getByRole("button", { name: "Retry history" })); await waitFor(() => expect(mock.api.listRuns).toHaveBeenCalledTimes(3)); selectedReplay.resolve([]);
    await waitFor(() => expect(screen.queryByText("Run history could not be refreshed. Retry.")).not.toBeInTheDocument()); expect(screen.getByText("completed · run-a")).toBeInTheDocument(); expect(screen.getByText(/#2 run_completed: Completed/)).toBeInTheDocument(); expect(mock.api.listRunEvents).toHaveBeenCalledTimes(2);
  });

  it("accepts a live event registered during pending persisted replay", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const listener = mock.delayRunRegistration(); const replay = deferred<RunEvent[]>();
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockReturnValue(replay.promise); mock.api.getRun = vi.fn().mockResolvedValue(a);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledWith(a.id)); listener.resolve(); await waitFor(() => expect(mock.activeRunListeners()).toBe(1));
    mock.emit(event(a.id, 2, "live two")); replay.resolve([event(a.id, 1, "persisted one")]); await expectSequence(1, "persisted one"); await expectSequence(2, "live two");
  });

  it("keeps persisted replay usable when run listener registration rejects", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); mock.rejectRunRegistration(new Error("/private/listener: raw failure"));
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockResolvedValue([event(a.id, 1, "persisted")]); mock.api.getRun = vi.fn().mockResolvedValue(a);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await expectSequence(1, "persisted"); await screen.findByText("Live updates are unavailable."); expect(screen.queryByText(/private\/listener|raw failure/)).not.toBeInTheDocument();
  });

  it("cleans a late run listener after final unmount while selected recovery is pending", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const listener = mock.delayRunRegistration(); const replay = deferred<RunEvent[]>();
    mock.api.listRuns = vi.fn().mockResolvedValue([a]); mock.api.listRunEvents = vi.fn().mockReturnValue(replay.promise); mock.api.getRun = vi.fn().mockResolvedValue(a);
    const view = renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledWith(a.id)); view.unmount(); listener.resolve(); replay.resolve([event(a.id, 1, "late")]);
    await waitFor(() => expect(mock.unlisten).toHaveBeenCalledOnce()); expect(mock.activeRunListeners()).toBe(0);
  });
});
