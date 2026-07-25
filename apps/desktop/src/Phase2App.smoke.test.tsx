import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { createMockPhase2Services, deferred, renderPhase2App } from "./test/phase2Harness";
describe("Phase2App boundary", () => {
  it("initializes injected services and cleans up once", async () => { const mock = createMockPhase2Services(); const view = renderPhase2App(mock.services); await waitFor(() => expect(mock.api.environment).toHaveBeenCalledOnce()); expect(mock.api.listRuns).toHaveBeenCalledOnce(); expect(mock.services.listenToRunEvents).toHaveBeenCalledOnce(); view.rerender(<></>); expect(mock.unlisten).toHaveBeenCalledOnce(); });
  it("uses injected Escape hide", async () => { const mock = createMockPhase2Services(); renderPhase2App(mock.services); await userEvent.keyboard("{Escape}"); expect(mock.services.hidePrompt).toHaveBeenCalledOnce(); });
  it("cleans a late listener registration", async () => { const mock = createMockPhase2Services(); const late = deferred<() => void>(); mock.services.listenToRunEvents = async () => late.promise; const view = renderPhase2App(mock.services); view.unmount(); const dispose = () => undefined; late.resolve(dispose); await Promise.resolve(); });
});
