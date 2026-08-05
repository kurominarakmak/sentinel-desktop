import type { Invoke } from "./phase2";

/** Phase 7 public approval contract. References are opaque and must always be
 * supplied with their project/worktree/task ownership tuple. */
export type ApprovalProfile = "safe" | "balanced" | "autonomous" | "custom";
export type ApprovalState = "pending" | "allowed_once" | "allowed_for_task" | "denied" | "hard_denied";
export type ApprovalDecision = "allow_once" | "allow_for_task" | "deny";
export type ApprovalCapability = { available: boolean; hardDeniedCategories: readonly string[] };
export type ApprovalRequestDto = {
  approvalReference: string;
  profile: ApprovalProfile;
  actionCategory: string;
  summary: string;
  state: ApprovalState;
  version: number;
  decisionAvailable: boolean;
};
/** A runtime reference is optional only for persisted control-plane records
 * created without an adapter context; when supplied, adapter and reference
 * must be supplied together and are still opaque. */
export type ApprovalOwner = { projectId: string; worktreeId: string; taskKey: string; adapter?: "codex" | "claude_code"; runtimeReference?: string };
export type ApprovalLookup = ApprovalOwner & { approvalReference: string };
export type ApprovalDecisionInput = ApprovalLookup & { expectedVersion: number; decision: ApprovalDecision };
export type ApprovalCommandError = { code: string; message: string };

const command = async <T>(invoke: Invoke, name: string, args?: Record<string, unknown>): Promise<T> => {
  try { return await invoke<T>(name, args); }
  catch (value) {
    const error = value as Partial<ApprovalCommandError>;
    throw { code: typeof error.code === "string" ? error.code : "approval_error", message: typeof error.message === "string" ? error.message : "The approval request could not be completed." } satisfies ApprovalCommandError;
  }
};
export const approvalApi = (invoke: Invoke) => ({
  capability: () => command<ApprovalCapability>(invoke, "approval_capability"),
  listPending: (owner: ApprovalOwner) => command<ApprovalRequestDto[]>(invoke, "list_pending_approvals", { request: owner }),
  query: (request: ApprovalLookup) => command<ApprovalRequestDto>(invoke, "query_approval_request", { request }),
  decide: (request: ApprovalDecisionInput) => command<ApprovalRequestDto>(invoke, "decide_approval_request", { request }),
});
export const approvalStateText = (state: ApprovalState): string => ({ pending: "Pending decision", allowed_once: "Approved once", allowed_for_task: "Approved for this task", denied: "Denied", hard_denied: "Denied by policy" })[state];
