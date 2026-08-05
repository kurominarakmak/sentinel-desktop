import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { evidenceApi } from "./evidence";
type EvidenceApi = ReturnType<typeof evidenceApi>;
export function EvidenceGatePanel({ api = evidenceApi(invoke), projectId, worktreeId }: { api?: EvidenceApi; projectId: string; worktreeId: string }) {
  const [result, setResult] = useState<Awaited<ReturnType<EvidenceApi["evaluate"]>> | null>(null); const [message, setMessage] = useState(""); const [loading, setLoading] = useState(false); const generation = useRef(0);
  const evaluate = async () => { if (!projectId || !worktreeId || loading) return; const current = ++generation.current; setLoading(true); setMessage(""); try { const value = await api.evaluate({ projectId, worktreeId }); if (generation.current === current) setResult(value); } catch { if (generation.current === current) setMessage("Evidence Gate is unavailable."); } finally { if (generation.current === current) setLoading(false); } };
  return <section aria-live="polite"><h2>Evidence Gate</h2><p>Read-only baseline evidence; an agent claim alone cannot establish completion.</p><button onClick={() => void evaluate()} disabled={!projectId || !worktreeId || loading}>{loading ? "Evaluating…" : "Evaluate evidence"}</button>{message && <p role="alert">{message}</p>}{result && <><p>Evidence bundle {result.bundleFingerprint.slice(0, 16)}</p><ol>{result.results.map(item => <li key={item.fingerprint}><strong>{item.decision} · {item.gateId} v{item.gateVersion}</strong><p>{item.title}</p><p>{item.reason}</p><p>{item.limitation}</p></li>)}</ol></>}</section>;
}
