import type { Invoke } from "./phase2";
export type EvidenceDecision = "passed" | "failed" | "unavailable";
export type EvidenceGateResult = { fingerprint: string; gateId: "EG001"; gateVersion: number; decision: EvidenceDecision; title: string; reason: string; limitation: string };
export type EvidenceEvaluation = { bundleFingerprint: string; results: EvidenceGateResult[] };
export type EvidenceRequest = { projectId: string; worktreeId: string };
const call = async <T>(invoke: Invoke, command: string, args?: Record<string, unknown>): Promise<T> => {
  try { return await invoke<T>(command, args); } catch { throw new Error("Evidence Gate is unavailable."); }
};
export const evidenceApi = (invoke: Invoke) => ({ capability: () => call<boolean>(invoke, "evidence_gate_capability"), evaluate: (request: EvidenceRequest) => call<EvidenceEvaluation>(invoke, "evaluate_evidence_gate", { request }) });
