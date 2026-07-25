import { describe, expect, it, vi } from "vitest";
import { FATAL_STARTUP_MESSAGE, mountApplication } from "./bootstrap";

describe("mountApplication", () => {
  it("mounts once with a valid root", () => {
    const root = document.createElement("div"); const mount = vi.fn((target: HTMLElement) => { target.textContent = "mounted"; });
    mountApplication({ document, root, mount }); expect(mount).toHaveBeenCalledOnce(); expect(root).toHaveTextContent("mounted"); expect(document.body).not.toHaveTextContent(FATAL_STARTUP_MESSAGE);
  });
  it("renders a safe visible fallback when the root is missing", () => {
    const mount = vi.fn(); mountApplication({ document, root: null, mount });
    const fallback = document.body.querySelector('[role="alert"]'); expect(mount).not.toHaveBeenCalled(); expect(fallback).toHaveTextContent(FATAL_STARTUP_MESSAGE); expect(fallback).not.toHaveTextContent(/SQL|stderr|Users/);
  });
  it("renders a safe fallback once when mounting throws", () => {
    const root = document.createElement("div"); const mount = vi.fn(() => { throw new Error("/private/path: SQL stderr executable"); });
    mountApplication({ document, root, mount }); const fallback = root.querySelector('[role="alert"]');
    expect(mount).toHaveBeenCalledOnce(); expect(fallback).toHaveTextContent(FATAL_STARTUP_MESSAGE); expect(fallback).not.toHaveTextContent(/private\/path|SQL|stderr|executable/);
  });
});
