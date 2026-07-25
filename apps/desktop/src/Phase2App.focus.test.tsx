import { StrictMode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Phase2App } from "./Phase2App";
import { createMockPhase2Services, createRunDto, createRunEvent, renderPhase2App } from "./test/phase2Harness";

async function renderFocusedApp() {
  const mock = createMockPhase2Services(); const run = createRunDto({ id: "run-a", status: "running" });
  mock.api.listRuns = vi.fn().mockResolvedValue([run]); mock.api.listRunEvents = vi.fn().mockResolvedValue([createRunEvent({ payload: { text: "persisted event" } })]); mock.api.getRun = vi.fn().mockResolvedValue(run);
  const view = renderPhase2App(mock.services); await screen.findByRole("button", { name: /run-a/ });
  return { mock, view };
}

describe("Phase2App focus lifecycle", () => {
  it("restores prompt focus without remounting or changing UI state", async () => {
    const user = userEvent.setup(); const { mock } = await renderFocusedApp(); const task = screen.getByRole("textbox", { name: "Task" }); const scenario = screen.getByRole("combobox", { name: "Scenario" });
    await user.type(task, "preserve this task"); await user.selectOptions(scenario, "failure"); await user.click(screen.getByRole("button", { name: /run-a/ })); await screen.findByText("running · run-a"); await screen.findByText(/persisted event/);
    scenario.focus(); expect(document.activeElement).toBe(scenario); const before = task; mock.emitFocus();
    expect(document.activeElement).toBe(task); expect(task).toHaveValue("preserve this task"); expect(scenario).toHaveValue("failure"); expect(screen.getByText("running · run-a")).toBeInTheDocument(); expect(screen.getByText(/persisted event/)).toBeInTheDocument(); expect(screen.getByRole("textbox", { name: "Task" })).toBe(before); expect(mock.focusRegistrations()).toBe(1);
  });

  it("restores focus after Escape without losing task text", async () => {
    const user = userEvent.setup(); const { mock } = await renderFocusedApp(); const task = screen.getByRole("textbox", { name: "Task" }); const scenario = screen.getByRole("combobox", { name: "Scenario" });
    await user.type(task, "keep after hide"); scenario.focus(); await user.keyboard("{Escape}"); expect(mock.services.hidePrompt).toHaveBeenCalledOnce();
    mock.emitFocus(); expect(document.activeElement).toBe(task); expect(task).toHaveValue("keep after hide");
  });

  it("falls back to plain focus when focus options are unsupported", async () => {
    const { mock } = await renderFocusedApp(); const task = screen.getByRole("textbox", { name: "Task" }); const originalFocus = task.focus.bind(task); const focus = vi.fn((options?: FocusOptions) => { if (options) throw new TypeError("options unsupported"); originalFocus(); });
    Object.defineProperty(task, "focus", { configurable: true, value: focus }); document.body.focus(); mock.emitFocus();
    expect(focus).toHaveBeenCalledTimes(2); expect(document.activeElement).toBe(task);
  });

  it("registers once across a stable rerender and cleans up normally", async () => {
    const mock = createMockPhase2Services(); const view = renderPhase2App(mock.services); await waitFor(() => expect(mock.focusRegistrations()).toBe(1));
    view.rerender(<Phase2App services={mock.services} />); expect(mock.focusRegistrations()).toBe(1); expect(mock.focusUnlisten).not.toHaveBeenCalled(); view.unmount(); expect(mock.focusUnlisten).toHaveBeenCalledOnce();
  });

  it("cleans a late focus registration after unmount", async () => {
    const mock = createMockPhase2Services(); const gate = mock.delayFocusRegistration(); const view = renderPhase2App(mock.services); view.unmount(); gate.resolve();
    await waitFor(() => expect(mock.focusUnlisten).toHaveBeenCalledOnce()); expect(mock.activeFocusListeners()).toBe(0);
  });

  it("keeps the app usable when focus registration rejects", async () => {
    const mock = createMockPhase2Services(); mock.rejectFocusRegistration(new Error("/private/path: raw OS error")); renderPhase2App(mock.services);
    await screen.findByRole("textbox", { name: "Task" }); await Promise.resolve(); expect(screen.queryByText(/private\/path|raw OS error/)).not.toBeInTheDocument();
  });

  it("keeps focus resources balanced under StrictMode", async () => {
    const mock = createMockPhase2Services(); const view = render(<StrictMode><Phase2App services={mock.services} /></StrictMode>);
    await waitFor(() => expect(mock.activeFocusListeners()).toBe(1)); expect(mock.activeFocusListeners()).toBeLessThanOrEqual(1); view.unmount();
    await waitFor(() => expect(mock.activeFocusListeners()).toBe(0)); expect(mock.focusUnlisten).toHaveBeenCalledTimes(mock.focusRegistrations());
  });

  it("cleans late focus registrations after StrictMode final disposal", async () => {
    const mock = createMockPhase2Services(); const gate = mock.delayFocusRegistration(); const view = render(<StrictMode><Phase2App services={mock.services} /></StrictMode>);
    view.unmount(); gate.resolve(); await waitFor(() => expect(mock.focusRegistrations()).toBeGreaterThan(0));
    expect(mock.activeFocusListeners()).toBe(0); expect(mock.focusUnlisten).toHaveBeenCalledTimes(mock.focusRegistrations());
  });
});
