import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import "./v3-design-system.css";
import { V3TaskDetail } from "./V3TaskDetail";

export type V3Task = { id: string; summary: string; lifecycle: string; recoveryRequired: boolean; recoveryReason?: string | null };
export type V3Detail = { task: V3Task; activity: Array<Record<string, unknown>>; worktree: Record<string, unknown> | null; diff: Record<string, unknown> | null; validations: Array<Record<string, unknown>>; findings: Array<Record<string, unknown>>; repairRounds: Array<Record<string, unknown>>; finalApprovalPacket: Record<string, unknown> | null; finalApprovalId: string | null; agent: string | null };
export type AttentionState = "working" | "approval-required" | "failed" | "recovery-required" | "ready-for-review" | "idle";
export function attentionState(task: V3Task | undefined): AttentionState {
  if (!task) return "idle";
  if (task.recoveryRequired || task.lifecycle === "recovering") return "recovery-required";
  if (["failed", "blocked", "cancelled"].includes(task.lifecycle)) return "failed";
  if (task.lifecycle === "awaitingapproval" || task.lifecycle === "awaiting_approval") return "approval-required";
  if (["readyforhuman", "ready_for_human", "reviewing"].includes(task.lifecycle)) return "ready-for-review";
  return "working";
}
const label = (state: AttentionState) => ({ working: "Working", "approval-required": "Approval required", failed: "Needs attention", "recovery-required": "Recovery required", "ready-for-review": "Ready for review", idle: "No active task" })[state];
export function activeTaskId(tasks: V3Task[], selected = "") { if (selected && tasks.some((task) => task.id === selected)) return selected; return tasks.find((task) => task.recoveryRequired || !["completed", "cancelled", "failed"].includes(task.lifecycle))?.id ?? tasks[0]?.id ?? ""; }
export function V3Workflow() {
  if (getCurrentWindow().label !== "prompt") return null;
  const [tasks, setTasks] = useState<V3Task[]>([]); const [selected, setSelected] = useState<string>(""); const [detail, setDetail] = useState<V3Detail | null>(null); const [prompt, setPrompt] = useState(""); const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  const refresh = async (id = selected) => { try { const values = await invoke<V3Task[]>("list_v3_tasks"); setTasks(values); const next = activeTaskId(values, id); setSelected(next); if (next) setDetail(await invoke<V3Detail>("get_v3_task_detail", { request: { taskId: next } })); else setDetail(null); setError(""); } catch { setError("V3 task state is unavailable."); } };
  useEffect(() => { void refresh(); let unlisten: (() => void) | undefined; void listen<string>("v3-state-changed", (event) => { void refresh(event.payload); }).then((dispose) => { unlisten = dispose; }); return () => unlisten?.(); }, []);
  useEffect(() => { let unlisten: (() => void) | undefined; void listen<string>("v3-open-task-detail", (event) => { void refresh(event.payload); }).then((dispose) => { unlisten = dispose; }); return () => unlisten?.(); }, []);
  const send = async () => { if (!prompt.trim() || busy) return; setBusy(true); try { const id = await invoke<string>("start_codex_session_task", { request: { summary: prompt.trim(), prompt: prompt.trim() } }); setPrompt(""); await refresh(id); } catch { setError("Codex task could not be started."); } finally { setBusy(false); } };
  const stop = async () => { if (!selected) return; setBusy(true); try { await invoke("cancel_codex_session_task", { request: { taskId: selected } }); await refresh(selected); } catch { setError("Task stop could not be requested."); } finally { setBusy(false); } };
  const decide = async (approve: boolean) => { if (!detail?.finalApprovalId) return; setBusy(true); try { await invoke("decide_v3_final_approval", { request: { approvalId: detail.finalApprovalId, approve } }); await refresh(selected); } catch { setError("Approval decision could not be recorded."); } finally { setBusy(false); } };
  const state = attentionState(detail?.task ?? tasks.find((task) => task.id === selected));
  return <section className="v3-workflow" aria-label="V3 workflow"><div className="v3-prompt-shell"><header><span><strong>Quick Prompt</strong><small>Sentinel · Codex</small></span><span className="v3-status-badge">{label(state)}</span></header><label className="v3-prompt-input">What should Codex work on?<textarea autoFocus value={prompt} onChange={(event) => setPrompt(event.target.value)} onKeyDown={(event) => { if ((event.metaKey || event.ctrlKey) && event.key === "Enter") { event.preventDefault(); void send(); } }} placeholder="Describe a focused task…" rows={3} /></label><div className="v3-prompt-actions"><span className="muted">⌘↵ to send</span><button className="v3-primary-action" disabled={!prompt.trim() || busy} onClick={() => void send()}>{busy ? "Sending…" : "Send to Codex"}</button></div>{error && <p role="alert">{error}</p>}</div>{tasks.length > 0 && <label className="v3-task-picker">Active task<select value={selected} onChange={(event) => void refresh(event.target.value)}>{tasks.map((task) => <option value={task.id} key={task.id}>{task.summary}</option>)}</select></label>}{detail && <V3TaskDetail detail={detail} state={state} busy={busy} onStop={() => void stop()} onDecision={(approve) => void decide(approve)} />}</section>;
}
