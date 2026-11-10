import type { BranchEntry, ListFrame } from "./branch-state.ts";
import { BranchState } from "./branch-state.ts";
import type { TodoList } from "./wasm.ts";

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
    latestFrame(branch)?.snapshot ?? { tasks: [] };

export { branchList, branchSnapshot, latestSnapshot };
export { DELTA_ENTRY, STATE_ENTRY, stateChange } from "./delta.ts";
export type { TodoDelta } from "./delta.ts";
