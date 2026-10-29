import type { TodoPresenter, WorkContext, WorkHost } from "./work.ts";
import { describe, expect, it, vi } from "vitest";
import { TodoPresentation } from "./presentation.ts";
import { TodoWork } from "./work.ts";
import { sections } from "./wasm.ts";

const firstArgument = 0,
  oneCall = 1,
  /**
   * Builds the real provider against a branch-sensitive terminal context.
   * @param {boolean} collision Whether another tool owns todo.
   * @returns {object} Provider and direct presenter spies.
   */
  fixture = (
    collision = false,
  ): {
    ctx: WorkContext;
    presenter: TodoPresenter;
    work: Readonly<TodoWork>;
    snapshot: ReturnType<typeof vi.fn<TodoPresenter["show"]>>;
    opened: ReturnType<typeof vi.fn<() => void>>;
    stopped: ReturnType<typeof vi.fn<() => void>>;
  } => {
    const host: WorkHost = {
        getAllTools: () =>
          collision
            ? []
            : [
                {
                  name: "todo",
                  sourceInfo: { path: new URL("index.ts", import.meta.url).pathname },
                },
              ],
      },
      ctx: WorkContext = {
        mode: "tui",
        sessionManager: {
          getBranch: () => [],
          getLeafId: () => null,
          getSessionId: () => "session",
        },
        ui: { notify: vi.fn<() => void>(), setWidget: vi.fn<() => void>() },
      },
      work = new TodoWork(host, new TodoPresentation()),
      snapshot = vi.fn<TodoPresenter["show"]>(() => false),
      opened = vi.fn<() => void>(),
      stopped = vi.fn<() => void>();
    return {
      ctx,
      opened,
      presenter: { open: opened, show: snapshot, stop: stopped },
      snapshot,
      stopped,
      work,
    };
  };

describe("work publication", () => {
  it("publishes validated data without hiding before acknowledgement", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.work.attach(test.presenter);
    expect(test.snapshot).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ branch: null, revision: 1, session: "session" }),
    );
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
    test.work.stop();
  });
});

describe("work claims", () => {
  it("routes the read-only view after claim and restores it on release", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.snapshot.mockReturnValue(true);
    test.work.start(test.ctx);
    const detach = test.work.attach(test.presenter);
    expect(test.work.open()).toBe(true);
    expect(test.opened).toHaveBeenCalledExactlyOnceWith();
    detach();
    expect(test.work.open()).toBe(false);
    test.work.stop();
  });

  it("stops the presenter and releases its claim at shutdown", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.snapshot.mockReturnValue(true);
    test.work.start(test.ctx);
    test.work.attach(test.presenter);
    expect(test.work.open()).toBe(true);
    test.work.stop();
    expect(test.stopped).toHaveBeenCalledExactlyOnceWith();
    expect(test.work.open()).toBe(false);
  });

  it("does not publish after shutdown", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.work.attach(test.presenter);
    test.work.stop();
    test.work.publish(test.ctx);
    expect(test.snapshot).toHaveBeenCalledTimes(oneCall);
  });
});

describe("work lifecycle", () => {
  it("publishes when the provider starts after attachment", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.attach(test.presenter);
    expect(test.snapshot).not.toHaveBeenCalled();
    test.work.start(test.ctx);
    expect(test.snapshot).toHaveBeenCalledTimes(oneCall);
    test.work.stop();
  });

  it("releases a claimed view when the presenter rejects an update", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.snapshot.mockReturnValue(true);
    test.work.start(test.ctx);
    test.work.attach(test.presenter);
    expect(test.work.open()).toBe(true);
    test.snapshot.mockReturnValue(false);
    test.work.publish(test.ctx);
    expect(test.work.open()).toBe(false);
    test.work.stop();
  });
});

describe("work snapshots", () => {
  it("publishes core-owned sections in the snapshot", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.work.attach(test.presenter);
    expect(test.snapshot).toHaveBeenCalledTimes(oneCall);
    expect(test.snapshot.mock.lastCall?.at(firstArgument)).toMatchObject({
      display: { sections: sections({ tasks: [] }) },
    });
    test.work.stop();
  });
});

describe("provider ownership", () => {
  it("leaves another todo owner alone", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(true);
    test.work.start(test.ctx);
    test.work.attach(test.presenter);
    expect(test.snapshot).not.toHaveBeenCalled();
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
  });

  it("does not publish outside the terminal", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      ctx = Object.assign({}, test.ctx, { mode: "rpc" as const });
    test.work.start(ctx);
    test.work.attach(test.presenter);
    expect(test.snapshot).not.toHaveBeenCalled();
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
  });
});
