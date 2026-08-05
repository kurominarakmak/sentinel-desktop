import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { driftApi } from "./drift";
type DriftApi = ReturnType<typeof driftApi>;
export function DriftPanel({ api = driftApi(invoke), projectId, worktreeId }: { api?: DriftApi; projectId: string; worktreeId: string }) {
  const [result, setResult] = useState<Awaited<ReturnType<DriftApi["evaluate"]>> | null>(null); const [message, setMessage] = useState(""); const [loading, setLoading] = useState(false); const generation = useRef(0);
  const evaluate = async () => { if (!api || !projectId || !worktreeId || loading) return; const current = ++generation.current; setLoading(true); setMessage(""); try { const value = await api.evaluate({ projectId, worktreeId }); if (current === generation.current) setResult(value); } catch { if (current === generation.current) setMessage("Drift Guardian is unavailable."); } finally { if (current === generation.current) setLoading(false); } };
  return <section aria-live="polite"><h2>Drift Guardian</h2><p>Deterministic managed-state findings only; no repository changes are made.</p><button onClick={() => void evaluate()} disabled={!api || !projectId || !worktreeId || loading}>{loading ? "Evaluating…" : "Evaluate drift"}</button>{message && <p role="alert">{message}</p>}{result && <><p>Snapshot {result.snapshotFingerprint.slice(0, 16)}</p>{result.findings.length ? <ol>{result.findings.map((finding) => <li key={finding.fingerprint}><strong>{finding.severity} · {finding.ruleId} v{finding.ruleVersion}</strong><p>{finding.title}</p><p>Expected: {finding.expected}</p><p>Observed: {finding.observed}</p><p>{finding.explanation}</p></li>)}</ol> : <p>No deterministic drift findings.</p>}</>}</section>;
}
