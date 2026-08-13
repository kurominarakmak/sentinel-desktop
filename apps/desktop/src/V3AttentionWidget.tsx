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

  return <section className="v3-attention" aria-label="V3 attention" aria-live="polite">
    <header><strong>Attention</strong><span className="state-dot" aria-label={stateLabel[state]} /></header>
    <p className="utility-title">{stateLabel[state]}</p>
    {task ? <><p className="muted">{agent} · {task.summary}</p><button onClick={() => void openV3TaskDetail(task.id)}>Open Task Detail</button>
      {state === "working" && <button disabled={busy} onClick={() => void stop()}>Stop</button>}
      {detail?.finalApprovalId && <><button disabled={busy} onClick={() => void decide(true)}>Approve</button><button disabled={busy} onClick={() => void decide(false)}>Reject</button></>}
    </> : <p className="muted">No managed V3 task is active.</p>}
  </section>;
}
