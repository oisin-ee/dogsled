import type { Subtask, Task, TodoList } from "./wasm.ts";
import type { OrderChange } from "./order-index.ts";

type Immutable<Value> = { readonly [Key in keyof Value]: Immutable<Value[Key]> };
interface TaskChange {
  readonly id: string;
  readonly task: Immutable<Subtask>;
}
interface TodoDelta {
  readonly changed: readonly TaskChange[];
  readonly orders: readonly OrderChange[];
  readonly removed: readonly string[];
  readonly version: typeof deltaVersion;
}
interface ListIndex {
  readonly orders: Readonly<Map<string | null, readonly string[]>>;
  readonly tasks: Readonly<Map<string, Immutable<Subtask>>>;
}
const STATE_ENTRY = "todo-state",
  DELTA_ENTRY = "todo-delta",
  deltaVersion = 1,
  first = 0,
  step = 1,
  halves = 2,
  last = -1;
type StoredChange =
  | { readonly customType: typeof STATE_ENTRY; readonly data: TodoList }
  | { readonly customType: typeof DELTA_ENTRY; readonly data: TodoDelta };

// A child and a root carry the same fields, excluding nested children.
const leafTask = (task: Task | Immutable<Subtask>): Immutable<Subtask> =>
    Object.assign(
      { content: task.content, status: task.status },
      task.description === undefined ? {} : { description: task.description },
      task.unit === undefined ? {} : { unit: task.unit },
    ),
  childIndex = (
    root: Task,
    tasks: Readonly<Map<string, Immutable<Subtask>>>,
  ): string[] | undefined => {
    const children: string[] = [];
    for (const child of root.subtasks ?? []) {
      const childId = child.unit?.reference.unitId;
      if (childId === undefined) {
        return undefined;
      }
      children.push(childId);
      tasks.set(childId, child);
    }
    return children;
  },
  indexed = (list: TodoList): ListIndex | undefined => {
    const tasks = new Map<string, Immutable<Subtask>>(),
      orders = new Map<string | null, readonly string[]>(),
      roots: string[] = [];
    for (const root of list.tasks) {
      const unitId = root.unit?.reference.unitId,
        children = childIndex(root, tasks);
      if (unitId === undefined || children === undefined) {
        return undefined;
      }
      roots.push(unitId);
      tasks.set(unitId, leafTask(root));
      orders.set(unitId, children);
    }
    orders.set(null, roots);
    return { orders, tasks };
  },
  insertion = (tails: readonly { readonly position: number }[], position: number): number => {
    let lower = first,
      upper = tails.length;
    while (lower < upper) {
      const middle = Math.floor((lower + upper) / halves);
      if ((tails.at(middle)?.position ?? Infinity) < position) {
        lower = middle + step;
      } else {
        upper = middle;
      }
    }
    return lower;
  },
  predecessorsFor = (
    start: string | undefined,
    predecessors: Readonly<Map<string, string | undefined>>,
  ): ReadonlySet<string> => {
    const stable = new Set<string>();
    let current = start;
    while (current !== undefined) {
      stable.add(current);
      current = predecessors.get(current);
    }
    return stable;
  },
  stableOrder = (before: readonly string[], after: readonly string[]): ReadonlySet<string> => {
    const positions = new Map(before.map((unitId, index) => [unitId, index])),
      tails: { readonly id: string; readonly position: number }[] = [],
      predecessors = new Map<string, string | undefined>();
    for (const unitId of after) {
      const position = positions.get(unitId);
      if (position !== undefined) {
        const offset = insertion(tails, position);
        predecessors.set(unitId, offset === first ? undefined : tails.at(offset - step)?.id);
        tails[offset] = { id: unitId, position };
      }
    }
    return predecessorsFor(tails.at(last)?.id, predecessors);
  },
  orderChanges = (before: Readonly<ListIndex>, after: Readonly<ListIndex>): OrderChange[] => {
    const orders: OrderChange[] = [];
    for (const [parent, ids] of after.orders) {
      const stable = stableOrder(before.orders.get(parent) ?? [], ids);
      let previous: string | null = null;
      for (const unitId of ids) {
        if (!stable.has(unitId)) {
          orders.push({ after: previous, id: unitId, parent });
        }
        previous = unitId;
      }
    }
    return orders;
  },
  changedTasks = (before: Readonly<ListIndex>, after: Readonly<ListIndex>): TaskChange[] => {
    const changes: TaskChange[] = [];
    for (const [unitId, task] of after.tasks) {
      if (JSON.stringify(before.tasks.get(unitId)) !== JSON.stringify(task)) {
        changes.push({ id: unitId, task });
      }
    }
    return changes;
  },
  deltaBetween = (before: Readonly<ListIndex>, after: Readonly<ListIndex>): TodoDelta => ({
    changed: changedTasks(before, after),
    orders: orderChanges(before, after),
    removed: [...before.tasks.keys()].filter((unitId) => !after.tasks.has(unitId)),
    version: deltaVersion,
  }),
  stateChange = (before: TodoList, after: TodoList): StoredChange => {
    const previous = indexed(before),
      next = indexed(after);
    if (previous === undefined || next === undefined) {
      return { customType: STATE_ENTRY, data: after };
    }
    const data = deltaBetween(previous, next),
      encoder = new TextEncoder();
    return encoder.encode(JSON.stringify(data)).length <
      encoder.encode(JSON.stringify(after)).length
      ? { customType: DELTA_ENTRY, data }
      : { customType: STATE_ENTRY, data: after };
  };

export { DELTA_ENTRY, STATE_ENTRY, deltaVersion, indexed, leafTask, stateChange };
export type { Immutable, ListIndex, TaskChange, TodoDelta };
