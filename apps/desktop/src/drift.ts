import type { Invoke } from "./phase2";
export type DriftSeverity = "warning" | "error";
export type DriftFinding = { fingerprint: string; ruleId: "DG001" | "DG002"; ruleVersion: number; severity: DriftSeverity; title: string; expected: string; observed: string; explanation: string };
export type DriftEvaluation = { snapshotFingerprint: string; findings: DriftFinding[] };
export type DriftRequest = { projectId: string; worktreeId: string };
const call = async <T>(invoke: Invoke, name: string, args?: Record<string, unknown>): Promise<T> => {
  try { return await invoke<T>(name, args); } catch { throw new Error("Drift Guardian is unavailable."); }
};
export const driftApi = (invoke: Invoke) => ({ capability: () => call<boolean>(invoke, "drift_guardian_capability"), evaluate: (request: DriftRequest) => call<DriftEvaluation>(invoke, "evaluate_drift_guardian", { request }) });
