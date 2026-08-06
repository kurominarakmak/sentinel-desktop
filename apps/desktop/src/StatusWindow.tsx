import type { Phase2AppServices } from "./Phase2App";
import { ProviderUsageWidget } from "./ProviderUsageWidget";

export function StatusWindow({ services }: { services: Phase2AppServices }) {
  return <main className="utility-window" aria-live="polite">
    <header><strong>Agent Sentinel</strong><span className="state-dot" aria-label="Idle" /></header>
    <p className="utility-title">No active task</p>
    <p className="muted">Open the prompt to start or review a managed task.</p>
    <ProviderUsageWidget />
    <button onClick={() => void services.hidePrompt()}>Open Prompt</button>
  </main>;
}
