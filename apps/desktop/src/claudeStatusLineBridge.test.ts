import { describe, expect, it } from "vitest";
import { installClaudeStatusLineBridge, restoreClaudeStatusLineBridge } from "./claudeStatusLineBridge";
describe("Claude status-line bridge", () => {
  it("is opt-in, never overwrites another status line, and restores its backup", () => {
    expect(installClaudeStatusLineBridge({ statusLine: { command: "other" } })).toBeNull();
    const installed = installClaudeStatusLineBridge({ theme: "dark" })!;
    expect(restoreClaudeStatusLineBridge(installed.settings, installed.backup)).toEqual({ theme: "dark" });
  });
});
