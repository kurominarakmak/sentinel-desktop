import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createMockPhase2Services, createRunDto, createRunEvent, deferred, renderPhase2App } from "./test/phase2Harness";
import type { RunStatus } from "./phase2";

const select = async (id = "run-a") => userEvent.click(screen.getByRole("button", { name: new RegExp(id) }));
const run = (id: string, status: RunStatus, cancellable = !["completed", "failed", "cancelled"].includes(status)) => createRunDto({ id, status, cancellable, task_text: id });

describe("Phase2App recovery failures", () => {
  it("continues terminal metadata recovery when event history fails", async () => {
    const mock = createMockPhase2Services(); const running = run("run-a", "running", true); const completed = run("run-a", "completed", false); const live = createRunEvent({ sequence_number: 1, payload: { text: "live event" } });
    mock.api.listRuns = vi.fn().mockResolvedValue([running]); mock.api.listRunEvents = vi.fn().mockResolvedValueOnce([live]).mockRejectedValueOnce(new Error("/private/sqlite: raw error")); mock.api.getRun = vi.fn().mockResolvedValueOnce(running).mockResolvedValueOnce(completed);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(); await screen.findByText("running · run-a"); expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    mock.emit(createRunEvent({ run_id: "run-a", sequence_number: 2, event_type: "run_completed", payload: {} }));
    await screen.findByText("completed · run-a"); expect(mock.api.getRun).toHaveBeenCalledTimes(2); expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument(); expect(screen.getByText(/live event/)).toBeInTheDocument(); expect(screen.getByText("Event history could not be loaded. Retry.")).toBeInTheDocument(); expect(screen.queryByText(/private\/sqlite|raw error/)).not.toBeInTheDocument();
  });

  it("continues cancellation metadata recovery when event history fails", async () => {
    const mock = createMockPhase2Services(); const running = run("run-a", "running", true); const cancelling = run("run-a", "cancelling", false);
    mock.api.listRuns = vi.fn().mockResolvedValue([running]); mock.api.listRunEvents = vi.fn().mockResolvedValueOnce([createRunEvent({ payload: { text: "kept event" } })]).mockRejectedValueOnce(new Error("internal history failure")); mock.api.getRun = vi.fn().mockResolvedValueOnce(running).mockResolvedValueOnce(cancelling); mock.api.cancelRun = vi.fn().mockResolvedValue("cancellation_requested");
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(); await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await screen.findByText("cancelling · run-a"); expect(mock.api.getRun).toHaveBeenCalledTimes(2); expect(screen.getByText(/kept event/)).toBeInTheDocument(); expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });

  it("does not offer cancellation for a persisted nonterminal run without ownership", async () => {
    const mock = createMockPhase2Services(); const detached = run("run-a", "running", false);
    mock.api.listRuns = vi.fn().mockResolvedValue([detached]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValue(detached);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(); await screen.findByText("running · run-a"); expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });

  it("handles run_not_active and refreshes capability before another cancellation", async () => {
    const mock = createMockPhase2Services(); const active = run("run-a", "running", true); const detached = run("run-a", "running", false);
    mock.api.listRuns = vi.fn().mockResolvedValue([active]); mock.api.listRunEvents = vi.fn().mockResolvedValue([]); mock.api.getRun = vi.fn().mockResolvedValueOnce(active).mockResolvedValueOnce(detached); mock.api.cancelRun = vi.fn().mockResolvedValue("run_not_active");
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(); await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await screen.findByText("This run is no longer active in this desktop session."); expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument(); expect(mock.api.cancelRun).toHaveBeenCalledOnce(); expect(screen.getByText("running · run-a")).toBeInTheDocument();
  });

  it("keeps selected detail and reports a safe retryable event-history error", async () => {
    const mock = createMockPhase2Services(); const initial = run("run-a", "queued", false); const authoritative = run("run-a", "running", true);
    mock.api.listRuns = vi.fn().mockResolvedValue([initial]); mock.api.listRunEvents = vi.fn().mockRejectedValue(new Error("/private/path: SQL failure")); mock.api.getRun = vi.fn().mockResolvedValue(authoritative);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(); await screen.findByText("running · run-a"); expect(screen.getByText("Event history could not be loaded. Retry.")).toBeInTheDocument(); expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument(); expect(screen.queryByText(/private\/path|SQL failure/)).not.toBeInTheDocument();
  });

  it("keeps known detail and events on metadata failure, then retries successfully", async () => {
    const mock = createMockPhase2Services(); const history = run("run-a", "queued", false); const recovered = run("run-a", "running", true); const event = createRunEvent({ payload: { text: "persisted event" } });
    mock.api.listRuns = vi.fn().mockResolvedValue([history]); mock.api.listRunEvents = vi.fn().mockResolvedValueOnce([event]).mockResolvedValueOnce([event]); mock.api.getRun = vi.fn().mockRejectedValueOnce(new Error("/private/path: stderr")).mockResolvedValueOnce(recovered);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(); await screen.findByText("queued · run-a"); expect(screen.getByText(/persisted event/)).toBeInTheDocument(); expect(screen.getByText("Run details could not be refreshed. Retry.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" })); await screen.findByText("running · run-a"); expect(screen.queryByText("Run details could not be refreshed. Retry.")).not.toBeInTheDocument(); expect(screen.getByText(/persisted event/)).toBeInTheDocument();
  });

  it("does not show A recovery errors after the user selects B", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a", "queued", false); const b = run("run-b", "running", true); const aEvents = deferred<ReturnType<typeof createRunEvent>[]>();
    mock.api.listRuns = vi.fn().mockResolvedValue([a, b]); mock.api.listRunEvents = vi.fn().mockImplementation((id) => id === "run-a" ? aEvents.promise : Promise.resolve([])); mock.api.getRun = vi.fn().mockImplementation((id) => Promise.resolve(id === "run-b" ? b : a));
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select("run-a"); await waitFor(() => expect(mock.api.listRunEvents).toHaveBeenCalledWith("run-a")); await select("run-b"); await screen.findByText("running · run-b"); aEvents.reject(new Error("A failed"));
    await waitFor(() => expect(screen.getByText("running · run-b")).toBeInTheDocument()); expect(screen.queryByText("Event history could not be loaded. Retry.")).not.toBeInTheDocument();
  });
});
