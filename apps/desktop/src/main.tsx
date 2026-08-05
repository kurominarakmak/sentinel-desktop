import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Phase2App, type Phase2AppServices } from "./Phase2App";
import { AppErrorBoundary } from "./AppErrorBoundary";
import { mountApplication } from "./bootstrap";
import { api, type RunEvent } from "./phase2";
import { approvalApi } from "./approval";
import { driftApi } from "./drift";
import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

const services: Phase2AppServices = { api, approvalApi: approvalApi(invoke), driftApi: driftApi(invoke), async listenToRunEvents(callback) { return listen<RunEvent>("phase2-run-event", (event) => callback(event.payload)); }, async listenToTaskInputFocus(callback) { return listen("focus-task-input", () => callback()); }, async hidePrompt() { await getCurrentWindow().hide(); } };
mountApplication({ document, root: document.getElementById("root"), mount(root) { createRoot(root).render(<AppErrorBoundary><Phase2App services={services} /></AppErrorBoundary>); } });
