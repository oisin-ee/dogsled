import { DELTA_ENTRY, STATE_ENTRY, indexed, leafTask } from "./delta.ts";
import type { Immutable, ListIndex, TodoDelta } from "./delta.ts";
import type { Subtask, TodoList } from "./wasm.ts";
import { deltaData, snapshotData } from "./delta-validation.ts";
import { History } from "./history.ts";
import { OrderIndex } from "./order-index.ts";
import { Provenance } from "./provenance.ts";
import { parse } from "./wasm.ts";

interface BranchEntry {
  readonly type: string;
  readonly customType?: string;
  readonly data?: unknown;
  readonly id?: string;
}
interface ListFrame {
  readonly changed: readonly Immutable<Subtask>[];
  readonly entry: BranchEntry;
  readonly fenced: boolean;
  readonly progress: boolean;
  readonly list: TodoList;
  readonly snapshot: TodoList;
}
interface TaskRecord {
  readonly revision: number;
  readonly task: Immutable<Subtask>;
}
interface ListVersion {
  readonly hasBase: boolean;
  readonly orders: Readonly<OrderIndex>;
  readonly revision: number;
  readonly size: number;
  readonly tasks: Readonly<History<string, TaskRecord | undefined>>;
}
const step = 1,
  empty = 0,
  required = (
    version: Readonly<ListVersion>,
    unitId: string,
    provenance: Readonly<Provenance>,
  ): Immutable<Subtask> => {
    const record = version.tasks.get(unitId, version.revision);
    if (record === undefined) {
      throw new Error("INVALID_TODO_DELTA: order names a missing task");
    }
    const task = structuredClone(record.task);
    if (record.revision < version.revision && task.unit !== undefined) {
      provenance.resolve(task.unit, "");
    }
    return task;
  },
  materialize = (version: Readonly<ListVersion>, provenance: Readonly<Provenance>): TodoList => {
    if (!version.hasBase) {
      throw new Error("INVALID_TODO_DELTA: no stable-ID base snapshot");
    }
    const list = parse({
      tasks: version.orders.ids(null, version.revision).map((unitId) =>
        Object.assign({}, required(version, unitId, provenance), {
          subtasks: version.orders
            .ids(unitId, version.revision)
            .map((childId) => required(version, childId, provenance)),
        }),
      ),
    });
    if (
      list.tasks.reduce((count, root) => count + step + (root.subtasks?.length ?? empty), empty) !==
      version.size
    ) {
      throw new Error("INVALID_TODO_DELTA: tasks missing from order");
    }
    return list;
  };

/** Owns one linear replay, retaining changed values rather than full-list copies. */
class BranchState {
  readonly #provenance = new Provenance();
  #tasks = new History<string, TaskRecord | undefined>();
  #orders = new OrderIndex();
  #known = new Set<string>();
  #revision = empty;
  #hasBase = false;
  #fenced = false;
  #latest: ListFrame | undefined = undefined;

  #record(task: Immutable<Subtask>, entry: BranchEntry): void {
    if (task.unit !== undefined) {
      this.#provenance.resolve(structuredClone(task.unit), entry.id ?? "");
    }
  }

  #index(index: Readonly<ListIndex>): void {
    for (const [unitId, task] of index.tasks) {
      this.#tasks.set(unitId, this.#revision, { revision: this.#revision, task });
    }
    for (const [parent, ids] of index.orders) {
      let after: string | null = null;
      for (const unitId of ids) {
        this.#orders.apply({ after, id: unitId, parent }, this.#revision);
        after = unitId;
      }
    }
  }

  #replace(index: Readonly<ListIndex> | undefined): void {
    this.#hasBase = index !== undefined;
    this.#known = new Set(index?.tasks.keys());
    this.#tasks = new History();
    this.#orders = new OrderIndex();
    if (index !== undefined) {
      this.#index(index);
    }
  }

  #full(entry: BranchEntry, data: unknown): ListFrame {
    const { fenced, list } = snapshotData(data);
    if (this.#fenced && !fenced) {
      throw new Error("INVALID_TODO_DELTA: unfenced snapshot after delta history");
    }
    const progress = JSON.stringify(this.#latest?.list ?? { tasks: [] }) !== JSON.stringify(list),
      index = indexed(list),
      changed = list.tasks.flatMap((root) => [leafTask(root), ...(root.subtasks ?? [])]),
      provenance = this.#provenance;
    this.#fenced ||= fenced;
    this.#replace(index);
    for (const task of changed) {
      this.#record(task, entry);
    }
    return {
      changed,
      entry,
      fenced: this.#fenced,
      list,
      progress,
      get snapshot(): TodoList {
        return provenance.list(list);
      },
    };
  }

  #changed(delta: TodoDelta, entry: BranchEntry): boolean {
    let progress = false;
    for (const change of delta.changed) {
      progress ||=
        JSON.stringify(this.#tasks.get(change.id, this.#revision)?.task) !==
        JSON.stringify(change.task);
      this.#known.add(change.id);
      this.#tasks.set(change.id, this.#revision, { revision: this.#revision, task: change.task });
      this.#record(change.task, entry);
    }
    return progress;
  }

  #removed(ids: readonly string[]): boolean {
    let progress = false;
    for (const unitId of ids) {
      progress = this.#known.delete(unitId) || progress;
      this.#tasks.set(unitId, this.#revision, undefined);
      this.#orders.apply(unitId, this.#revision);
    }
    return progress;
  }

  #apply(delta: TodoDelta, entry: BranchEntry): boolean {
    const removed = this.#removed(delta.removed);
    let progress = this.#changed(delta, entry) || removed;
    for (const order of delta.orders) {
      if (!this.#known.has(order.id) || (order.parent !== null && !this.#known.has(order.parent))) {
        throw new Error("INVALID_TODO_DELTA: order names a missing task");
      }
      progress = this.#orders.apply(order, this.#revision) || progress;
    }
    return progress;
  }

  #delta(entry: BranchEntry, data: unknown): ListFrame {
    const delta = deltaData(data);
    if (!this.#hasBase) {
      throw new Error("INVALID_TODO_DELTA: no stable-ID base snapshot");
    }
    const progress = this.#apply(delta, entry),
      version: ListVersion = {
        hasBase: this.#hasBase,
        orders: this.#orders,
        revision: this.#revision,
        size: this.#known.size,
        tasks: this.#tasks,
      },
      provenance = this.#provenance;
    let list: TodoList | undefined = undefined;
    return {
      changed: delta.changed.map((change) => change.task),
      entry,
      fenced: this.#fenced,
      get list(): TodoList {
        list ??= materialize(version, provenance);
        return list;
      },
      progress,
      get snapshot(): TodoList {
        return provenance.list(this.list);
      },
    };
  }

  // Full snapshots replace the index. Deltas touch only changed tasks and links.
  public read(entry: BranchEntry): ListFrame | undefined {
    if (entry.type !== "custom") {
      return undefined;
    }
    if (entry.customType === STATE_ENTRY || entry.customType === DELTA_ENTRY) {
      this.#revision += step;
      this.#latest =
        entry.customType === STATE_ENTRY
          ? this.#full(entry, entry.data)
          : this.#delta(entry, entry.data);
      return this.#latest;
    }
    return undefined;
  }
}

export { BranchState };
export type { BranchEntry, ListFrame };
