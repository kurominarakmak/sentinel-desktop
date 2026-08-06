import { describe, expect, it } from "vitest";
import { loadProviderUsage, parseUsedLimitPercentage, type ProviderUsageAdapter } from "./providerUsage";

describe("parseUsedLimitPercentage", () => {
  it("parses a used percentage and derives the remaining allowance", () => {
    expect(parseUsedLimitPercentage("You've used 65% of your weekly limit")).toEqual({ usedPercent: 65, remainingPercent: 35 });
  });

  it("rejects missing and invalid percentages", () => {
    expect(parseUsedLimitPercentage("usage limit reached")).toBeNull();
    expect(parseUsedLimitPercentage("used 101%")).toBeNull();
  });
});

describe("loadProviderUsage", () => {
  it("reports unavailable when no reliable local provider source exists", async () => {
    await expect(loadProviderUsage()).resolves.toEqual([
      { provider: "codex", state: "unavailable", detail: "No supported local Codex usage-limit source." },
      { provider: "claude", state: "unavailable", detail: "No supported local Claude Code usage-limit source." },
    ]);
  });

  it("contains adapter failures as unknown", async () => {
    const failed: ProviderUsageAdapter = { provider: "codex", fetch: async () => { throw new Error("offline"); } };
    await expect(loadProviderUsage([failed])).resolves.toEqual([
      { provider: "codex", state: "unknown", detail: "Usage status could not be retrieved." },
    ]);
  });
});
