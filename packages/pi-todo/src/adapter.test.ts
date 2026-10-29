import type { SettleEvent, TodoContext, TodoHost } from "./lifecycle.ts";
import { describe, expect, it, vi } from "vitest";
import { replay, settle, show, start } from "./lifecycle.ts";
import { REMINDER_ENTRY } from "./reminder.ts";
import { STATE_ENTRY } from "./state.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { TodoParams } from "./tool.ts";
import { runTodo } from "./tool.ts";
import { widget } from "./wasm.ts";

vi.mock(import("./wasm.ts"), { spy: true });

interface Fixture {
  readonly branch: readonly SessionEntry[];
  readonly append: (entry: SessionEntry) => void;
  readonly rewind: () => void;
  readonly ctx: TodoContext;
  readonly event: SettleEvent;
  readonly host: TodoHost;
  readonly mutate: (params: TodoParams) => void;
}

/**
 * Builds the Pi branch boundary fixture.
 * @param {boolean} collision Whether another extension owns todo.
 * @returns {Fixture} Branch and Pi seams.
 */
const fixture = (collision = false): Fixture => {
    const branch: SessionEntry[] = [],
      ctx: TodoContext = {
        mode: "tui",
        sessionManager: { getBranch: () => branch },
        ui: { notify: vi.fn<() => void>(), setWidget: vi.fn<() => void>() },
      },
      event: SettleEvent = {
        context: { canContinue: true },
        continue: false,
        entries: [],
        outcome: "completed",
      },
      path = collision
        ? "/another/todo.ts"
        : decodeURIComponent(new URL("index.ts", import.meta.url).pathname);
    return {
      append: (entry): void => {
        branch.push(entry);
      },
      branch,
      ctx,
      event,
      host: { getAllTools: () => [{ name: "todo", sourceInfo: { path } }] },
      mutate: (params): void => {
        runTodo(params, {
          append: (customType, data): void => {
            branch.push({
              customType,
              data,
              id: crypto.randomUUID(),
              parentId: null,
              timestamp: "2026-10-16T00:00:00Z",
              type: "custom",
            });
          },
          branch: () => branch,
          sessionId: "adapter-test",
        });
      },
      rewind: (): void => {
        branch.pop();
      },
    };
  },
  /**
   * Adds a persisted reminder to simulate an accepted boundary result.
   * @param {Fixture} test The branch fixture.
   */
  remember = (test: Fixture): void => {
    test.append({
      content: "reminder",
      customType: REMINDER_ENTRY,
      display: false,
      id: crypto.randomUUID(),
      parentId: null,
      timestamp: "2026-10-16T00:00:00Z",
      type: "custom_message",
    });
  };

describe("branch replay", () => {
  it("restores the selected snapshot at start and tree navigation", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.mutate({ items: ["a", "b"], op: "init" });
    start(test.host, test.ctx);
    expect(widget).toHaveBeenLastCalledWith(
      {
        tasks: [
          expect.objectContaining({ content: "a", status: "pending", subtasks: [] }),
          expect.objectContaining({ content: "b", status: "pending", subtasks: [] }),
        ],
      },
      false,
    );
    expect(test.ctx.ui.setWidget).toHaveBeenLastCalledWith("pi-todo", expect.any(Function));
    test.mutate({ op: "drop" });
    replay(test.ctx);
    expect(widget).toHaveBeenLastCalledWith(
      {
        tasks: [
          expect.objectContaining({ content: "a", status: "abandoned", subtasks: [] }),
          expect.objectContaining({ content: "b", status: "abandoned", subtasks: [] }),
        ],
      },
      false,
    );
    expect(test.ctx.ui.setWidget).toHaveBeenLastCalledWith("pi-todo", expect.any(Function));
  });

  it("does not use snapshots from an abandoned branch", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.mutate({ items: ["a"], op: "init" });
    test.mutate({ op: "drop" });
    test.rewind();
    replay(test.ctx);
    show(test.ctx);
    expect(widget).toHaveBeenLastCalledWith(
      { tasks: [expect.objectContaining({ content: "a", status: "pending", subtasks: [] })] },
      false,
    );
    expect(test.ctx.ui.setWidget).toHaveBeenLastCalledWith("pi-todo", expect.any(Function));
    expect(test.ctx.ui.notify).toHaveBeenLastCalledWith(
      expect.stringContaining("Todo list: 0/1 completed\n[ ] a"),
      "info",
    );
  });
});

describe("non-terminal mode", () => {
  it("does not install a custom widget outside the TUI", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.mutate({ items: ["a"], op: "init" });
    replay(Object.assign({}, test.ctx, { mode: "rpc" as const }));
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
  });
});

