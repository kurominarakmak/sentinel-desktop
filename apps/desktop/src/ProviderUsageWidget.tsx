import { useEffect, useState } from "react";
import { loadProviderUsage, type ProviderUsage, type ProviderUsageAdapter } from "./providerUsage";
import codexLogo from "./assets/codex-mark.svg";
import claudeLogo from "./assets/claude-mark.svg";
import "./ProviderUsageWidget.css";

const initial: ProviderUsage[] = ["codex", "claude"].map(provider => ({ provider: provider as "codex" | "claude", state: "unknown", detail: "Waiting for provider update…" }));
export function ProviderUsageWidget({ adapters = [] }: { adapters?: readonly ProviderUsageAdapter[] }) {
  const [usage, setUsage] = useState<ProviderUsage[]>(initial);
  useEffect(() => { if (adapters.length) void loadProviderUsage(adapters).then(setUsage); }, [adapters]);
  return <section className="provider-usage" aria-label="Provider usage limits"><h2>Provider usage</h2>{usage.map(entry => <Row key={entry.provider} usage={entry} />)}</section>;
}
function Row({ usage }: { usage: ProviderUsage }) {
  const name = usage.provider === "codex" ? "Codex" : "Claude Code"; const logo = usage.provider === "codex" ? codexLogo : claudeLogo;
  if (usage.state !== "available") return <div className="provider-usage-row"><img src={logo} alt="" /><span>{name}</span><strong>{usage.state === "unknown" ? "Unknown" : "Unavailable"}</strong></div>;
  return <div className="provider-usage-row"><img src={logo} alt="" /><span>{name}</span>{usage.windows.map(limit => <div className="provider-limit" key={limit.id}><meter min="0" max="100" value={limit.usedPercent} aria-label={`${name} ${limit.id} usage`}>{limit.usedPercent}%</meter><strong>{limit.usedPercent}% used</strong><small>{limit.id.replace("_", " ")} · resets {limit.resetsAt}{limit.windowDurationMins ? ` · ${limit.windowDurationMins} min` : ""}</small></div>)}</div>;
}
