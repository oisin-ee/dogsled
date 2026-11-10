import { DELTA_ENTRY, STATE_ENTRY, branchSnapshot, latestSnapshot, saveState } from "./state.ts";
import { count, oldRuntime, oldShow } from "./delta-review-fixture.ts";
import { describe, expect, it, vi } from "vitest";
import { entry, initial } from "./delta-fixture.ts";
import { firstId, fixture } from "./contract-fixture.ts";
import { latestSnapshot as oldLatest, branchSnapshot as oldSnapshot } from "./old-state-fixture.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { reminder as oldReminder } from "./old-reminder-fixture.ts";
import { runTodo as oldTool } from "./old-tool-fixture.ts";
import { reminderFacts } from "./reminder.ts";

const format = "todo-delta/1",
  last = -1,
  one = 1,
  twice = 2,
  cloneBound = 3,
  readers = [
    latestSnapshot,
    branchSnapshot,
    (branch: readonly SessionEntry[]): unknown => reminderFacts(branch, []),
  ];

describe("delta downgrade fence", () => {
  it("fences the first delta with the current list", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const caller = fixture([initial(count)]),
      before = branchSnapshot(caller.entries);
    caller.call({ description: "First delta", op: "update", task: firstId(before) });
    expect(caller.entries).toHaveLength(cloneBound);
    expect(caller.entries.at(-twice)).toMatchObject({
      customType: STATE_ENTRY,
      data: { format, tasks: before.tasks },
    });
    expect(caller.entries.at(last)).toMatchObject({ customType: DELTA_ENTRY });
    expect(() => oldSnapshot(caller.entries)).toThrow("unknown field `format`");
  });

  it("marks every later full snapshot without adding another fence", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const caller = fixture([Object.assign({}, initial(count), { format })]),
      current = branchSnapshot(caller.entries),
      replacement = { tasks: [] };
    saveState(current, replacement, caller.session);
    expect(caller.entries).toHaveLength(twice);
    expect(caller.entries.at(last)).toMatchObject({
      customType: STATE_ENTRY,
      data: { format, tasks: [] },
    });
    expect(latestSnapshot(caller.entries)).toStrictEqual(replacement);
    expect(oldLatest(caller.entries)).toMatchObject({ format });
  });

  it("treats a fence as a snapshot and rejects an old writer after it", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const base = entry("base", STATE_ENTRY, initial(count)),
      fence = entry("fence", STATE_ENTRY, Object.assign({}, initial(one), { format })),
      branch = [base, fence];
    expect(latestSnapshot(branch)).toStrictEqual(initial(one));
    for (const read of readers) {
      expect(() => read([...branch, entry("old", STATE_ENTRY, initial(count))])).toThrow(
        "INVALID_TODO_DELTA: unfenced snapshot after delta history",
      );
    }
    expect(oldSnapshot([base])).toStrictEqual(branchSnapshot([base]));
  });
});

describe("retained a479b93 list consumers", () => {
  it("refuses the actual latestSnapshot consumer after a full fallback", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const branch = [entry("fallback", STATE_ENTRY, { format, tasks: [] })],
      notify = vi.fn<() => void>(),
      context = {
        mode: "tui" as const,
        sessionManager: { getBranch: (): SessionEntry[] => branch },
        ui: { notify, setWidget: vi.fn<() => void>() },
      };
    expect(() => {
      oldShow(context);
    }).toThrow("unknown field `format`");
    expect(notify).not.toHaveBeenCalled();
  });

  it("makes actual readers and every tool operation fail before append", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const branch = [entry("fence", STATE_ENTRY, Object.assign({}, initial(count), { format }))],
      append = vi.fn<() => void>();
    expect(() => oldSnapshot(branch)).toThrow("unknown field `format`");
    expect(() => oldReminder(branch, [])).toThrow("unknown field `format`");
    for (const op of ["view", "init", "append", "start", "done", "drop", "update"] as const) {
      expect(() => oldTool({ op }, { append, branch: () => branch, sessionId: "session" })).toThrow(
        "unknown field `format`",
      );
    }
    expect(append).not.toHaveBeenCalled();
  });
});

describe("retained a479b93 provider capability", () => {
  it(
    "refuses provider reads, claim and settlement without appending",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const { runtime, provider } = await oldRuntime();
      try {
        runtime.manager.appendCustomEntry(
          STATE_ENTRY,
          Object.assign({}, initial(count), { format }),
        );
        const { length } = runtime.manager.getEntries(),
          capability = provider.capability(),
          results = [
            capability.active(),
            capability.imported(),
            capability.inspect("id"),
            capability.claim({}),
            capability.settle({}),
          ];
        for (const result of results) {
          expect(result.ok).toBe(false);
          expect(JSON.stringify(result)).toContain("unknown field `format`");
        }
        expect(runtime.manager.getEntries()).toHaveLength(length);
      } finally {
        runtime.session.dispose();
      }
    },
  );
});

describe("retained a479b93 provider startup and tool", () => {
  it(
    "refuses provider import and tool mutations without appending",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const { runtime, provider, context } = await oldRuntime();
      try {
        runtime.manager.appendCustomEntry(
          STATE_ENTRY,
          Object.assign({}, initial(count), { format }),
        );
        const { length } = runtime.manager.getEntries();
        expect(() => {
          provider.import("{}", "git@github.com:owner/repo.git", context);
        }).toThrow("unknown field `format`");
        expect(() => provider.run({ items: ["Old writer"], op: "append" }, context)).toThrow(
          "unknown field `format`",
        );
        expect(runtime.manager.getEntries()).toHaveLength(length);
      } finally {
        runtime.session.dispose();
      }
    },
  );
});
