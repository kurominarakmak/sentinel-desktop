import { describe, expect, it } from "vitest";
import { approvalApi, approvalStateText } from "./approval";

describe("Phase 7 approval contract", () => {
  it("uses only explicit ownership and opaque reference command arguments", async () => {
    const calls: Array<{ name: string; args?: Record<string, unknown> }> = [];
    const api = approvalApi(async <T>(name: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push({ name, args });
      return [] as T;
    });
    await api.listPending({ projectId: "p", worktreeId: "w", taskKey: "t" });
    expect(calls).toEqual([{ name: "list_pending_approvals", args: { request: { projectId: "p", worktreeId: "w", taskKey: "t" } } }]);
    expect(approvalStateText("hard_denied")).toBe("Denied by policy");
  });
});
