import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ApprovalPanel } from "./ApprovalPanel";

describe("ApprovalPanel", () => {
  it("records a backend-confirmed decision without claiming execution", async () => {
    const decide = vi.fn().mockResolvedValue({ approvalReference: "cr_12345678901234567890123456789012", profile: "safe", actionCategory: "read_repository", summary: "Review change", state: "allowed_once", version: 1, decisionAvailable: false });
    render(<ApprovalPanel projectId="project" worktreeId="worktree" api={{ capability: vi.fn().mockResolvedValue({ available: true, hardDeniedCategories: [] }), listPending: vi.fn().mockResolvedValue([{ approvalReference: "cr_12345678901234567890123456789012", profile: "safe", actionCategory: "read_repository", summary: "Review change", state: "pending", version: 0, decisionAvailable: true }]), query: vi.fn(), decide }} />);
    await screen.findByText("Decisions are recorded; runtime delivery is not available in this phase.");
    fireEvent.change(screen.getByLabelText("Task key"), { target: { value: "task" } });
    fireEvent.click(screen.getByText("Refresh approvals"));
    await screen.findByText("Review change");
    fireEvent.click(screen.getByText("Approve once"));
    await waitFor(() => expect(decide).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("Approved once decision recorded.")).toBeTruthy();
    expect(screen.queryByText(/action executed/i)).toBeNull();
  });
});
