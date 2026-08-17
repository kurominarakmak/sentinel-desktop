import { createRoot } from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { type Phase2AppServices } from "./Phase2App";
import { DesktopWindowRouter } from "./DesktopWindowRouter";
import { AppErrorBoundary } from "./AppErrorBoundary";
import { mountApplication } from "./bootstrap";
import { api, type RunEvent } from "./phase2";
import { approvalApi } from "./approval";
import { driftApi } from "./drift";
import { invoke } from "@tauri-apps/api/core";
import { V3Workflow } from "./V3Workflow";
import "./styles.css";

const services: Phase2AppServices = {
  api,
  approvalApi: approvalApi(invoke),
  driftApi: driftApi(invoke),
  async listenToRunEvents(callback) {
    return listen<RunEvent>("phase2-run-event", (event) =>
      callback(event.payload),
    );
  },
  async listenToTaskInputFocus(callback) {
    return listen("focus-task-input", () => callback());
  },
  async hidePrompt() {
    await getCurrentWindow().hide();
  },
};
mountApplication({
  document,
  root: document.getElementById("root"),
  mount(root) {
    createRoot(root).render(
      <AppErrorBoundary>
        <DesktopWindowRouter services={services} />
        <V3Workflow />
      </AppErrorBoundary>,
    );
  },
});
