import { describe, expect, it, vi } from "vitest";
import { registerEscapeToHide } from "./escape";

class FakeWindow {
  handlers = new Set<(event: KeyboardEvent) => void>();

  addEventListener(_type: string, handler: EventListenerOrEventListenerObject, capture?: boolean) {
    expect(capture).toBe(true);
    this.handlers.add(handler as (event: KeyboardEvent) => void);
  }

  removeEventListener(_type: string, handler: EventListenerOrEventListenerObject, capture?: boolean) {
    expect(capture).toBe(true);
    this.handlers.delete(handler as (event: KeyboardEvent) => void);
  }

  dispatch(event: KeyboardEvent) {
    for (const handler of this.handlers) handler(event);
  }
}

function keyEvent(key: string, target = "TEXTAREA") {
  return {
    key,
    target: { tagName: target },
    preventDefault: vi.fn(),
    stopPropagation: vi.fn(),
  } as unknown as KeyboardEvent;
}

describe("registerEscapeToHide", () => {
  it("hides the prompt for Escape from a focused textarea", async () => {
    const target = new FakeWindow();
    const hide = vi.fn().mockResolvedValue(undefined);
    const event = keyEvent("Escape");
    registerEscapeToHide(target as unknown as Window, hide);

    target.dispatch(event);
    await Promise.resolve();

    expect(hide).toHaveBeenCalledOnce();
    expect(event.preventDefault).toHaveBeenCalledOnce();
    expect(event.stopPropagation).toHaveBeenCalledOnce();
  });

  it("ignores non-Escape keys", async () => {
    const target = new FakeWindow();
    const hide = vi.fn().mockResolvedValue(undefined);
    const event = keyEvent("Enter");
    registerEscapeToHide(target as unknown as Window, hide);

    target.dispatch(event);
    await Promise.resolve();

    expect(hide).not.toHaveBeenCalled();
    expect(event.preventDefault).not.toHaveBeenCalled();
  });

  it("removes the capture listener before a replacement registration", () => {
    const target = new FakeWindow();
    const hide = vi.fn().mockResolvedValue(undefined);
    const cleanup = registerEscapeToHide(target as unknown as Window, hide);
    cleanup();
    registerEscapeToHide(target as unknown as Window, hide);

    expect(target.handlers).toHaveLength(1);
  });
});
