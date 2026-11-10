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
/** Links ordered tasks without copying siblings when one task moves. */
class OrderIndex {
  readonly #positions = new Map<string, Position>();
  readonly #heads = new History<string | null, string | null>();
  readonly #next = new History<string, string | null>();
  /**
   * Validates and applies one link while retaining historical versions.
   * @param {OrderChange | string} change Placement or removed ID.
   * @param {number} revision Replay revision.
   * @returns {boolean} Whether the placement changed.
   */
  public apply(change: OrderChange | string, revision: number): boolean {
    const id = typeof change === "string" ? change : change.id,
      previous = this.#positions.get(id),
      changed =
        typeof change === "string"
          ? previous !== undefined
          : previous?.parent !== change.parent || previous.previous !== change.after;
    if (typeof change === "string") {
      this.#remove(change, revision);
    } else {
      this.#place(change, revision);
    }
    return changed;
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
    const previous = change.after === null ? undefined : this.#positions.get(change.after);
    if (
      change.id === change.after ||
      change.id === change.parent ||
      (change.after !== null && previous?.parent !== change.parent)
    ) {
      throw new Error("INVALID_TODO_DELTA: invalid order predecessor");
    }
    this.#remove(change.id, revision);
    const next =
      change.after === null
        ? (this.#heads.get(change.parent, revision) ?? null)
        : (previous?.next ?? null);
    this.#positions.set(change.id, { next, parent: change.parent, previous: change.after });
    this.#connect(change.parent, { next: change.id, previous: change.after }, revision);
    this.#connect(change.parent, { next, previous: change.id }, revision);
  }

  /**
   * Reads ordered IDs for a historical frame.
   * @param {string | null} parent Parent ID, or null for roots.
   * @param {number} revision Replay revision.
   * @returns {string[]} Ordered task IDs.
   */
  public ids(parent: string | null, revision: number): string[] {
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
