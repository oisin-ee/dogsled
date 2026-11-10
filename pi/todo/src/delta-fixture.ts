import type { ReminderFact, TodoList } from "./wasm.ts";
import { STATE_ENTRY, branchList, branchSnapshot, latestSnapshot, stateChange } from "./state.ts";
import { apply, completionReminder, parse } from "./wasm.ts";
import { contract, first, firstId, fixture, last } from "./contract-fixture.ts";
import {
  referenceFacts,
  referenceLatest,
  referenceSnapshot,
} from "./snapshot-reference-fixture.ts";
import { reminder, reminderFacts } from "./reminder.ts";
import type { Immutable } from "./delta.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { TodoParams } from "./tool.ts";

interface SnapshotPair {
  readonly actual: TodoList;
  readonly expected: TodoList;
}
const snapshotLists = (facts: readonly Immutable<ReminderFact>[]): unknown[] => {
    const lists: unknown[] = [];
    for (const fact of facts) {
      if (fact.kind === "snapshot") {
        lists.push(fact.list);
      }
    }
    return lists;
  },
  deltaCount = (branch: readonly SessionEntry[]): number =>
    branch.filter((item) => item.type === "custom" && item.customType === "todo-delta").length,
  taskAt = (list: TodoList, index = first): NonNullable<TodoList["tasks"][number]> => {
    const task = list.tasks.at(index);
    if (task === undefined) {
      throw new Error("Missing fixture root");
    }
    return task;
  },
  storedData = (branch: readonly SessionEntry[]): { data: unknown; customType: string } => {
    const stored = branch.at(last);
    if (stored?.type !== "custom") {
      throw new Error("Missing custom state entry");
    }
    return { customType: stored.customType, data: stored.data };
  },
  taskCount = 192,
  entry = (id: string, customType: string, data: unknown): SessionEntry => ({
    customType,
    data,
    id,
    parentId: null,
    timestamp: "2026-11-10T00:00:00Z",
    type: "custom",
  }),
  initial = (count: number): TodoList =>
    apply(
      { tasks: [] },
      {
        items: Array.from({ length: count }, (_unused, index) => ({
          content: `Task ${index}`,
          contract: contract(),
        })),
        op: "init",
      },
      { mutationId: "initial", sessionId: "test-session" },
    ).list,
  statusSizes = (count: number): { delta: number; full: number; type: string } => {
    const caller = fixture(),
      before = caller.call({
        items: Array.from({ length: count }, (_unused, index) => ({
          content: `Task ${index}`,
          contract: contract(),
        })),
        op: "init",
      }),
      after = caller.call({ op: "start", task: firstId(before) }),
      change = storedData(caller.entries),
      encoder = new TextEncoder();
    return {
      delta: encoder.encode(JSON.stringify(change.data)).length,
      full: encoder.encode(JSON.stringify(after)).length,
      type: change.customType,
    };
  };

/** Paired old and new writers, with a provenance observation at each mutation. */
class Replay {
  public readonly branch: SessionEntry[] = [];
  public readonly full: SessionEntry[] = [];
  readonly #observed: SnapshotPair[] = [];

  public save(list: TodoList, forceFull = false): void {
    const id = `entry-${this.full.length}`,
      change = forceFull
        ? { customType: STATE_ENTRY, data: list }
        : stateChange(branchSnapshot(this.branch), list),
      parentId = this.full.at(last)?.id ?? null,
      changedData = structuredClone(change.data),
      fullData = structuredClone(list),
      deltaEntry = Object.assign(entry(id, change.customType, changedData), { parentId }),
      fullEntry = Object.assign(entry(id, STATE_ENTRY, fullData), { parentId });
    this.branch.push(deltaEntry);
    this.full.push(fullEntry);
    this.#observed.push({
      actual: branchSnapshot(this.branch),
      expected: referenceSnapshot(this.full),
    });
  }

  public call(params: TodoParams): void {
    const context = { mutationId: `change-${this.full.length}`, sessionId: "test-session" },
      expected = apply(referenceSnapshot(this.full), params, context),
      actual = apply(branchSnapshot(this.branch), params, context);
    this.#observed.push({ actual: actual.list, expected: expected.list });
    this.save(actual.list);
  }

  public compare(): { actual: unknown; expected: unknown } {
    const facts = referenceFacts(this.full);
    return {
      actual: {
        derived: branchSnapshot(this.branch),
        facts: reminderFacts(this.branch, []),
        frames: [...branchList(this.branch)].map((frame) => frame.list),
        full: branchSnapshot(this.full),
        latest: latestSnapshot(this.branch),
        observed: this.#observed.map((pair) => pair.actual),
        reminder: reminder(this.branch, []),
      },
      expected: {
        derived: referenceSnapshot(this.full),
        facts,
        frames: snapshotLists(facts),
        full: referenceSnapshot(this.full),
        latest: parse(referenceLatest(this.full)),
        observed: this.#observed.map((pair) => pair.expected),
        reminder: completionReminder({ facts }),
      },
    };
  }
}

export { Replay, deltaCount, entry, initial, statusSizes, storedData, taskAt, taskCount };
