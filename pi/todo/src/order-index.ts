import { History } from "./history.ts";

interface OrderChange {
  readonly after: string | null;
  readonly id: string;
  readonly parent: string | null;
}
interface Neighbours {
  readonly next: string | null;
  readonly previous: string | null;
}
interface Position {
  next: string | null;
  readonly parent: string | null;
  previous: string | null;
}
const first = 0,
  step = 1;

/** Links ordered tasks without copying siblings when one task moves. */
class OrderIndex {
  readonly #positions = new Map<string, Position>();
  readonly #heads = new History<string | null, string | null>();
  readonly #next = new History<string, string | null>();
  readonly #pending: { readonly revision: number; readonly change: OrderChange | string }[] = [];
  #cursor = first;

  /**
   * Defers links until a reader requests a list, not just changed records.
   * @param {OrderChange | string} change Placement or removed ID.
   * @param {number} revision Replay revision.
   */
  public defer(change: OrderChange | string, revision: number): void {
    this.#pending.push({ change, revision });
  }

  #connect(parent: string | null, pair: Readonly<Neighbours>, revision: number): void {
    const previous = pair.previous === null ? undefined : this.#positions.get(pair.previous),
      next = pair.next === null ? undefined : this.#positions.get(pair.next);
    if (pair.previous === null) {
      this.#heads.set(parent, revision, pair.next);
    } else {
      this.#next.set(pair.previous, revision, pair.next);
    }
    if (previous !== undefined) {
      previous.next = pair.next;
    }
    if (next !== undefined) {
      next.previous = pair.previous;
    }
  }

  #remove(id: string, revision: number): void {
    const position = this.#positions.get(id);
    if (position !== undefined) {
      this.#connect(position.parent, position, revision);
      this.#positions.delete(id);
    }
  }

  #place(change: OrderChange, revision: number): void {
    this.#remove(change.id, revision);
    const previous = change.after === null ? undefined : this.#positions.get(change.after);
    if (
      change.id === change.after ||
      (change.after !== null && previous?.parent !== change.parent)
    ) {
      throw new Error("INVALID_TODO_DELTA: invalid order predecessor");
    }
    const next =
      change.after === null
        ? (this.#heads.get(change.parent, revision) ?? null)
        : (previous?.next ?? null);
    this.#positions.set(change.id, { next, parent: change.parent, previous: change.after });
    this.#connect(change.parent, { next: change.id, previous: change.after }, revision);
    this.#connect(change.parent, { next, previous: change.id }, revision);
  }

  #apply(revision: number): void {
    let operation = this.#pending.at(this.#cursor);
    while (operation !== undefined && operation.revision <= revision) {
      if (typeof operation.change === "string") {
        this.#remove(operation.change, operation.revision);
      } else {
        this.#place(operation.change, operation.revision);
      }
      this.#cursor += step;
      operation = this.#pending.at(this.#cursor);
    }
  }

  /**
   * Reads ordered IDs for a historical frame.
   * @param {string | null} parent Parent ID, or null for roots.
   * @param {number} revision Replay revision.
   * @returns {string[]} Ordered task IDs.
   */
  public ids(parent: string | null, revision: number): string[] {
    this.#apply(revision);
    const ids: string[] = [],
      seen = new Set<string>();
    let current = this.#heads.get(parent, revision) ?? null;
    while (current !== null) {
      if (seen.has(current)) {
        throw new Error("INVALID_TODO_DELTA: cyclic order");
      }
      seen.add(current);
      ids.push(current);
      current = this.#next.get(current, revision) ?? null;
    }
    return ids;
  }
}

export { OrderIndex };
export type { OrderChange };
