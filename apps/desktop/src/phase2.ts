import { invoke } from "@tauri-apps/api/core";

export const scenarios = ["success", "failure", "failure-exit-zero", "delayed", "malformed", "partial", "cancellation-child", "stderr-secrets", "burst"] as const;
export const MAX_TASK_BYTES = 8_000;
export type Scenario = typeof scenarios[number];
export type RunStatus = "queued" | "preparing" | "running" | "cancelling" | "cancelled" | "completed" | "failed";
/** Exact stable JSON shape returned by submit_fake_run, get_run, and list_runs. */
export type RunDto = { id: string; task_text: string; agent_kind: "fake" | "codex" | "claude_code"; status: RunStatus; schema_version: number; created_at_ms: number; started_at_ms: number | null; finished_at_ms: number | null; exit_code: number | null; error_category: string | null; error_message: string | null; cancellable: boolean };
export type CancellationResult = "cancellation_requested" | "already_terminal" | "already_cancelling" | "run_not_active" | "termination_failed";
export type RunEvent = { schema_version: number; run_id: string; sequence_number: number; event_type: string; timestamp_ms: number; payload: unknown };
export type RuntimeEnvironment = { schema_version: number; database_initialized: boolean; fake_agent_available: boolean };
export type CommandError = { code: string; message: string };
export type Invoke = <T>(name: string, args?: Record<string, unknown>) => Promise<T>;
export type Unlisten = () => void;
export type Listen = <T>(event: string, handler: (payload: T) => void) => Promise<Unlisten>;
export function isScenario(value: string): value is Scenario { return (scenarios as readonly string[]).includes(value); }
export function validateTask(value: string): string | null {
  if (!value.trim()) return "Enter a task before starting a run.";
  return new TextEncoder().encode(value.trim()).length > MAX_TASK_BYTES ? "Task is too long." : null;
}
export function statusText(value: string): string {
  return ["queued", "preparing", "running", "cancelling", "cancelled", "completed", "failed"].includes(value) ? value : `Unknown status: ${value.slice(0, 80)}`;
}
export function isRunLifecycleEvent(eventType: string): boolean {
  return ["run_preparing", "run_running", "run_cancelling", "run_cancelled", "run_completed", "run_failed"].includes(eventType);
}
export function isTerminalRunEvent(eventType: string): boolean {
  return ["run_cancelled", "run_completed", "run_failed"].includes(eventType);
}
const command = async <T>(call: Invoke, name: string, args?: Record<string, unknown>): Promise<T> => {
  try { return await call<T>(name, args); }
  catch (value) { const error = value as Partial<CommandError>; throw { code: typeof error.code === "string" ? error.code : "phase2_error", message: typeof error.message === "string" ? error.message : "The request could not be completed." } satisfies CommandError; }
};
export function createApi(call: Invoke) { return {
  submitFakeRun: (task_text: string, scenario: Scenario) => command<RunDto>(call, "submit_fake_run", { request: { task_text, scenario } }),
  listRuns: () => command<RunDto[]>(call, "list_runs"), getRun: (id: string) => command<RunDto>(call, "get_run", { id }),
  listRunEvents: (id: string) => command<RunEvent[]>(call, "list_run_events", { id }),
  cancelRun: (id: string) => command<CancellationResult>(call, "cancel_run", { id }),
  environment: () => command<RuntimeEnvironment>(call, "get_runtime_environment"),
}; }
export const api = createApi(invoke);
export function reconcile(events: RunEvent[], incoming: RunEvent): RunEvent[] {
  if (events.length && events[0].run_id !== incoming.run_id) return events;
  if (events.some((event) => event.sequence_number === incoming.sequence_number)) return events;
  return [...events, incoming].sort((a, b) => a.sequence_number - b.sequence_number);
}
/** Persisted records win identical-sequence conflicts because SQLite is authoritative. */
export function mergeRunEvents(runId: string, current: RunEvent[], persisted: RunEvent[]): RunEvent[] {
  const merged = new Map<number, RunEvent>();
  for (const event of current) if (event.run_id === runId) merged.set(event.sequence_number, event);
  for (const event of persisted) if (event.run_id === runId) merged.set(event.sequence_number, event);
  return [...merged.values()].sort((left, right) => left.sequence_number - right.sequence_number);
}
/** Replaces one history row without changing the order of the list. */
export function patchRunInHistory(runs: RunDto[], updatedRun: RunDto): RunDto[] {
  return runs.map((run) => run.id === updatedRun.id ? updatedRun : run);
}
/** Replaces an existing row or prepends a newly submitted recent run. */
export function upsertRunInHistory(runs: RunDto[], updatedRun: RunDto): RunDto[] {
  return runs.some((run) => run.id === updatedRun.id)
    ? patchRunInHistory(runs, updatedRun)
    : [updatedRun, ...runs];
}
const MAX_EVENT_SUMMARY_CHARACTERS = 240;
const MAX_EVENT_INSPECTION_CHARACTERS = 8_000;
const UNAVAILABLE_MESSAGE = "Run message unavailable";
const UNSAFE_INSPECTION_CHARACTERS = /[\u0000-\u001F\u007F-\u009F\u200B-\u200F\u202A-\u202E\u2060\u2066-\u2069\uFEFF]/g;
const isRecord = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value);
const scalar = (payload: unknown, field: string): string | null => isRecord(payload) && Object.prototype.hasOwnProperty.call(payload, field) && typeof payload[field] === "string" ? payload[field] : null;
const numberField = (payload: unknown, field: string): number | null => isRecord(payload) && Object.prototype.hasOwnProperty.call(payload, field) && typeof payload[field] === "number" && Number.isFinite(payload[field]) ? payload[field] : null;
/** Removes invisible separators for security classification without changing the displayed text. */
const securityInspectionText = (value: string) => value.normalize("NFKC").replace(UNSAFE_INSPECTION_CHARACTERS, "").replace(/\\/g, "/").toLowerCase();
/** Checks a security-normalized copy only; event identity and persisted payloads remain untouched. */
export function containsSensitiveAbsolutePath(value: string): boolean {
  const normalized = securityInspectionText(value);
  const boundary = "(?:^|[\\s\\\"'`([{:=])";
  const windowsDrivePath = new RegExp(`${boundary}[a-z]:/+`);
  const uncPath = new RegExp(`${boundary}//[^/\\s]+/[^/\\s]+(?:/[^\\s,;]+)?`);
  const unixSensitiveRoot = new RegExp(boundary + "/(?:users|home|usr|private|etc|var|opt|tmp|applications|library|volumes|root|mnt|srv|bin|sbin)(?=/|[\\s\\\"'`),.;:!?]|$)");
  const unixAbsolutePath = new RegExp(`${boundary}/(?:[^/\\s]+/)+[^\\s,;]+`);
  return windowsDrivePath.test(normalized) || uncPath.test(normalized) || unixSensitiveRoot.test(normalized) || unixAbsolutePath.test(normalized);
}
/** Identifies database statements and engine diagnostics before visible truncation. */
export function containsDatabaseDiagnostic(value: string): boolean {
  const trimmed = value.trim();
  const statement = /^(?:sql(?:ite|ite3)?\s*(?:query|statement)?\s*[:=-]\s*)?(?:pragma|vacuum|attach|detach|analyze|reindex|explain|select|insert|update|delete|replace|create|alter|drop|with|begin|commit|rollback|savepoint|release)\b/i;
  const labelledStatement = /\b(?:sql|sqlite(?:3)?)\s*(?:query|statement|command)?\s*[:=-]\s*(?:pragma|vacuum|attach|detach|analyze|reindex|explain|select|insert|update|delete|replace|create|alter|drop|with|begin|commit|rollback|savepoint|release)\b/i;
  const diagnostic = /\b(?:sqlite(?:3)?\s*(?:error|exception)?\s*:|database\s+(?:is\s+(?:locked|busy)|disk image is malformed|schema has changed)|unable to open database file|no such (?:table|column)|(?:foreign key |unique |not null )?constraint failed|disk i\/o error|malformed database schema|sql syntax error|near\s+["'][^"']+["']\s*:\s*syntax error|transaction is closed|cannot (?:start a transaction|commit|rollback))\b/i;
  return statement.test(trimmed) || labelledStatement.test(trimmed) || diagnostic.test(trimmed);
}
/** Sanitizes only documented user-visible message text; diagnostics never fall back to raw text. */
export function sanitizeEventText(value: string): string {
  if (Array.from(value).length > MAX_EVENT_INSPECTION_CHARACTERS) return UNAVAILABLE_MESSAGE;
  const inspection = securityInspectionText(value);
  if (containsSensitiveAbsolutePath(inspection) || containsDatabaseDiagnostic(inspection) || /\bstderr\s*:|\b(?:stack trace|traceback|thread ['\"]main['\"] panicked|panicked at|failed to execute|command not found|permission denied)\b|\bat\s+.+\(.+:\d+:\d+\)/i.test(inspection)) return UNAVAILABLE_MESSAGE;
  const normalized = value.replace(/[\u0000-\u001F\u007F-\u009F]/g, " ").replace(/\s+/g, " ").trim();
  if (!normalized) return "Message unavailable";
  const redacted = normalized
    .replace(/(?:authorization\s*:\s*bearer\s+|bearer\s+|api[_-]?key\s*=\s*|(?:password|token|secret)\s*[=:]\s*)[^\s,;]+/gi, "[REDACTED]")
    .replace(/(?:\/Users\/[^\s,;]+|\/home\/[^\s,;]+|[A-Za-z]:\\Users\\[^\s,;]+)/g, "[PATH REDACTED]");
  const characters = Array.from(redacted);
  return characters.length > MAX_EVENT_SUMMARY_CHARACTERS ? `${characters.slice(0, MAX_EVENT_SUMMARY_CHARACTERS).join("")}…` : redacted;
}
/** Safe, allowlisted display summary; it never serializes arbitrary event payloads. */
export function formatEventSummary(event: RunEvent): string {
  const lifecycle: Record<string, string> = { run_preparing: "Preparing", run_running: "Running", run_cancelling: "Cancelling", run_cancelled: "Cancelled", run_completed: "Completed", run_failed: "Failed" };
  if (event.event_type in lifecycle) return lifecycle[event.event_type];
  if (event.event_type === "message") return sanitizeEventText(scalar(event.payload, "text") ?? "");
  if (event.event_type === "phase_changed") { const phase = scalar(event.payload, "phase"); return phase ? `Phase: ${sanitizeEventText(phase)}` : "Phase changed"; }
  if (event.event_type === "command_completed") { const exitCode = numberField(event.payload, "exit_code"); return exitCode === null ? "Command completed" : `Command completed (exit code ${Math.trunc(exitCode)})`; }
  if (event.event_type === "session_started") return "Session started";
  if (event.event_type === "command_started") return "Command started";
  if (event.event_type === "file_changed") return "File changed";
  if (event.event_type === "waiting_for_input") return "Waiting for input";
  if (event.event_type === "agent_completed") return "Agent completed";
  if (event.event_type === "agent_failed" || event.event_type === "parser_error") return "Internal event";
  return "Additional run information is unavailable";
}
