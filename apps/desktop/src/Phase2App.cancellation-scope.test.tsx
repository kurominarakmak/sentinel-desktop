import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { updateCancellationOperationForRequest } from "./Phase2App";
import { createMockPhase2Services, createRunDto, deferred, renderPhase2App } from "./test/phase2Harness";
import type { CancellationResult, RunDto } from "./phase2";

const run = (id: string, overrides: Partial<RunDto> = {}) => createRunDto({ id, task_text: id, cancellable: true, ...overrides });
const select = async (id: string) => userEvent.click(screen.getByRole("button", { name: new RegExp(id) }));

function configureRuns(mock: ReturnType<typeof createMockPhase2Services>, runs: RunDto[]) {
  const getRun = vi.fn().mockImplementation((id: string) => Promise.resolve(runs.find((item) => item.id === id)!));
  mock.api.listRuns = vi.fn().mockResolvedValue(runs);
  mock.api.listRunEvents = vi.fn().mockResolvedValue([]);
  mock.api.getRun = getRun;
  return getRun;
}

describe("Phase2App RunId-scoped cancellation operations", () => {
  it("does not let an older same-run request overwrite a newer operation", () => {
    const operations = { "run-a": { requestId: 2, pending: true, feedback: "new request" }, "run-b": { requestId: 1, pending: true, feedback: "B request" } };
    expect(updateCancellationOperationForRequest(operations, "run-a", 1, { pending: false, feedback: "old result" })).toBe(operations);
    expect(updateCancellationOperationForRequest(operations, "run-a", 2, { pending: false, feedback: "new result" })).toEqual({ "run-a": { requestId: 2, pending: false, feedback: "new result" }, "run-b": operations["run-b"] });
  });

  it("does not transfer A pending state to independently cancellable B", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const b = run("run-b"); const pendingA = deferred<CancellationResult>();
    const getRun = configureRuns(mock, [a, b]); mock.api.cancelRun = vi.fn().mockReturnValue(pendingA.promise);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Cancelling…" })).toBeDisabled(); await select(b.id);
    expect(screen.getByText("running · run-b")).toBeInTheDocument(); expect(screen.queryByRole("button", { name: "Cancelling…" })).not.toBeInTheDocument(); expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled(); expect(mock.api.cancelRun).toHaveBeenCalledTimes(1);
    const getCallsBeforeResult = getRun.mock.calls.length; pendingA.resolve("cancellation_requested"); await waitFor(() => expect(getRun.mock.calls.length).toBeGreaterThan(getCallsBeforeResult));
  });

  it("allows B to cancel while A remains pending", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const b = run("run-b"); const pendingA = deferred<CancellationResult>(); const pendingB = deferred<CancellationResult>();
    const getRun = configureRuns(mock, [a, b]); mock.api.cancelRun = vi.fn().mockImplementation((id: string) => id === a.id ? pendingA.promise : pendingB.promise);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); await select(b.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(mock.api.cancelRun).toHaveBeenNthCalledWith(1, a.id); expect(mock.api.cancelRun).toHaveBeenNthCalledWith(2, b.id); expect(screen.getByRole("button", { name: "Cancelling…" })).toBeDisabled();
    const getCallsBeforeResult = getRun.mock.calls.length; pendingB.resolve("cancellation_requested"); await waitFor(() => expect(getRun.mock.calls.length).toBeGreaterThan(getCallsBeforeResult)); const getCallsAfterB = getRun.mock.calls.length; pendingA.resolve("cancellation_requested"); await waitFor(() => expect(getRun.mock.calls.length).toBeGreaterThan(getCallsAfterB));
  });

  it.each<[CancellationResult, string]>([["termination_failed", "Cancellation could not be completed."], ["run_not_active", "This run is no longer active in this desktop session."]])("keeps A's %s feedback hidden while B is selected", async (result, feedback) => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const b = run("run-b"); const pendingA = deferred<CancellationResult>();
    const getRun = configureRuns(mock, [a, b]); mock.api.cancelRun = vi.fn().mockReturnValue(pendingA.promise);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); await select(b.id);
    const getCallsBeforeResult = getRun.mock.calls.length; pendingA.resolve(result); await waitFor(() => expect(getRun.mock.calls.length).toBeGreaterThan(getCallsBeforeResult)); expect(screen.getByText("running · run-b")).toBeInTheDocument(); expect(screen.queryByText(feedback)).not.toBeInTheDocument(); expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
    await select(a.id); expect(await screen.findByText(feedback)).toBeInTheDocument();
  });

  it("keeps a rejected A cancellation hidden after B then C selection changes", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const b = run("run-b"); const c = run("run-c"); const pendingA = deferred<CancellationResult>();
    configureRuns(mock, [a, b, c]); mock.api.cancelRun = vi.fn().mockReturnValue(pendingA.promise);
    renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); await select(b.id); await select(c.id);
    pendingA.reject(new Error("/private/process stderr SELECT token=hunter2")); await waitFor(() => expect(screen.getByText("running · run-c")).toBeInTheDocument()); expect(screen.queryByText("Cancellation could not be requested.")).not.toBeInTheDocument(); expect(screen.queryByText(/private\/process|SELECT|hunter2/)).not.toBeInTheDocument(); expect(screen.getByRole("button", { name: "Cancel" })).toBeEnabled();
    await select(a.id); expect(await screen.findByText("Cancellation could not be requested.")).toBeInTheDocument();
  });

  it("does not update React state after unmount while A and B cancellations settle", async () => {
    const mock = createMockPhase2Services(); const a = run("run-a"); const b = run("run-b"); const pendingA = deferred<CancellationResult>(); const pendingB = deferred<CancellationResult>();
    configureRuns(mock, [a, b]); mock.api.cancelRun = vi.fn().mockImplementation((id: string) => id === a.id ? pendingA.promise : pendingB.promise);
    const view = renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ }); await select(a.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); await select(b.id); await userEvent.click(screen.getByRole("button", { name: "Cancel" })); view.unmount();
    pendingA.resolve("cancellation_requested"); pendingB.reject(new Error("internal cancellation failure")); await Promise.allSettled([pendingA.promise, pendingB.promise]); await Promise.resolve(); expect(mock.activeRunListeners()).toBe(0); expect(mock.unlisten).toHaveBeenCalledOnce();
  });
});
