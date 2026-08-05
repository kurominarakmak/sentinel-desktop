/** Phase 4 public bridge contract.  These strings are opaque identifiers, not
 * database IDs and never contain a path, PID, argv, environment, or prompt. */
export type OpaqueCodexRunReference = string & { readonly __opaqueCodexRunReference: unique symbol };
export type CodexRunStatus = "created" | "starting" | "running" | "cancelling" | "cancelled" | "succeeded" | "failed";

export interface CodexRunDto {
  publicReference: OpaqueCodexRunReference;
  status: CodexRunStatus;
  progressSummary: string | null;
  terminalSummary: string | null;
  errorCategory: string | null;
  cancellationAvailable: boolean;
}

export interface CodexRunStartRequest {
  projectId: string;
  worktreeId: string;
  taskKey: string;
  prompt: string;
}

export interface CodexRunLookupRequest {
  projectId: string;
  worktreeId: string;
  taskKey: string;
  publicReference: OpaqueCodexRunReference;
}

export const codexRunTerminal = (status: CodexRunStatus): boolean =>
  status === "cancelled" || status === "succeeded" || status === "failed";

/** The App Server is deliberately unavailable/experimental in Phase 4. */
export const codexAppServerAvailable = false;
