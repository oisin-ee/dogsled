import type { WorkContext, WorkHost } from "./work.ts";
import { describe, expect, it, vi } from "vitest";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { Mock } from "vitest";
import { TodoPresentation } from "./presentation.ts";
import { TodoWork } from "./work.ts";
import { sections } from "./wasm.ts";

type Listener = (value: unknown) => void;
interface Fixture {
  readonly ctx: WorkContext;
  readonly events: Readonly<ExtensionAPI["events"]>;
  readonly opened: Mock<Listener>;
  readonly snapshot: Mock<Listener>;
  readonly work: Readonly<TodoWork>;
  readonly claim: (value?: unknown) => void;
  readonly request: () => void;
  readonly release: () => void;
}

/**
 * Creates a synchronous bus with independent extension listeners.
 * @returns {ExtensionAPI["events"]} Public event bus seam.
 */
const firstArgument = 0,
  oneCall = 1,
  eventBus = (): ExtensionAPI["events"] => {
    const listeners = new Map<string, Set<Listener>>();
    return {
      emit: (name, value): void => {
        for (const listener of listeners.get(name) ?? []) {
          listener(value);
        }
      },
      on: (name, listener): (() => void) => {
        const group = listeners.get(name) ?? new Set<Listener>();
        group.add(listener);
        listeners.set(name, group);
        return (): void => {
          group.delete(listener);
        };
      },
    };
  },
  /**
   * Builds the real provider against a branch-sensitive terminal context.
   * @param {boolean} collision Whether another tool owns todo.
   * @returns {Fixture} Provider and public boundary spies.
   */
  fixture = (collision = false): Fixture => {
    const events = eventBus(),
      host: WorkHost = {
        events,
        getAllTools: () =>
          collision
            ? []
            : [
                {
                  name: "todo",
                  sourceInfo: {
                    path: decodeURIComponent(new URL("index.ts", import.meta.url).pathname),
                  },
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
      snapshot = vi.fn<Listener>(),
      opened = vi.fn<Listener>(),
      envelope = { presenter: "work", request: "request", version: 3 };
    events.on("oisin.todo.snapshot", snapshot);
    events.on("oisin.work.open", opened);
    return {
      claim: (value = snapshot.mock.lastCall?.at(firstArgument)): void => {
        events.emit("oisin.work.claim", value);
      },
      ctx,
      events,
      opened,
      release: (): void => {
        events.emit("oisin.work.release", envelope);
      },
      request: (): void => {
        events.emit("oisin.work.request", envelope);
      },
      snapshot,
      work,
    };
  };

describe("work provider", () => {
  it("publishes validated data without hiding before acknowledgement", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.request();
    expect(test.snapshot).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        branch: null,
        revision: 1,
        session: "session",
        version: 3,
      }),
    );
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
    test.work.stop();
  });

  it("routes the read-only view after claim and restores it on release", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.request();
    test.claim();
    expect(test.work.open()).toBe(true);
    expect(test.opened).toHaveBeenCalledExactlyOnceWith({
      presenter: "work",
      request: "request",
      version: 3,
      view: "todos",
    });
    test.release();
    expect(test.work.open()).toBe(false);
    test.work.stop();
  });

  it("rejects a stale claim and unsubscribes at shutdown", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.request();
    test.claim({ presenter: "old", request: "old", version: 3 });
    expect(test.work.open()).toBe(false);
    test.work.stop();
    test.request();
    expect(test.snapshot).toHaveBeenCalledTimes(oneCall);
  });
});

describe("work protocol v3", () => {
  it("publishes core-owned sections in the snapshot", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.request();
    expect(test.snapshot).toHaveBeenCalledTimes(oneCall);
    expect(test.snapshot.mock.lastCall?.at(firstArgument)).toMatchObject({
      display: { sections: sections({ tasks: [] }) },
      version: 3,
    });
    test.work.stop();
  });

  it("rejects a v2 request like any incompatible version", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.work.start(test.ctx);
    test.events.emit("oisin.work.request", { presenter: "work", request: "request", version: 2 });
    expect(test.snapshot).not.toHaveBeenCalled();
    test.work.stop();
  });
});

describe("provider ownership", () => {
  it("leaves another todo owner alone", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(true);
    test.work.start(test.ctx);
    test.request();
    expect(test.snapshot).not.toHaveBeenCalled();
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
  });

  it("does not publish outside the terminal", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      ctx = Object.assign({}, test.ctx, { mode: "rpc" as const });
    test.work.start(ctx);
    test.request();
    expect(test.snapshot).not.toHaveBeenCalled();
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
  });
});
