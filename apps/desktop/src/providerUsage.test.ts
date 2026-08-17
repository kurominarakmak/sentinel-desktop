import { describe, expect, it } from "vitest";
import { claudeUsageAdapter, codexUsageAdapter, loadProviderUsage, parseClaudeStatusLine, parseCodexRateLimits } from "./providerUsage";

describe("provider usage fixtures", () => {
  it("maps available Codex primary and secondary windows", () => {
    expect(parseCodexRateLimits({ rateLimits: { primary: { usedPercent: 25, windowDurationMins: 300, resetsAt: "2026-08-06T12:00:00Z" }, secondary: { usedPercent: 80, windowDurationMins: 10080, resetsAt: "2026-08-10T12:00:00Z" } } })).toEqual([
      { id: "primary", usedPercent: 25, windowDurationMins: 300, resetsAt: "2026-08-06T12:00:00Z" }, { id: "secondary", usedPercent: 80, windowDurationMins: 10080, resetsAt: "2026-08-10T12:00:00Z" },
    ]);
  });
  it("maps Codex epoch reset timestamps", () => {
    expect(parseCodexRateLimits({ primary: { usedPercent: 90, resetsAt: 1786298259 } })).toEqual([
      { id: "primary", usedPercent: 90, resetsAt: "2026-08-09T17:57:39.000Z" },
    ]);
  });
  it("keeps partial windows and rejects absent or malformed rate data", () => {
    expect(parseCodexRateLimits({ primary: { usedPercent: 10, resetsAt: "later" } })).toEqual([{ id: "primary", usedPercent: 10, resetsAt: "later" }]);
    expect(parseCodexRateLimits({ primary: { usedPercent: 101, resetsAt: "later" } })).toBeNull();
    expect(parseCodexRateLimits({})).toBeNull();
  });
  it("maps official Claude status-line windows", () => {
    expect(parseClaudeStatusLine({ rate_limits: { five_hour: { used_percentage: 12, resets_at: "2026-08-06T12:00:00Z" }, seven_day: { used_percentage: 60, resets_at: "2026-08-10T12:00:00Z" } } })).toEqual([
      { id: "five_hour", usedPercent: 12, resetsAt: "2026-08-06T12:00:00Z" }, { id: "seven_day", usedPercent: 60, resetsAt: "2026-08-10T12:00:00Z" },
    ]);
  });
  it("handles absent, malformed, and updated provider data", async () => {
    const codex = codexUsageAdapter(async () => ({ rateLimits: { primary: { usedPercent: 10, resetsAt: "first" } } }));
    const claude = claudeUsageAdapter(async () => ({ rate_limits: { five_hour: { used_percentage: "bad", resets_at: "later" } } }));
    await expect(loadProviderUsage([codex, claude])).resolves.toEqual([
      { provider: "codex", state: "available", source: "Codex App Server account/rateLimits/read", windows: [{ id: "primary", usedPercent: 10, resetsAt: "first" }] },
      { provider: "claude", state: "unavailable", detail: "Claude Code did not provide rate limits for this account." },
    ]);
    const updated = codexUsageAdapter(async () => ({ primary: { usedPercent: 45, resetsAt: "updated" } }));
    await expect(updated.fetch()).resolves.toMatchObject({ state: "available", windows: [{ usedPercent: 45, resetsAt: "updated" }] });
  });
});
