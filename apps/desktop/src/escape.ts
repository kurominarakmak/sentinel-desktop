type KeydownTarget = Pick<Window, "addEventListener" | "removeEventListener">;

export function registerEscapeToHide(
  target: KeydownTarget,
  hidePrompt: () => Promise<void>,
  reportError: (message: string, error: unknown) => void = console.error,
) {
  const handleKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;

    event.preventDefault();
    event.stopPropagation();
    void hidePrompt().catch((error) => {
      reportError("Agent Sentinel: failed to hide prompt", error);
    });
  };

  target.addEventListener("keydown", handleKeyDown, true);
  return () => target.removeEventListener("keydown", handleKeyDown, true);
}
