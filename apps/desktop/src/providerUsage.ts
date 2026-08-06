export type ProviderId = "codex" | "claude";

export type ProviderUsage =
  | { provider: ProviderId; state: "available"; usedPercent: number; remainingPercent: number; source: string }
  | { provider: ProviderId; state: "unavailable"; detail: string }
  | { provider: ProviderId; state: "unknown"; detail: string };

export interface ProviderUsageAdapter {
  provider: ProviderId;
  fetch(): Promise<ProviderUsage>;
}

export function parseUsedLimitPercentage(text: string): { usedPercent: number; remainingPercent: number } | null {
  const match = /\b(?:you(?:'|’)?ve\s+)?used\s+(\d{1,3})%/i.exec(text);
  if (!match) return null;
  const usedPercent = Number(match[1]);
  return Number.isInteger(usedPercent) && usedPercent >= 0 && usedPercent <= 100
    ? { usedPercent, remainingPercent: 100 - usedPercent }
    : null;
}

function unavailable(provider: ProviderId, detail: string): ProviderUsage {
  return { provider, state: "unavailable", detail };
}

// Neither CLI exposes a stable, authenticated local quota/limit API. Do not
// infer account allowance from transcript token counts or warning text.
export const codexUsageAdapter: ProviderUsageAdapter = {
  provider: "codex",
  async fetch() {
    return unavailable("codex", "No supported local Codex usage-limit source.");
  },
};

export const claudeUsageAdapter: ProviderUsageAdapter = {
  provider: "claude",
  async fetch() {
    return unavailable("claude", "No supported local Claude Code usage-limit source.");
  },
};

export async function loadProviderUsage(
  adapters: readonly ProviderUsageAdapter[] = [codexUsageAdapter, claudeUsageAdapter],
): Promise<ProviderUsage[]> {
  return Promise.all(adapters.map(async (adapter) => {
    try {
      return await adapter.fetch();
    } catch {
      return { provider: adapter.provider, state: "unknown", detail: "Usage status could not be retrieved." };
    }
  }));
}
