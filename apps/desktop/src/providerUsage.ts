export type ProviderId = "codex" | "claude";
export type UsageWindow = { id: "primary" | "secondary" | "five_hour" | "seven_day"; usedPercent: number; windowDurationMins?: number; resetsAt: string };
export type ProviderUsage =
  | { provider: ProviderId; state: "available"; windows: UsageWindow[]; source: string }
  | { provider: ProviderId; state: "unavailable"; detail: string }
  | { provider: ProviderId; state: "unknown"; detail: string };

export interface ProviderUsageAdapter { provider: ProviderId; fetch(): Promise<ProviderUsage>; }
type Json = Record<string, unknown>;

const object = (value: unknown): Json | null => value !== null && typeof value === "object" && !Array.isArray(value) ? value as Json : null;
const percent = (value: unknown): number | null => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 100 ? value : null;
const resetTime = (value: unknown): string | null => {
  if (typeof value === "string" && value.trim()) return value;
  if (typeof value === "number" && Number.isFinite(value) && value > 0) return new Date(value * 1000).toISOString();
  return null;
};

function window(id: UsageWindow["id"], value: unknown): UsageWindow | null {
  const entry = object(value); if (!entry) return null;
  const usedPercent = percent(entry.usedPercent ?? entry.used_percentage);
  const resetsAt = resetTime(entry.resetsAt ?? entry.resets_at);
  if (usedPercent === null || !resetsAt) return null;
  const duration = entry.windowDurationMins ?? entry.window_duration_mins;
  return { id, usedPercent, resetsAt, ...(typeof duration === "number" && duration > 0 ? { windowDurationMins: duration } : {}) };
}

export function parseCodexRateLimits(payload: unknown): UsageWindow[] | null {
  const limits = object(payload)?.rateLimits ?? payload;
  const record = object(limits); if (!record) return null;
  const windows = [window("primary", record.primary), window("secondary", record.secondary)].filter((entry): entry is UsageWindow => entry !== null);
  return windows.length ? windows : null;
}

export function parseClaudeStatusLine(payload: unknown): UsageWindow[] | null {
  const rateLimits = object(payload)?.rate_limits; const record = object(rateLimits); if (!record) return null;
  const windows = [window("five_hour", record.five_hour), window("seven_day", record.seven_day)].filter((entry): entry is UsageWindow => entry !== null);
  return windows.length ? windows : null;
}

export function codexUsageAdapter(read: () => Promise<unknown>): ProviderUsageAdapter {
  return { provider: "codex", async fetch() { const windows = parseCodexRateLimits(await read()); return windows ? { provider: "codex", state: "available", windows, source: "Codex App Server account/rateLimits/read" } : { provider: "codex", state: "unknown", detail: "Waiting for a valid Codex rate-limit update." }; } };
}
export function claudeUsageAdapter(read: () => Promise<unknown>): ProviderUsageAdapter {
  return { provider: "claude", async fetch() { const windows = parseClaudeStatusLine(await read()); return windows ? { provider: "claude", state: "available", windows, source: "Claude Code status-line JSON" } : { provider: "claude", state: "unavailable", detail: "Claude Code did not provide rate limits for this account." }; } };
}

export async function loadProviderUsage(adapters: readonly ProviderUsageAdapter[]): Promise<ProviderUsage[]> {
  return Promise.all(adapters.map(async adapter => { try { return await adapter.fetch(); } catch { return { provider: adapter.provider, state: "unknown", detail: "Usage status could not be retrieved." }; } }));
}
