import { childId, contract, first, firstId, fixture } from "./contract-fixture.ts";
import { describe, expect, it } from "vitest";
import type { Fixture } from "./contract-fixture.ts";

/**
 * Closes a real executable leaf or reviewed parent before testing append.
 * @param {boolean} parent Whether to create an outcome with a child.
 * @param {"done" | "drop"} op Closure operation.
 * @returns {Fixture} Persisted closed work with runtime identity.
 */
const closed = (parent: boolean, op: "done" | "drop"): Fixture => {
  const test = fixture(),
    leaf = { content: "Build", contract: contract(), key: "build" },
    list = test.call({
      items: [parent ? { content: "Outcome", subtasks: [leaf] } : leaf],
      op: "init",
      start: "build",
    }),
    task = firstId(list);
  if (parent) {
    const child = childId(list, first);
    test.call(
      op === "done" ? { evidence: "Acceptance passed", op, task: child } : { op, task: child },
    );
  }
  test.call(op === "done" ? { evidence: "Acceptance passed", op, task } : { op, task });
  return test;
};

describe("closed migrated roots", () => {
  it.each([
    { op: "done", parent: false },
    { op: "done", parent: true },
    { op: "drop", parent: false },
    { op: "drop", parent: true },
  ] as const)("rejects append after $op, parent=$parent", { timeout: 5000 }, ({ op, parent }) => {
    expect.hasAssertions();
    const test = closed(parent, op),
      before = test.call({ op: "view" }),
      entries = [...test.entries];
    expect(() =>
      test.call({ items: ["New scope"], op: "append", parent: firstId(before) }),
    ).toThrow("UNIT_CLOSED");
    expect(test.entries).toStrictEqual(entries);
    expect(test.call({ op: "view" })).toStrictEqual(before);
  });
});

describe("closed legacy roots", () => {
  it.each([
    { status: "completed", subtasks: [] },
    { status: "completed", subtasks: [{ content: "Child", status: "completed" }] },
    { status: "abandoned", subtasks: [] },
    { status: "abandoned", subtasks: [{ content: "Child", status: "abandoned" }] },
  ] as const)(
    "rejects append without persisting migration %j",
    { timeout: 5000 },
    ({ status, subtasks }) => {
      expect.hasAssertions();
      const test = fixture([{ tasks: [{ content: "Closed", status, subtasks }] }]),
        before = test.call({ op: "view" }),
        entries = [...test.entries];
      expect(() => test.call({ items: ["New scope"], op: "append", parent: "Closed" })).toThrow(
        "UNIT_CLOSED",
      );
      expect(test.entries).toStrictEqual(entries);
      expect(test.call({ op: "view" })).toStrictEqual(before);
    },
  );
});
