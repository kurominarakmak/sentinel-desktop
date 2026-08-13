import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

export type V3Task = { id: string; summary: string; lifecycle: string; recoveryRequired: boolean };
export type V3Detail = { task: V3Task; activity: string[]; worktree: Record<string, unknown> | null; diff: unknown; validations: Array<Record<string, unknown>>; findings: Array<Record<string, unknown>>; repairRounds: Array<Record<string, unknown>>; finalApprovalPacket: unknown; finalApprovalId: string | null; agent: string | null };
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
export function V3Workflow() {
  if (getCurrentWindow().label !== "prompt") return null;
  const [tasks, setTasks] = useState<V3Task[]>([]); const [selected, setSelected] = useState<string>(""); const [detail, setDetail] = useState<V3Detail | null>(null); const [prompt, setPrompt] = useState(""); const [error, setError] = useState(""); const [busy, setBusy] = useState(false);
  const refresh = async (id = selected) => { try { const values = await invoke<V3Task[]>("list_v3_tasks"); setTasks(values); const next = id && values.some((task) => task.id === id) ? id : values[0]?.id ?? ""; setSelected(next); if (next) setDetail(await invoke<V3Detail>("get_v3_task_detail", { request: { taskId: next } })); else setDetail(null); setError(""); } catch { setError("V3 task state is unavailable."); } };
  useEffect(() => { void refresh(); let unlisten: (() => void) | undefined; void listen<string>("v3-state-changed", (event) => { void refresh(event.payload); }).then((dispose) => { unlisten = dispose; }); return () => unlisten?.(); }, []);
  useEffect(() => { let unlisten: (() => void) | undefined; void listen<string>("v3-open-task-detail", (event) => { void refresh(event.payload); }).then((dispose) => { unlisten = dispose; }); return () => unlisten?.(); }, []);
  const send = async () => { if (!prompt.trim() || busy) return; setBusy(true); try { const id = await invoke<string>("start_codex_session_task", { request: { summary: prompt.trim(), prompt: prompt.trim() } }); setPrompt(""); await refresh(id); } catch { setError("Codex task could not be started."); } finally { setBusy(false); } };
  const stop = async () => { if (!selected) return; setBusy(true); try { await invoke("cancel_codex_session_task", { request: { taskId: selected } }); await refresh(selected); } catch { setError("Task stop could not be requested."); } finally { setBusy(false); } };
  const decide = async (approve: boolean) => { if (!detail?.finalApprovalId) return; setBusy(true); try { await invoke("decide_v3_final_approval", { request: { approvalId: detail.finalApprovalId, approve } }); await refresh(selected); } catch { setError("Approval decision could not be recorded."); } finally { setBusy(false); } };
  const state = attentionState(detail?.task ?? tasks.find((task) => task.id === selected));
  return <section aria-label="V3 workflow"><header><strong>Attention</strong><span className="state-dot" aria-label={label(state)} /> {label(state)}</header><label>Quick Prompt (Codex)<textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder="Create and send a V3 task…" rows={2} /></label><button disabled={!prompt.trim() || busy} onClick={() => void send()}>{busy ? "Sending…" : "Send task"}</button>{tasks.length > 0 && <label>Current task<select value={selected} onChange={(event) => void refresh(event.target.value)}>{tasks.map((task) => <option value={task.id} key={task.id}>{task.summary}</option>)}</select></label>}{detail && <article aria-label="Task Detail"><h2>Task Detail</h2><p>{detail.task.summary}</p><p className="muted">{detail.agent?.includes("claude") ? "Claude" : "Codex"} · {detail.task.lifecycle}</p><button disabled={busy || state !== "working"} onClick={() => void stop()}>Stop</button>{detail.finalApprovalId && <p><button disabled={busy} onClick={() => void decide(true)}>Approve</button><button disabled={busy} onClick={() => void decide(false)}>Reject</button></p>}<details open><summary>Activity</summary>{detail.activity.map((item, index) => <p key={`${item}-${index}`}>{item}</p>)}</details><details><summary>Worktree and diff</summary><pre>{JSON.stringify({ worktree: detail.worktree, diff: detail.diff }, null, 2)}</pre></details><details><summary>Validation</summary><pre>{JSON.stringify(detail.validations, null, 2)}</pre></details><details><summary>Findings and repairs</summary><pre>{JSON.stringify({ findings: detail.findings, rounds: detail.repairRounds }, null, 2)}</pre></details>{Boolean(detail.finalApprovalPacket) && <details><summary>Final approval packet</summary><pre>{JSON.stringify(detail.finalApprovalPacket, null, 2)}</pre></details>}</article>}{error && <p role="alert">{error}</p>}</section>;
}
