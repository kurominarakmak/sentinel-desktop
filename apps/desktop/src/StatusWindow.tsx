import type { Phase2AppServices } from "./Phase2App";
import { ProviderUsageWidget } from "./ProviderUsageWidget";
import { V3AttentionWidget } from "./V3AttentionWidget";
import { registerEscapeToHide } from "./escape";
import { useEffect } from "react";
import "./v3-design-system.css";

export function StatusWindow({ services }: { services: Phase2AppServices }) {
  useEffect(() => registerEscapeToHide(window, () => services.hidePrompt()), [services]);
  return <main className="utility-window">
    <V3AttentionWidget />
    <ProviderUsageWidget />
    <button onClick={() => void services.hidePrompt()}>Hide</button>
  </main>;
}
