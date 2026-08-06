import { useEffect, useState } from "react";
import { codexUsageAdapter, claudeUsageAdapter, loadProviderUsage, type ProviderUsage } from "./providerUsage";
import codexLogo from "./assets/codex-mark.svg";
import claudeLogo from "./assets/claude-mark.svg";
import "./ProviderUsageWidget.css";

const initialUsage: ProviderUsage[] = [
  { provider: "codex", state: "unknown", detail: "Checking local usage status…" },
  { provider: "claude", state: "unknown", detail: "Checking local usage status…" },
];

export function ProviderUsageWidget() {
  const [usage, setUsage] = useState<ProviderUsage[]>(initialUsage);
  useEffect(() => { void loadProviderUsage().then(setUsage); }, []);
  return <section className="provider-usage" aria-label="Provider usage limits">
    <h2>Provider usage</h2>
    {usage.map((entry) => <ProviderUsageRow key={entry.provider} usage={entry} />)}
  </section>;
}

function ProviderUsageRow({ usage }: { usage: ProviderUsage }) {
  const isCodex = usage.provider === codexUsageAdapter.provider;
  const name = isCodex ? "Codex" : "Claude Code";
  const logo = isCodex ? codexLogo : claudeLogo;
  if (usage.state !== "available") {
    return <div className="provider-usage-row"><img src={logo} alt="" /><span>{name}</span><strong>{usage.state === "unknown" ? "Unknown" : "Unavailable"}</strong></div>;
  }
  return <div className="provider-usage-row"><img src={logo} alt="" /><span>{name}</span><meter min="0" max="100" value={usage.usedPercent} aria-label={`${name} usage`}>{usage.usedPercent}%</meter><strong>{usage.remainingPercent}% remaining</strong><small>{usage.usedPercent}% used</small></div>;
}
