import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ProviderUsageWidget } from "./ProviderUsageWidget";

describe("ProviderUsageWidget", () => {
  it("shows provider names and does not invent a percentage when local usage is unavailable", async () => {
    render(<ProviderUsageWidget />);
    expect(screen.getByText("Codex")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(await screen.findAllByText("Unavailable")).toHaveLength(2);
    expect(screen.queryByText(/% remaining/)).not.toBeInTheDocument();
  });
});
