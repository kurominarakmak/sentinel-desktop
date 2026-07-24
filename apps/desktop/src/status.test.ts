import { describe, expect, it } from "vitest";
import { statusFor } from "./status";

describe("statusFor", () => {
  it("maps normalized events to tray states", () => {
    expect(statusFor({ type: "session_started" })).toBe("Running");
    expect(statusFor({ type: "waiting_for_input" })).toBe("Waiting");
    expect(statusFor({ type: "completed" })).toBe("Completed");
    expect(statusFor({ type: "failed" })).toBe("Failed");
  });
});