describe("empty list", () => {
  it("shows no widget while the branch has no tasks", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      { calls } = vi.mocked(test.ctx.ui.setWidget).mock;
    start(test.host, test.ctx);
    test.mutate({ items: ["a"], op: "init" });
    replay(test.ctx);
    test.rewind();
    replay(test.ctx);
    expect(calls).toStrictEqual([
      ["pi-todo", undefined],
      ["pi-todo", expect.any(Function)],
      ["pi-todo", undefined],
    ]);
  });
});

describe("collision", () => {
  it("notifies once at startup and leaves the owner alone", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(true);
    start(test.host, test.ctx);
    expect(test.ctx.ui.notify).toHaveBeenCalledExactlyOnceWith(
      "pi-todo is inactive: todo belongs to /another/todo.ts.",
      "warning",
    );
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
    expect(settle(test.host, test.event, test.ctx)).toStrictEqual({});
  });
});

describe("settlement", () => {
  it("adds a visible reminder and preserves prior drafts", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const prior = { customType: "other", type: "custom" as const },
      test = fixture(),
      withPrior = Object.assign({}, test.event, { entries: [prior] });
    test.mutate({ items: ["a"], op: "init" });
    expect(settle(test.host, withPrior, test.ctx)).toMatchObject({ continue: true });
    expect(settle(test.host, withPrior, test.ctx).entries).toMatchObject([
      prior,
      { customType: REMINDER_ENTRY, display: true, type: "custom_message" },
    ]);
    expect(JSON.stringify(settle(test.host, withPrior, test.ctx))).toContain("Reminder 1/3");
  });

  it.each(["aborted", "error"] as const)(
    "does not continue after %s",
    { timeout: 5000 },
    (outcome) => {
      expect.hasAssertions();
      const test = fixture();
      test.mutate({ items: ["a"], op: "init" });
      expect(settle(test.host, Object.assign({}, test.event, { outcome }), test.ctx)).toStrictEqual(
        {},
      );
    },
  );

  it("makes an assistant-ending context continuable with its message", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.mutate({ items: ["a"], op: "init" });
    expect(
      settle(
        test.host,
        Object.assign({}, test.event, { context: { canContinue: false } }),
        test.ctx,
      ),
    ).toMatchObject({ continue: true });
    expect(
      settle(test.host, Object.assign({}, test.event, { continue: true }), test.ctx),
    ).toStrictEqual({});
  });
});

describe("progress guard", () => {
  it("waits for a todo change, not a view or repeated write", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.mutate({ items: ["a"], op: "init" });
    remember(test);
    test.mutate({ op: "view" });
    expect(settle(test.host, test.event, test.ctx)).toStrictEqual({});
    for (const entry of test.branch.filter((stored) => stored.type === "custom")) {
      test.append(Object.assign({}, entry, { id: "repeated-status" }));
    }
    expect(settle(test.host, test.event, test.ctx)).toStrictEqual({});
  });

  it("re-arms after a change and stays silent for closed work", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.mutate({ items: ["a"], op: "init" });
    remember(test);
    test.mutate({ items: ["b"], op: "append" });
    expect(JSON.stringify(settle(test.host, test.event, test.ctx))).toContain("Reminder 2/3");
    test.mutate({ op: "drop" });
    expect(settle(test.host, test.event, test.ctx)).toStrictEqual({});
  });
});

describe("nested-only todo", () => {
  it(
    "uses custom entries when the outer transcript carries only nestedCalls",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const test = fixture();
      test.mutate({ items: ["nested work"], op: "init" });
      test.append({
        id: "outer",
        message: {
          content: [{ text: "Script completed", type: "text" }],
          isError: false,
          nestedCalls: {
            calls: [{ durationMs: 1, id: "outer/1", name: "todo", status: "ok" }],
            complete: true,
          },
          role: "toolResult",
          timestamp: 0,
          toolCallId: "outer",
          toolName: "codemode",
        },
        parentId: null,
        timestamp: "2026-10-16T00:00:00Z",
        type: "message",
      });
      expect(test.branch).toMatchObject([
        { customType: STATE_ENTRY },
        { message: { toolName: "codemode" } },
      ]);
      expect(JSON.stringify(settle(test.host, test.event, test.ctx))).toContain("nested work");
      show(test.ctx);
      expect(test.ctx.ui.notify).toHaveBeenCalledExactlyOnceWith(
        expect.stringContaining("Todo list: 0/1 completed\n[ ] nested work"),
        "info",
      );
    },
  );
});
