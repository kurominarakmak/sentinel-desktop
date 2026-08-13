import { describe, expect, it } from "vitest";
import { activeTaskId, attentionState, type V3Task } from "./V3Workflow";
const task = (lifecycle: string, recoveryRequired = false): V3Task => ({ id: "t", summary: "x", lifecycle, recoveryRequired });
describe("V3 attention mapping", () => {
  it("maps approval, failure, recovery, and ready states without inference", () => {
    expect(attentionState(task("implementing"))).toBe("working");
    expect(attentionState(task("awaiting_approval"))).toBe("approval-required");
    expect(attentionState(task("failed"))).toBe("failed");
    expect(attentionState(task("reviewing", true))).toBe("recovery-required");
    expect(attentionState(task("ready_for_human"))).toBe("ready-for-review");
  });
});
describe("V3 active task restoration", () => {
  it("keeps an available selection and otherwise restores the newest non-terminal task", () => {
    const tasks = [task("completed"), { ...task("implementing"), id: "active" }];
    expect(activeTaskId(tasks)).toBe("active");
    expect(activeTaskId(tasks, "t")).toBe("t");
  });
});
