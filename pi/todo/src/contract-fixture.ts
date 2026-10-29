import type { TodoParams, TodoSession } from "./tool.ts";
import { STATE_ENTRY } from "./state.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { TodoList } from "./wasm.ts";
import { runTodo } from "./tool.ts";

interface Fixture {
  readonly entries: SessionEntry[];
  readonly session: TodoSession;
  readonly call: (params: TodoParams) => TodoList;
}

/**
 * Creates a branch-backed caller using the real tool and WASM core.
 * @param {readonly unknown[]} snapshots Existing stored snapshots.
 * @returns {Fixture} The tool caller and Pi append seam.
 */
const first = 0,
  second = 1,
  third = 2,
  last = -1,
  fixture = (snapshots: readonly unknown[] = []): Fixture => {
    const entries = snapshots.map((data): SessionEntry => ({
        customType: STATE_ENTRY,
        data,
        id: crypto.randomUUID(),
        parentId: null,
        timestamp: "2026-10-20T00:00:00Z",
        type: "custom",
      })),
      session: TodoSession = {
        append: (customType, data): void => {
          entries.push({
            customType,
            data,
            id: crypto.randomUUID(),
            parentId: entries.at(last)?.id ?? null,
            timestamp: "2026-10-20T00:00:00Z",
            type: "custom",
          });
        },
        branch: () => entries,
        sessionId: "test-session",
      };
    return { call: (params) => runTodo(params, session).details, entries, session };
  },
  /**
   * A complete self-assigned contract for deterministic behavior tests.
   * @returns {NonNullable<TodoParams["contract"]>} Authored contract fields.
   */
  contract = (): NonNullable<TodoParams["contract"]> => ({
    acceptance: {
      argv: ["moon", "run", "todo-core:test"],
      cwd: ".",
      expected: "passes",
      kind: "command",
    },
    assignment: { kind: "self" },
    deliverable: "Executable task contracts",
    dependsOn: [],
    scope: { generates: [], owns: ["src/"], reads: ["README.md"], repositoryId: "oisinpi" },
  }),
  /**
   * Returns the runtime ID from the first root in a tool result.
   * @param {TodoList} list Real tool result.
   * @returns {string} Runtime ID.
   */
  firstId = (list: TodoList): string => {
    const id = list.tasks.at(first)?.unit?.reference.unitId;
    if (id === undefined) {
      throw new Error("Expected a runtime unit ID");
    }
    return id;
  },
  /**
   * Requires a root identity at a known fixture position.
   * @param {TodoList} list Tool result.
   * @param {number} index Root position.
   * @returns {string} Runtime ID.
   */
  rootId = (list: TodoList, index: number): string => {
    const id = list.tasks.at(index)?.unit?.reference.unitId;
    if (id === undefined) {
      throw new Error("Expected root identity");
    }
    return id;
  },
  /**
   * Requires a child identity from the first outcome.
   * @param {TodoList} list Tool result.
   * @param {number} index Child position.
   * @returns {string} Runtime ID.
   */
  childId = (list: TodoList, index: number): string => {
    const id = list.tasks.at(first)?.subtasks?.at(index)?.unit?.reference.unitId;
    if (id === undefined) {
      throw new Error("Expected child identity");
    }
    return id;
  };

export { childId, contract, first, firstId, fixture, last, rootId, second, third };
export type { Fixture };
