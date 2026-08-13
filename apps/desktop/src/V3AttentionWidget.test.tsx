import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), emit: vi.fn(), prompt: { show: vi.fn(), unminimize: vi.fn(), setFocus: vi.fn() } }));
const { invoke, listen, emit, prompt } = mocks;
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen, emit: mocks.emit }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ label: "status" }), Window: { getByLabel: vi.fn().mockResolvedValue(mocks.prompt) } }));
import { V3AttentionWidget } from "./V3AttentionWidget";

const task = (lifecycle = "implementing", recoveryRequired = false) => ({ id: "task-1", summary: "Ship status", lifecycle, recoveryRequired });
const detail = (lifecycle = "implementing", approval = false) => ({ task: task(lifecycle), activity: [], worktree: null, diff: null, validations: [], findings: [], repairRounds: [], finalApprovalPacket: null, finalApprovalId: approval ? "approval-1" : null, agent: "claude" });
describe("V3AttentionWidget", () => {
  beforeEach(() => { invoke.mockReset(); listen.mockReset(); emit.mockReset(); prompt.show.mockReset(); prompt.unminimize.mockReset(); prompt.setFocus.mockReset(); listen.mockResolvedValue(() => undefined); invoke.mockImplementation((command: string) => command === "list_v3_tasks" ? Promise.resolve([task()]) : Promise.resolve(detail())); });
  it("renders durable state and routes task detail without polling", async () => { render(<V3AttentionWidget />); await screen.findByText("Working"); expect(screen.getByText("Claude")).toBeInTheDocument(); expect(screen.getByText("Ship status")).toBeInTheDocument(); await userEvent.click(screen.getByRole("button", { name: "Open Task Detail" })); await waitFor(() => expect(prompt.show).toHaveBeenCalledOnce()); expect(emit).toHaveBeenCalledWith("v3-open-task-detail", "task-1"); expect(listen).toHaveBeenCalledWith("v3-state-changed", expect.any(Function)); await userEvent.click(screen.getByRole("button", { name: "Stop" })); expect(invoke).toHaveBeenCalledWith("cancel_codex_session_task", { request: { taskId: "task-1" } }); });
  it("wires supported stop and approval actions", async () => { invoke.mockImplementation((command: string) => command === "list_v3_tasks" ? Promise.resolve([task("awaiting_approval")]) : Promise.resolve(detail("awaiting_approval", true))); render(<V3AttentionWidget />); await screen.findByRole("button", { name: "Approve" }); await userEvent.click(screen.getByRole("button", { name: "Approve" })); expect(invoke).toHaveBeenCalledWith("decide_v3_final_approval", { request: { approvalId: "approval-1", approve: true } }); });
});
