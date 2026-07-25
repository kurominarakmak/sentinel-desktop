import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockPhase2Services, createRunDto, createRunEvent, deferred, renderPhase2App } from "./test/phase2Harness";
import type { RunStatus } from "./phase2";

const run = (id: string, status: RunStatus, task_text = id) => createRunDto({ id, status, task_text });
const lifecycle = (event_type: string, sequence_number: number, run_id = "run-a") => createRunEvent({ event_type, sequence_number, run_id, payload: {} });
const selectA = async () => { await userEvent.click(screen.getByRole("button", { name: /run-a/ })); };

describe("Phase2App lifecycle refreshes", () => {
  it("refreshes preparing, running, and cancelling RunDto metadata", async () => {
    const mock = createMockPhase2Services(); const queued = run("run-a", "queued"); const preparing = run("run-a", "preparing"); const running = run("run-a", "running"); const cancelling = run("run-a", "cancelling");
    mock.api.listRuns = vi.fn().mockResolvedValue([queued]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValueOnce(queued).mockResolvedValueOnce(preparing).mockResolvedValueOnce(running).mockResolvedValueOnce(cancelling);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(1));
    mock.emit(lifecycle("run_preparing", 2)); await screen.findByText(`preparing · ${queued.id}`);
    mock.emit(lifecycle("run_running", 3)); await screen.findByText(`running · ${queued.id}`);
    mock.emit(lifecycle("run_cancelling", 4)); await screen.findByText(`cancelling · ${queued.id}`);
    expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
  });

  it("keeps a newer lifecycle response when older getRun resolves last", async () => {
    const mock = createMockPhase2Services(); const queued = run("run-a", "queued"); const preparing = deferred<typeof queued>(); const running = deferred<typeof queued>();
    mock.api.listRuns = vi.fn().mockResolvedValue([queued]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValueOnce(queued).mockReturnValueOnce(preparing.promise).mockReturnValueOnce(running.promise);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(1));
    mock.emit(lifecycle("run_preparing", 2)); mock.emit(lifecycle("run_running", 3)); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(3));
    running.resolve(run("run-a", "running")); await screen.findByText("running · run-a"); preparing.resolve(run("run-a", "preparing"));
    await waitFor(() => expect(screen.getByText("running · run-a")).toBeInTheDocument()); expect(screen.getByRole("button", { name: /running.*run-a/ })).toBeInTheDocument();
  });

  it("patches an unselected run without replacing A detail or events", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a", "running", "task A"); const b = run("run-b", "queued", "task B"); const bRunning = run("run-b", "running", "task B");
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b]); mock.api.listRunEvents = vi.fn().mockResolvedValue([createRunEvent({ run_id: "run-a", payload: { text: "A event" } })]); mock.api.getRun = vi.fn().mockImplementation((id) => Promise.resolve(id === "run-b" ? bRunning : a));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await screen.findByText("running · run-a");
    mock.emit(lifecycle("run_running", 2, "run-b")); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledWith("run-b"));
    expect(screen.getByText("running · run-a")).toBeInTheDocument(); expect(screen.getByRole("button", { name: /running.*run-b/ })).toBeInTheDocument(); expect(screen.getByText(/A event/)).toBeInTheDocument();
  });

  it("deduplicates a nonterminal lifecycle sequence", async () => {
    const mock = createMockPhase2Services(); const queued = run("run-a", "queued"); const preparing = run("run-a", "preparing");
    mock.api.listRuns = vi.fn().mockResolvedValue([queued]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValueOnce(queued).mockResolvedValueOnce(preparing);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(1));
    mock.emit(lifecycle("run_preparing", 2)); mock.emit(lifecycle("run_preparing", 2)); await screen.findByText("preparing · run-a");
    expect(mock.api.getRun).toHaveBeenCalledTimes(2); expect(screen.getAllByText(/#2 run_preparing/)).toHaveLength(1);
  });

  it("deduplicates a terminal sequence and its final refresh flow", async () => {
    const mock = createMockPhase2Services(); const running = run("run-a", "running"); const completed = run("run-a", "completed");
    mock.api.listRuns = vi.fn().mockResolvedValue([running]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValueOnce(running).mockResolvedValueOnce(completed);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(1));
    mock.emit(lifecycle("run_completed", 8)); mock.emit(lifecycle("run_completed", 8)); await screen.findByText("completed · run-a");
    expect(mock.api.getRun).toHaveBeenCalledTimes(2); expect(mock.api.listRunEvents).toHaveBeenCalledTimes(2); expect(mock.api.listRuns).toHaveBeenCalledTimes(2); expect(screen.getAllByText(/#8 run_completed/)).toHaveLength(1);
  });

  it("recovers from a failed lifecycle metadata request on a newer sequence", async () => {
    const mock = createMockPhase2Services(); const queued = run("run-a", "queued"); const running = run("run-a", "running");
    mock.api.listRuns = vi.fn().mockResolvedValue([queued]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValueOnce(queued).mockRejectedValueOnce(new Error("safe background failure")).mockResolvedValueOnce(running);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(1));
    mock.emit(lifecycle("run_preparing", 2)); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(2)); expect(screen.getByText("queued · run-a")).toBeInTheDocument();
    mock.emit(lifecycle("run_running", 3)); await screen.findByText("running · run-a");
  });

  it("does not fetch metadata for a non-lifecycle message", async () => {
    const mock = createMockPhase2Services(); const queued = run("run-a", "queued");
    mock.api.listRuns = vi.fn().mockResolvedValue([queued]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValue(queued);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(1));
    mock.emit(createRunEvent({ run_id: "run-a", sequence_number: 2, event_type: "message", payload: { text: "ordinary message" } }));
    await screen.findByText(/ordinary message/); expect(mock.api.getRun).toHaveBeenCalledTimes(1);
  });

  it("does not let stale explicit recovery regress newer lifecycle metadata", async () => {
    const mock = createMockPhase2Services(); const queued = run("run-a", "queued"); const recovery = deferred<typeof queued>(); const lifecycleRun = deferred<typeof queued>(); let calls = 0;
    mock.api.listRuns = vi.fn().mockResolvedValue([queued]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockImplementation(() => ++calls === 1 ? recovery.promise : lifecycleRun.promise);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await selectA(); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(1));
    mock.emit(lifecycle("run_running", 3)); await waitFor(() => expect(mock.api.getRun).toHaveBeenCalledTimes(2)); lifecycleRun.resolve(run("run-a", "running")); await screen.findByText("running · run-a"); recovery.resolve(queued);
    await waitFor(() => expect(screen.getByText("running · run-a")).toBeInTheDocument()); expect(screen.getByRole("button", { name: /running.*run-a/ })).toBeInTheDocument();
  });
});
