export const FATAL_STARTUP_MESSAGE = "Agent Sentinel could not start";

export function renderFatal(document: Document, root?: HTMLElement | null) {
  const fallback = document.createElement("main");
  fallback.className = "fatal-error"; fallback.setAttribute("role", "alert"); fallback.textContent = FATAL_STARTUP_MESSAGE;
  if (root) root.replaceChildren(fallback); else document.body.append(fallback);
}

export function mountApplication({ document, root, mount }: { document: Document; root: HTMLElement | null; mount: (root: HTMLElement) => void }) {
  if (!root) { renderFatal(document); return; }
  try { mount(root); }
  catch { renderFatal(document, root); }
}
