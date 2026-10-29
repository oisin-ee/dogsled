import type { CustomEntry, SessionEntry } from "@earendil-works/pi-coding-agent";
import type { ExecutionRef, TodoList, UnitRef } from "./wasm.ts";
import { parse } from "./wasm.ts";

/** The snapshot of a branch with no list yet. */
const EMPTY: unknown = { tasks: [] },
  /** The custom entry that holds the list after every change. */
  STATE_ENTRY = "todo-state",
  /**
   * Whether `entry` is a stored list.
   * @param {SessionEntry} entry A branch entry.
   * @returns {boolean} True for a `todo-state` custom entry.
   */
  isStateEntry = (entry: SessionEntry): entry is CustomEntry =>
    entry.type === "custom" && entry.customType === STATE_ENTRY,
  /**
   * The latest stored snapshot on the branch, unvalidated: the core validates it.
   * @param {readonly SessionEntry[]} branch The session branch, oldest entry first.
   * @returns {unknown} The latest snapshot, or an empty list when there is none.
   */
  latestSnapshot = (branch: readonly SessionEntry[]): unknown => {
    for (const entry of branch.toReversed()) {
      if (isStateEntry(entry)) {
        return entry.data;
      }
    }
    return EMPTY;
  },
  /**
   * Resolves the first branch entry for a contract revision.
   * @param {Readonly<UnitRef>} reference Revision identity.
   * @param {string} entryId Containing Pi entry.
   * @param {Readonly<Map<string, string>>} origins Prior branch origins.
   * @returns {string} The revision's originating entry.
   */
  originFor = (
    reference: Readonly<UnitRef>,
    entryId: string,
    origins: Readonly<Map<string, string>>,
  ): string => {
    const key = JSON.stringify([reference.sessionId, reference.unitId, reference.revision]),
      origin = origins.get(key) ?? entryId;
    origins.set(key, origin);
    return origin;
  };

/** Derives entry provenance without rewriting the imported lead reference. */
class Provenance {
  readonly #origins = new Map<string, string>();
  readonly #seals = new Map<string, string>();

  /**
   * Resolves contract and seal provenance for one task snapshot.
   * @param {Readonly<object>} unit Task provenance fields.
   * @param {string} entryId Snapshot identity.
   */
  public resolve(
    unit: {
      readonly reference: Readonly<UnitRef>;
      readonly imported:
        | {
            readonly seed: { readonly execution: Readonly<ExecutionRef> };
            readonly seal: { readonly entryId: string } | undefined;
          }
        | undefined;
    },
    entryId: string,
  ): void {
    if (unit.imported === undefined) {
      Object.assign(unit.reference, {
        contractEntryId: originFor(unit.reference, entryId, this.#origins),
      });
    } else if (unit.imported.seal !== undefined) {
      const key = JSON.stringify(unit.imported.seed.execution),
        sealEntry = this.#seals.get(key) ?? entryId;
      this.#seals.set(key, sealEntry);
      Object.assign(unit.imported.seal, { entryId: sealEntry });
    }
  }
}

/**
 * Reads revision origins from the entries that first introduced them.
 * Status-only snapshots cannot move a revision's origin.
 * @param {readonly SessionEntry[]} branch Current branch in chronological order.
 * @returns {TodoList} Validated snapshot with derived provenance.
 */
const branchSnapshot = (branch: readonly SessionEntry[]): TodoList => {
  const provenance = new Provenance();
  let current: TodoList = { tasks: [] };
  for (const entry of branch.filter((candidate) => isStateEntry(candidate))) {
    current = parse(entry.data);
    for (const task of current.tasks.flatMap((root) => [root, ...(root.subtasks ?? [])])) {
      if (task.unit !== undefined) {
        provenance.resolve(
          { imported: task.unit.imported, reference: task.unit.reference },
          entry.id,
        );
      }
    }
  }
  return current;
};

export { STATE_ENTRY, branchSnapshot, latestSnapshot };
