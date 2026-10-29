import type { TodoContext, TodoHost } from "./lifecycle.ts";
import { describe, expect, it, vi } from "vitest";
import { TodoPresentation } from "./presentation.ts";
import { replay } from "./lifecycle.ts";

vi.mock(import("./lifecycle.ts"), { spy: true });

/**
 * Creates UI seams for the real replay path against an empty session branch.
 * @returns {TodoContext} Branch reader and UI spies.
 */
const context = (): TodoContext => ({
    mode: "tui",
    sessionManager: { getBranch: () => [] },
    ui: { notify: vi.fn<() => void>(), setWidget: vi.fn<() => void>() },
  }),
  host: TodoHost = {
    getAllTools: () => [
      {
        name: "todo",
        sourceInfo: { path: decodeURIComponent(new URL("index.ts", import.meta.url).pathname) },
      },
    ],
  };

describe("todo disclosure", () => {
  it("toggles reversibly and preserves expansion through task refreshes", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const presentation = new TodoPresentation(),
      ctx = context();
    presentation.refresh(ctx);
    expect(replay).toHaveBeenLastCalledWith(ctx, false);
    presentation.toggle(host, ctx);
    expect(replay).toHaveBeenLastCalledWith(ctx, true);
    presentation.refresh(ctx);
    expect(replay).toHaveBeenLastCalledWith(ctx, true);
    presentation.toggle(host, ctx);
    expect(replay).toHaveBeenLastCalledWith(ctx, false);
  });

  it("collapses on session start and branch navigation", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const presentation = new TodoPresentation(),
      ctx = context();
    presentation.toggle(host, ctx);
    presentation.reset(ctx);
    expect(replay).toHaveBeenLastCalledWith(ctx, false);
    presentation.toggle(host, ctx);
    presentation.start(host, ctx);
    presentation.refresh(ctx);
    expect(replay).toHaveBeenLastCalledWith(ctx, false);
  });

  it("does not touch another todo owner's widget", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const presentation = new TodoPresentation(),
      ctx = context();
    presentation.toggle({ getAllTools: () => [] }, ctx);
    expect(ctx.ui.setWidget).not.toHaveBeenCalled();
    presentation.refresh(ctx);
    expect(replay).toHaveBeenLastCalledWith(ctx, false);
  });
});
