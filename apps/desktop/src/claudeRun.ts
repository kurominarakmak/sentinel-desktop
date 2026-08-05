import type { OpaqueCodexRunReference, CodexRunStatus } from "./codexRun";

/** Phase 5 public shape; the session token stays private to Rust. */
export interface ClaudeRunDto {
  publicReference: OpaqueCodexRunReference;
  status: CodexRunStatus;
  progressSummary: string | null;
  terminalSummary: string | null;
  errorCategory: string | null;
  cancellationAvailable: boolean;
  followupAvailable: boolean;
}

export interface ClaudeRunStartRequest { projectId: string; worktreeId: string; taskKey: string; prompt: string; }
export interface ClaudeRunLookupRequest { projectId: string; worktreeId: string; taskKey: string; publicReference: OpaqueCodexRunReference; }
export interface ClaudeFollowupRequest extends ClaudeRunLookupRequest { prompt: string; }

export const claudeRunTerminal = (status: CodexRunStatus) => status === "cancelled" || status === "succeeded" || status === "failed";
