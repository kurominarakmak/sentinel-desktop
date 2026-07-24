import { Component, type ErrorInfo, type ReactNode, useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { registerEscapeToHide } from "./escape";
import { statusFor } from "./status";
import "./styles.css";

type Agent = "Fake Agent" | "Codex" | "Claude Code";
type Event = { type: string; phase?: string; text?: string; command?: string; exit_code?: number; path?: string; error?: string };
type Diagnostics = { fake: string; codex: string; claude_code: string };

type ErrorBoundaryState = { error: Error | null };

class PromptErrorBoundary extends Component<{ children: ReactNode }, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Agent Sentinel prompt failed to render", error, info);
  }

  render() {
    if (this.state.error) {
      return <main className="fatal-error" role="alert"><h1>Agent Sentinel could not start</h1><p>The prompt UI failed to render. Check the developer console for details.</p></main>;
    }
    return this.props.children;
  }
}

function App() {
  const input = useRef<HTMLTextAreaElement>(null);
  const [projectPath, setProjectPath] = useState("");
  const [agent, setAgent] = useState<Agent>("Fake Agent");
  const [task, setTask] = useState("");
  const [status, setStatus] = useState("Idle");
  const [events, setEvents] = useState<Event[]>([]);
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);

  useEffect(() => {
    input.current?.focus();
    const unlistenAgent = listen<Event>("agent-event", ({ payload }) => { setEvents((items) => [...items, payload]); setStatus(statusFor(payload)); });
    const unlistenFocus = listen("focus-task-input", () => input.current?.focus());
    void invoke<Diagnostics>("spike_diagnostics").then(setDiagnostics);
    return () => { void unlistenAgent.then((dispose) => dispose()); void unlistenFocus.then((dispose) => dispose()); };
  }, []);

  useEffect(() => registerEscapeToHide(window, () => getCurrentWindow().hide()), []);

  async function run() {
    if (!task.trim()) return;
    setEvents([]); setStatus("Running");
    if (agent === "Fake Agent") await invoke("run_fake_agent");
    else await invoke("record_manual_probe_request", { agent });
    setTask(""); await getCurrentWindow().hide();
  }

  return <main onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void run(); } }}>
    <header><strong>Agent Sentinel</strong><span className={`status status-${status.toLowerCase()}`}>Spike status: {status}</span></header>
    <label>Project path<input value={projectPath} onChange={(event) => setProjectPath(event.target.value)} placeholder="/path/to/repository" /></label>
    <label>Agent<select value={agent} onChange={(event) => setAgent(event.target.value as Agent)}><option>Fake Agent</option><option>Codex</option><option>Claude Code</option></select></label>
    <label>Task<textarea ref={input} value={task} onChange={(event) => setTask(event.target.value)} placeholder="Describe a small feasibility task" rows={5} /></label>
    <button onClick={() => void run()} disabled={!task.trim()}>Run</button>
    <section><h2>Environment</h2><p>Fake: {diagnostics?.fake ?? "Checking..."}</p><p>Codex: {diagnostics?.codex ?? "Checking..."}</p><p>Claude Code: {diagnostics?.claude_code ?? "Checking..."}</p></section>
    <section aria-live="polite"><h2>Live events</h2><ol>{events.map((event, index) => <li key={index}>{event.type}: {event.phase ?? event.text ?? event.command ?? event.path ?? event.error ?? ""}</li>)}</ol></section>
  </main>;
}

function renderBootstrapError(root: HTMLElement, error: unknown) {
  console.error("Agent Sentinel prompt initialization failed", error);
  root.replaceChildren();
  const screen = document.createElement("main");
  screen.className = "fatal-error";
  screen.setAttribute("role", "alert");
  const heading = document.createElement("h1");
  heading.textContent = "Agent Sentinel could not start";
  const message = document.createElement("p");
  message.textContent = "The prompt UI failed to initialize. Check the developer console for details.";
  screen.append(heading, message);
  root.append(screen);
}

const rootElement = document.getElementById("root");
if (!rootElement) {
  const fallbackRoot = document.createElement("div");
  document.body.append(fallbackRoot);
  renderBootstrapError(fallbackRoot, new Error("Agent Sentinel requires a #root element"));
} else {
  try {
    createRoot(rootElement).render(<PromptErrorBoundary><App /></PromptErrorBoundary>);
  } catch (error) {
    renderBootstrapError(rootElement, error);
  }
}
