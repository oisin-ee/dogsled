import {
  DELTA_ENTRY,
  STATE_ENTRY,
  branchList,
  branchSnapshot,
  latestSnapshot,
  saveState,
} from "./state.ts";
import { changes, count, intermediateBranch, replayBranch } from "./delta-review-fixture.ts";
import { describe, expect, it, vi } from "vitest";
import { entry, initial, taskAt } from "./delta-fixture.ts";
import { first, firstId } from "./contract-fixture.ts";
import { reminder, reminderFacts } from "./reminder.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { TodoDelta } from "./state.ts";
import type { TodoList } from "./wasm.ts";
import { leafTask } from "./delta.ts";
import { protect } from "./wasm.ts";

const one = 1,
  twice = 2,
  cloneBound = 3,
  readers = [
    latestSnapshot,
    branchSnapshot,
    (branch: readonly SessionEntry[]): unknown => reminderFacts(branch, []),
  ],
  errorFor = (
    read: (branch: readonly SessionEntry[]) => unknown,
    branch: readonly SessionEntry[],
  ): string => {
    try {
      read(branch);
      return "accepted";
    } catch (error: unknown) {
      return String(error);
    }
  };

describe("eager task and order validation", () => {
  it(
    "rejects a malformed intermediate task identically even after repair",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const task = leafTask(taskAt(initial(one))),
        branch = intermediateBranch(Object.assign({}, task, { status: "lost" })),
        errors = readers.map((read) => errorFor(read, branch));
      expect(errors.at(first)).toContain("unknown variant `lost`");
      expect(new Set(errors).size).toBe(one);
    },
  );

  it("defers a repaired cross-task violation for every reader", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const task = taskAt(initial(one)),
      unit = Object.assign({}, task.unit, { draft: { dependsOn: ["missing-prerequisite"] } }),
      branch = intermediateBranch(Object.assign({}, leafTask(task), { unit }));
    expect(taskAt(latestSnapshot(branch)).unit?.draft.dependsOn).toStrictEqual([]);
    expect(branchSnapshot(branch).tasks).toHaveLength(count);
    expect(reminder(branch, [])).toContain("Reminder 1/3");
    for (const read of readers) {
      expect(() => read(branch.slice(first, twice))).toThrow("UNIT_NOT_FOUND");
    }
  });

  it(
    "rejects an invalid intermediate order before a later full snapshot",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const list = initial(count),
        branch = [
          entry("base", STATE_ENTRY, list),
          entry("bad", DELTA_ENTRY, {
            changed: [],
            orders: [{ after: "missing", id: firstId(list), parent: null }],
            removed: [],
            version: one,
          }),
          entry("fixed", STATE_ENTRY, list),
        ];
      for (const read of readers) {
        expect(() => read(branch)).toThrow("INVALID_TODO_DELTA: invalid order predecessor");
      }
    },
  );
});

describe("fence authorization", () => {
  it("checks the intended change before appending even the fence", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const branch = [entry("base", STATE_ENTRY, initial(count))],
      before = branchSnapshot(branch),
      after = structuredClone(before),
      lease = {
        contractEntryId: "base",
        revision: one,
        sessionId: "test-session",
        unitId: firstId(before),
      },
      session = {
        append: (customType: string, data: TodoList | TodoDelta, list: TodoList): void => {
          protect(before, list, [lease]);
          branch.push(entry("persist", customType, data));
        },
        branch: (): SessionEntry[] => branch,
      };
    taskAt(after).status = "completed";
    expect(() => {
      saveState(before, after, session);
    }).toThrow("UNIT_ALREADY_BOUND");
    expect(branch).toHaveLength(one);
  });
});

describe("bounded reminder replay", () => {
  it("clones O(changes + tasks), not every historical list", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const branch = replayBranch(),
      clone = vi.spyOn(globalThis, "structuredClone");
    try {
      Array.from(branchList(branch), (frame) => frame.list);
      expect(clone).toHaveBeenCalledTimes(count + changes + count * changes);
      clone.mockClear();
      reminderFacts(branch, []);
      expect(clone).toHaveBeenCalledTimes(count * twice + changes);
      expect(clone.mock.calls.length).toBeLessThanOrEqual(cloneBound * (count + changes));
    } finally {
      clone.mockRestore();
    }
  });
});
