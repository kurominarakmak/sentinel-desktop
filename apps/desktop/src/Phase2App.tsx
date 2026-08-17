import { useEffect, useRef, useState } from "react";
import { registerEscapeToHide } from "./escape";
import type { approvalApi } from "./approval";
import type { driftApi } from "./drift";
import { formatEventSummary, isRunLifecycleEvent, isScenario, isTerminalRunEvent, MAX_TASK_BYTES, mergeRunEvents, patchRunInHistory, reconcile, scenarios, upsertRunInHistory, type CancellationResult, type DesktopCapabilities, type ProjectDto, type RunDto, type RunEvent, type RuntimeEnvironment, type Scenario, type Unlisten, type WorktreeDto } from "./phase2";
import type { api as productionApi } from "./phase2";

export interface Phase2AppServices {
  api: typeof productionApi;
  approvalApi?: ReturnType<typeof approvalApi>;
  driftApi?: ReturnType<typeof driftApi>;
  listenToRunEvents(callback: (event: RunEvent) => void): Promise<Unlisten>;
  listenToTaskInputFocus(callback: () => void): Promise<Unlisten>;
  hidePrompt(): Promise<void>;
}
const EVENTS_ERROR = "Event history could not be loaded. Retry.";
const METADATA_ERROR = "Run details could not be refreshed. Retry.";
const HISTORY_ERROR = "Run history could not be refreshed. Retry.";
const cancellationMessage = (result: CancellationResult) => {
  if (result === "cancellation_requested") return "Cancellation was requested.";
  if (result === "already_cancelling") return "Cancellation is already in progress.";
  if (result === "already_terminal") return "This run has already finished.";
  if (result === "run_not_active") return "This run is no longer active in this desktop session.";
  if (result === "termination_failed") return "Cancellation could not be completed.";
  return undefined;
};
export type CancellationOperation = { requestId: number; pending: boolean; feedback: string };
export type CancellationOperations = Record<string, CancellationOperation>;
export function updateCancellationOperationForRequest(operations: CancellationOperations, runId: string, requestId: number, update: Partial<CancellationOperation>): CancellationOperations {
  const current = operations[runId];
  return !current || current.requestId !== requestId ? operations : { ...operations, [runId]: { ...current, ...update } };
}
function focusTaskInput(input: HTMLTextAreaElement | null) {
  if (!input) return;
  try { input.focus({ preventScroll: true }); }
  catch { input.focus(); }
}
export function Phase2App({ services }: { services: Phase2AppServices }) {
  const input = useRef<HTMLTextAreaElement>(null); const [task, setTask] = useState(""); const [scenario, setScenario] = useState<Scenario>("success"); const [agent, setAgent] = useState<"fake" | "codex" | "claude_code">("fake"); const [projects, setProjects] = useState<ProjectDto[]>([]); const [worktrees, setWorktrees] = useState<WorktreeDto[]>([]); const [projectId, setProjectId] = useState(""); const [worktreeId, setWorktreeId] = useState(""); const [desktopCapabilities, setDesktopCapabilities] = useState<DesktopCapabilities | null>(null);
  const [runs, setRuns] = useState<RunDto[]>([]); const [selected, setSelected] = useState<RunDto | null>(null); const [events, setEvents] = useState<RunEvent[]>([]); const [environment, setEnvironment] = useState<RuntimeEnvironment | null>(null);
  const [submissionError, setSubmissionError] = useState(""); const [cancellationOperations, setCancellationOperations] = useState<CancellationOperations>({}); const [environmentError, setEnvironmentError] = useState(""); const [liveUpdatesError, setLiveUpdatesError] = useState(""); const [historyError, setHistoryError] = useState(""); const [selectedEventsError, setSelectedEventsError] = useState(""); const [selectedMetadataError, setSelectedMetadataError] = useState(""); const [selectedRunLoading, setSelectedRunLoading] = useState(false); const [submitting, setSubmitting] = useState(false); const [environmentLoading, setEnvironmentLoading] = useState(false); const selectedId = useRef<string | null>(null); const selectionRequest = useRef(0); const historyRequest = useRef(0); const environmentRequest = useRef(0); const metadataGeneration = useRef(new Map<string, number>()); const latestLifecycleSequence = useRef(new Map<string, number>()); const terminalSequence = useRef(new Map<string, number>()); const cancellationOperationRef = useRef<CancellationOperations>({}); const mounted = useRef(false);
  const refreshRuns = async () => {
    const request = ++historyRequest.current;
    try {
      const loaded = await services.api.listRuns();
      if (request !== historyRequest.current) return false;
      setRuns(loaded); setHistoryError("");
      return true;
    } catch {
      if (request === historyRequest.current) setHistoryError(HISTORY_ERROR);
      return false;
    }
  };
  const refreshEnvironment = async () => {
    const request = ++environmentRequest.current; setEnvironmentLoading(true); setEnvironmentError("");
    try { const loaded = await services.api.environment(); if (request === environmentRequest.current) setEnvironment(loaded); }
    catch { if (request === environmentRequest.current) { setEnvironment(null); setEnvironmentError("Backend is unavailable."); } }
    finally { if (request === environmentRequest.current) setEnvironmentLoading(false); }
  };
  const refreshProjects = async () => { try { const loaded = await services.api.listProjects(); if (!mounted.current) return; setProjects(loaded); setProjectId((current) => loaded.some((item) => item.id === current) ? current : (loaded[0]?.id ?? "")); } catch { if (mounted.current) setProjects([]); } };
  const refreshWorktrees = async (id: string) => { if (!id) { setWorktrees([]); setWorktreeId(""); return; } try { const loaded = await services.api.listProjectWorktrees(id); if (!mounted.current) return; const ready = loaded.filter((item) => item.state === "ready"); setWorktrees(ready); setWorktreeId((current) => ready.some((item) => item.id === current) ? current : (ready[0]?.id ?? "")); } catch { if (mounted.current) { setWorktrees([]); setWorktreeId(""); } } };
  const beginMetadataRefresh = (runId: string) => { const generation = (metadataGeneration.current.get(runId) ?? 0) + 1; metadataGeneration.current.set(runId, generation); return generation; };
  const metadataIsCurrent = (runId: string, generation: number, lifecycleSequence?: number) => metadataGeneration.current.get(runId) === generation && (lifecycleSequence === undefined || latestLifecycleSequence.current.get(runId) === lifecycleSequence);
  const selectionIsCurrent = (runId: string, request: number) => selectedId.current === runId && selectionRequest.current === request;
  const setCancellationOperation = (runId: string, operation: CancellationOperation) => {
    cancellationOperationRef.current = { ...cancellationOperationRef.current, [runId]: operation };
    if (mounted.current) setCancellationOperations(cancellationOperationRef.current);
  };
  const updateCancellationOperation = (runId: string, requestId: number, update: Partial<CancellationOperation>) => {
    const current = cancellationOperationRef.current[runId];
    if (!current || current.requestId !== requestId) return false;
    setCancellationOperation(runId, updateCancellationOperationForRequest(cancellationOperationRef.current, runId, requestId, update)[runId]);
    return true;
  };
  const selectRun = async (runId: string, initialRun?: RunDto) => {
    // Background refreshes must never change the user's current selection.
    const request = ++selectionRequest.current; const generation = beginMetadataRefresh(runId); selectedId.current = runId; setSelected(initialRun ?? null); setEvents([]); setSelectedEventsError(""); setSelectedMetadataError(""); setSelectedRunLoading(true);
    try { const loaded = await services.api.listRunEvents(runId); if (selectionIsCurrent(runId, request)) setEvents((current) => mergeRunEvents(runId, current, loaded)); }
    catch { if (selectionIsCurrent(runId, request)) setSelectedEventsError(EVENTS_ERROR); }
    try { const latest = await services.api.getRun(runId); if (selectionIsCurrent(runId, request) && metadataIsCurrent(runId, generation)) { setSelected(latest); setRuns((current) => patchRunInHistory(current, latest)); } }
    catch { if (selectionIsCurrent(runId, request)) setSelectedMetadataError(METADATA_ERROR); }
    finally { if (selectionIsCurrent(runId, request)) setSelectedRunLoading(false); }
  };
  const refreshRunInPlace = async (runId: string, options: { refreshEvents?: boolean; refreshRecentRuns?: boolean; lifecycleSequence?: number; metadataGeneration?: number; reportErrors?: boolean } = {}) => {
    const request = selectedId.current === runId ? selectionRequest.current : undefined;
    const generation = options.metadataGeneration ?? beginMetadataRefresh(runId);
    const selectedRequestIsCurrent = () => request !== undefined && selectionIsCurrent(runId, request);
    if (options.refreshEvents !== false) {
      try { const loaded = await services.api.listRunEvents(runId); if (selectedRequestIsCurrent()) setEvents((current) => mergeRunEvents(runId, current, loaded)); }
      catch { if (options.reportErrors !== false && selectedRequestIsCurrent()) setSelectedEventsError(EVENTS_ERROR); }
    }
    if (options.refreshRecentRuns) await refreshRuns();
    try { const latest = await services.api.getRun(runId); if (!metadataIsCurrent(runId, generation, options.lifecycleSequence)) return; setRuns((current) => patchRunInHistory(current, latest)); if (selectedRequestIsCurrent()) setSelected(latest); }
    catch { if (options.reportErrors !== false && selectedRequestIsCurrent()) setSelectedMetadataError(METADATA_ERROR); }
  };
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { let disposed = false; let cleanup: Unlisten | undefined; input.current?.focus(); void refreshEnvironment(); void refreshRuns(); void refreshProjects(); const desktopApi = services.api as Partial<typeof services.api>; if (typeof desktopApi.desktopCapabilities === "function") void desktopApi.desktopCapabilities().then((value) => { if (!disposed) setDesktopCapabilities(value); }).catch(() => { if (!disposed) setDesktopCapabilities(null); }); const listener = services.listenToRunEvents((payload) => { if (selectedId.current === payload.run_id) setEvents((items) => reconcile(items, payload)); if (!isRunLifecycleEvent(payload.event_type)) return; const previous = latestLifecycleSequence.current.get(payload.run_id); if (previous !== undefined && payload.sequence_number <= previous) return; latestLifecycleSequence.current.set(payload.run_id, payload.sequence_number); const generation = beginMetadataRefresh(payload.run_id); if (isTerminalRunEvent(payload.event_type)) { if (terminalSequence.current.get(payload.run_id) === payload.sequence_number) return; terminalSequence.current.set(payload.run_id, payload.sequence_number); void refreshRunInPlace(payload.run_id, { refreshRecentRuns: true, lifecycleSequence: payload.sequence_number, metadataGeneration: generation }).catch(() => undefined); return; } void refreshRunInPlace(payload.run_id, { refreshEvents: false, lifecycleSequence: payload.sequence_number, metadataGeneration: generation }).catch(() => undefined); }); void listener.then((dispose) => { cleanup = dispose; if (disposed) dispose(); }).catch(() => { if (!disposed) setLiveUpdatesError("Live updates are unavailable."); }); return () => { disposed = true; environmentRequest.current += 1; cleanup?.(); }; }, []);
  useEffect(() => { void refreshWorktrees(projectId); }, [projectId]);
  useEffect(() => { let disposed = false; let cleanup: Unlisten | undefined; const listener = services.listenToTaskInputFocus(() => focusTaskInput(input.current)); void listener.then((dispose) => { cleanup = dispose; if (disposed) dispose(); }).catch(() => undefined); return () => { disposed = true; cleanup?.(); }; }, []);
  useEffect(() => registerEscapeToHide(window, () => services.hidePrompt()), []);
  async function run() { const trimmed = task.trim(); if (!trimmed) { setSubmissionError("Enter a task before starting a run."); return; } if (new TextEncoder().encode(trimmed).length > MAX_TASK_BYTES) { setSubmissionError("Task is too long."); return; } if (!isScenario(scenario)) { setSubmissionError("Select a supported scenario."); return; } if (submitting || !environment?.fake_agent_available) return; setSubmitting(true); setSubmissionError(""); let run: RunDto; try { run = await services.api.submitFakeRun(trimmed, scenario); } catch { setSubmissionError("The run could not be submitted."); setSubmitting(false); return; } historyRequest.current += 1; setRuns((current) => upsertRunInHistory(current, run)); setTask(""); await selectRun(run.id, run); void refreshRuns(); setSubmitting(false); }
  async function cancel() { if (!selected || !selected.cancellable) return; const runId = selected.id; const previous = cancellationOperationRef.current[runId]; if (previous?.pending) return; const requestId = (previous?.requestId ?? 0) + 1; setCancellationOperation(runId, { requestId, pending: true, feedback: "" }); try { const result = await services.api.cancelRun(runId); updateCancellationOperation(runId, requestId, { feedback: cancellationMessage(result) ?? "Cancellation could not be requested." }); if (mounted.current) await refreshRunInPlace(runId); } catch { updateCancellationOperation(runId, requestId, { feedback: "Cancellation could not be requested." }); } finally { updateCancellationOperation(runId, requestId, { pending: false }); } }
  async function retrySelectedRecovery() { if (!selected) return; const runId = selected.id; const request = selectionRequest.current; setSelectedEventsError(""); setSelectedMetadataError(""); setSelectedRunLoading(true); await refreshRunInPlace(runId); if (selectionIsCurrent(runId, request)) setSelectedRunLoading(false); }
  const selectedCancellation = selected ? cancellationOperations[selected.id] : undefined;
  const fakeSelected = agent === "fake";
  return <main className="prompt-window" onKeyDown={(event) => { if (event.key === "Enter" && event.metaKey && !event.nativeEvent.isComposing) { event.preventDefault(); void run(); } }}><header><strong>Agent Sentinel</strong><span className="state-dot" aria-label="Ready" /></header><section><label>Project<select value={projectId} onChange={(event) => setProjectId(event.target.value)}>{projects.length ? projects.map((item) => <option key={item.id} value={item.id}>{item.display_name}</option>) : <option value="">Open a project or choose a folder.</option>}</select></label>{projectId && <label>Worktree<select value={worktreeId} onChange={(event) => setWorktreeId(event.target.value)}>{worktrees.map((item) => <option key={item.id} value={item.id}>Managed worktree</option>)}</select></label>}<div className="segmented" role="group" aria-label="Agent">{(["fake", "codex", "claude_code"] as const).map((value) => <button key={value} aria-pressed={agent === value} onClick={() => setAgent(value)}>{value === "fake" ? "Fake" : value === "codex" ? "Codex" : "Claude"}</button>)}</div><label>Prompt<textarea ref={input} value={task} placeholder="Describe the task…" onChange={(event) => setTask(event.target.value)} rows={3} /></label><button onClick={() => void run()} disabled={!task.trim() || submitting || !environment?.fake_agent_available || !fakeSelected}>{submitting ? "Running…" : fakeSelected ? "Run fake task" : "Selected agent unavailable"}</button>{submissionError && <p role="alert">{submissionError}</p>}</section><footer className="muted">{fakeSelected ? "Fake Agent is available for development only." : "Select an available agent to run a task."}</footer></main>;
}
