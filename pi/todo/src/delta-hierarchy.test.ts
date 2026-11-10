import { DELTA_ENTRY, branchSnapshot, stateChange } from "./state.ts";
import { childId, contract, firstId } from "./contract-fixture.ts";
import { describe, expect, it } from "vitest";
import { Replay } from "./delta-fixture.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { TodoDelta } from "./state.ts";
import { apply } from "./wasm.ts";

const children = 120,
  roots = 100,
  childIndex = 50,
  fourth = 3,
  fifth = 4,
  one = 1,
  two = 2,
  last = -1,
  setup = (): Replay => {
    const replay = new Replay(),
      { list } = apply(
        { tasks: [] },
        {
          items: [
            {
              content: "Parent",
              subtasks: Array.from({ length: children }, (_unused, index) => ({
                content: `Child ${index}`,
                contract: contract(),
              })),
            },
            ...Array.from({ length: roots }, (_unused, index) => ({
              content: `Root ${index}`,
              contract: contract(),
            })),
          ],
          op: "init",
        },
        { mutationId: "children", sessionId: "test-session" },
      );
    replay.save(list, true);
    replay.call({
      description: "One changed child",
      op: "update",
      task: childId(list, childIndex),
    });
    return replay;
  },
  reorder = (
    replay: Readonly<Pick<Replay, "save">> & { readonly branch: readonly SessionEntry[] },
  ): TodoDelta => {
    const moved = branchSnapshot(replay.branch),
      parent = moved.tasks.shift(),
      child = parent?.subtasks?.shift();
    if (parent === undefined || child === undefined) {
      throw new Error("Missing hierarchy");
    }
    parent.subtasks?.push(child);
    moved.tasks.push(parent);
    const move = stateChange(branchSnapshot(replay.branch), moved);
    replay.save(moved);
    if (move.customType !== DELTA_ENTRY) {
      throw new Error("Expected order-only delta");
    }
    return move.data;
  },
  appendAndRemove = (
    replay: Readonly<Pick<Replay, "save" | "call">> & { readonly branch: readonly SessionEntry[] },
  ): void => {
    const list = branchSnapshot(replay.branch),
      parent = firstId({ tasks: list.tasks.slice(last) });
    list.tasks.splice(fourth, one);
    list.tasks.at(last)?.subtasks?.splice(fifth, one);
    replay.save(list);
    replay.call({ items: ["Added root"], op: "append" });
    replay.call({ items: ["Added child"], op: "append", parent });
  };

describe("hierarchical delta order", () => {
  it("changes one child and moves only the changed links", { timeout: 10_000 }, () => {
    expect.hasAssertions();
    const replay = setup(),
      childEntry = replay.branch.at(last),
      move = reorder(replay);
    appendAndRemove(replay);
    const comparison = replay.compare();
    expect(childEntry).toMatchObject({
      customType: DELTA_ENTRY,
      data: { changed: [{ task: { description: "One changed child" } }], orders: [] },
    });
    expect(move.orders).toHaveLength(two);
    expect(move.changed).toStrictEqual([]);
    expect(comparison.actual).toStrictEqual(comparison.expected);
  });
});
