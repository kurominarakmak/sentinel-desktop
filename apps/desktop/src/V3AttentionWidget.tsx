import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { Window } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import { attentionState, type AttentionState, type V3Detail, type V3Task } from "./V3Workflow";

const stateLabel: Record<AttentionState, string> = {
  working: "Working",
  "approval-required": "Approval required",
  failed: "Needs attention",
  "recovery-required": "Recovery required",
  "ready-for-review": "Ready",
  idle: "No active task",
};

export async function openV3TaskDetail(taskId: string) {
  const prompt = await Window.getByLabel("prompt");
  if (!prompt) return;
  await prompt.show();
  await prompt.unminimize();
  await prompt.setFocus();
  await emit("v3-open-task-detail", taskId);
}

export function V3AttentionWidget() {
  const [tasks, setTasks] = useState<V3Task[]>([]);
  const [detail, setDetail] = useState<V3Detail | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    const values = await invoke<V3Task[]>("list_v3_tasks");
    setTasks(values);
    const current = values[0];
    setDetail(current ? await invoke<V3Detail>("get_v3_task_detail", { request: { taskId: current.id } }) : null);
  };
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void refresh().catch(() => { if (!disposed) setTasks([]); });
    void listen<string>("v3-state-changed", () => { void refresh().catch(() => undefined); }).then((dispose) => { unlisten = dispose; });
    return () => { disposed = true; unlisten?.(); };
  }, []);

  const task = detail?.task ?? tasks[0];
  const state = attentionState(task);
  const agent = detail?.agent?.toLowerCase().includes("claude") ? "Claude" : "Codex";
  const decide = async (approve: boolean) => {
    if (!detail?.finalApprovalId) return;
    setBusy(true);
    try { await invoke("decide_v3_final_approval", { request: { approvalId: detail.finalApprovalId, approve } }); await refresh(); }
    finally { setBusy(false); }
  };
  const stop = async () => {
    if (!task) return;
    setBusy(true);
    try { await invoke("cancel_codex_session_task", { request: { taskId: task.id } }); await refresh(); }
    finally { setBusy(false); }
  };

  const activity = detail?.activity.at(-1)?.kind ?? (state === "working" ? "In progress" : stateLabel[state]);
  const validation = detail?.validations.at(-1)?.state;
  const findings = detail?.findings.filter((finding) => ["blocker", "high"].includes(String(finding.severity))).length ?? 0;
  return <section className="v3-attention" aria-label="V3 attention" aria-live="polite"><header><span><strong>Sentinel attention</strong><small>{agent}</small></span><span className={`v3-status-badge v3-status-${state}`}>{stateLabel[state]}</span></header>{task ? <><p className="v3-attention-task">{task.summary}</p><p className="muted v3-attention-activity">{String(activity).replaceAll("_", " ")}{validation ? ` · validation ${validation}` : ""}{findings ? ` · ${findings} finding${findings === 1 ? "" : "s"}` : ""}</p><div className="v3-attention-actions"><button className="v3-detail-action" onClick={() => void openV3TaskDetail(task.id)}>Open Task Detail</button>{state === "working" && <button disabled={busy} onClick={() => void stop()}>Stop</button>}{detail?.finalApprovalId && <><button className="v3-approve-action" disabled={busy} onClick={() => void decide(true)}>Approve</button><button className="v3-reject-action" disabled={busy} onClick={() => void decide(false)}>Reject</button></>}</div></> : <p className="muted">No managed V3 task is active.</p>}</section>;
}
