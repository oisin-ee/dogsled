import type { BranchEntry, ListFrame } from "./branch-state.ts";
import { DELTA_ENTRY, STATE_ENTRY, stateChange } from "./delta.ts";
import type { Immutable, TodoDelta } from "./delta.ts";
import type { Subtask, TodoList } from "./wasm.ts";
import { deltaData, snapshotData } from "./delta-validation.ts";
import { BranchState } from "./branch-state.ts";

/**
 * Replays full snapshots and deltas in branch order, in O(entries + changed tasks).
 * Full lists are materialized only on request; retained frames remain stable.
 * @param {readonly BranchEntry[]} branch Entries in chronological order.
 * @yields {ListFrame} State after each task entry, with changed records and lazy lists.
 */
const branchList = function* branchList(branch: readonly BranchEntry[]): Generator<ListFrame> {
    const state = new BranchState();
    for (const entry of branch) {
      const frame = state.read(entry);
      if (frame !== undefined) {
        yield frame;
      }
    }
  },
  /**
   * Finds the final folded frame.
   * @param {readonly BranchEntry[]} branch Current branch.
   * @returns {ListFrame | undefined} Last frame.
   */
  latestFrame = (branch: readonly BranchEntry[]): ListFrame | undefined => {
    let latest: ListFrame | undefined = undefined;
    for (const frame of branchList(branch)) {
      latest = frame;
    }
    return latest;
  },
  /**
   * Reads the final stored list.
   * @param {readonly BranchEntry[]} branch Current branch.
   * @returns {TodoList} Stored list.
   */
  latestSnapshot = (branch: readonly BranchEntry[]): TodoList =>
    latestFrame(branch)?.list ?? { tasks: [] },
  /**
   * Reads the final list with revision and seal origins.
   * @param {readonly BranchEntry[]} branch Current branch.
   * @returns {TodoList} Resolved list.
   */
  branchSnapshot = (branch: readonly BranchEntry[]): TodoList =>
    latestFrame(branch)?.snapshot ?? { tasks: [] },
  /**
   * Reads records across the session tree without combining unrelated branches.
   * @param {BranchEntry} entry One persisted entry.
   * @returns {readonly Immutable<Subtask>[]} Validated changed records.
   */
  entryTasks = (entry: BranchEntry): readonly Immutable<Subtask>[] => {
    if (entry.type !== "custom") {
      return [];
    }
    if (entry.customType === DELTA_ENTRY) {
      return deltaData(entry.data).changed.map((change) => change.task);
    }
    const tasks: Immutable<Subtask>[] = [];
    if (entry.customType === STATE_ENTRY) {
      for (const root of snapshotData(entry.data).list.tasks) {
        tasks.push(root, ...(root.subtasks ?? []));
      }
    }
    return tasks;
  },
  /**
   * Appends the downgrade fence before the first delta and marks later snapshots.
   * @param {TodoList} before Current list.
   * @param {TodoList} after Changed list.
   * @param {object} session Branch reader and entry writer.
   */
  saveState = (
    before: TodoList,
    after: TodoList,
    session: {
      readonly branch: () => readonly BranchEntry[];
      readonly append: (customType: string, data: TodoList | TodoDelta, list: TodoList) => void;
    },
  ): void => {
    const change = stateChange(before, after),
      fenced = latestFrame(session.branch())?.fenced === true,
      format = "todo-delta/1";
    if (change.customType === DELTA_ENTRY && !fenced) {
      // Authorize the intended mutation before appending even its fence.
      session.append(STATE_ENTRY, Object.assign({}, before, { format }), after);
    }
    session.append(
      change.customType,
      change.customType === STATE_ENTRY && fenced
        ? Object.assign({}, change.data, { format })
        : change.data,
      after,
    );
  };

export { branchList, branchSnapshot, latestSnapshot, entryTasks, saveState };
export { DELTA_ENTRY, STATE_ENTRY, stateChange } from "./delta.ts";
export type { TodoDelta } from "./delta.ts";
