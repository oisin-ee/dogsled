import type { Task, TodoList } from "./wasm.ts";
import type { TaskChange, TodoDelta } from "./delta.ts";
import { deltaVersion, leafTask } from "./delta.ts";
import { parse, validateTask } from "./wasm.ts";
import type { OrderChange } from "./order-index.ts";

const record = (data: unknown): data is Readonly<Record<string, unknown>> =>
    typeof data === "object" && data !== null,
  nullableId = (data: unknown): data is string | null => data === null || typeof data === "string",
  taskChange = (data: unknown): data is TaskChange =>
    record(data) &&
    typeof data["id"] === "string" &&
    record(data["task"]) &&
    !("subtasks" in data["task"]) &&
    Object.keys(data).every((key) => key === "id" || key === "task"),
  orderChange = (data: unknown): data is OrderChange =>
    record(data) &&
    typeof data["id"] === "string" &&
    nullableId(data["parent"]) &&
    nullableId(data["after"]) &&
    Object.keys(data).every((key) => ["id", "parent", "after"].includes(key)),
  envelope = (data: unknown): data is TodoDelta =>
    record(data) &&
    data["version"] === deltaVersion &&
    Object.keys(data).every((key) => ["version", "changed", "orders", "removed"].includes(key)) &&
    Array.isArray(data["changed"]) &&
    data["changed"].every((change: unknown) => taskChange(change)) &&
    Array.isArray(data["removed"]) &&
    data["removed"].every((unitId: unknown) => typeof unitId === "string") &&
    Array.isArray(data["orders"]) &&
    data["orders"].every((order: unknown) => orderChange(order)),
  changedTask = (data: unknown): Task => {
    try {
      return validateTask(data);
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`INVALID_TODO_DELTA: ${message}`, { cause: error });
    }
  },
  deltaData = (data: unknown): TodoDelta => {
    if (!envelope(data)) {
      throw new Error("INVALID_TODO_DELTA: unsupported or malformed delta");
    }
    const removed = new Set(data.removed);
    if (
      new Set(data.changed.map((change) => change.id)).size !== data.changed.length ||
      removed.size !== data.removed.length ||
      data.changed.some((change) => removed.has(change.id))
    ) {
      throw new Error("INVALID_TODO_DELTA: repeated or conflicting task changes");
    }
    const changed = data.changed.map((change): TaskChange => {
      const task = changedTask(change.task);
      if (change.id !== task.unit?.reference.unitId) {
        throw new Error("INVALID_TODO_DELTA: changed task identity differs");
      }
      return { id: change.id, task: leafTask(task) };
    });
    return Object.assign({}, data, { changed });
  },
  snapshotData = (data: unknown): { readonly fenced: boolean; readonly list: TodoList } => {
    const fenced = record(data) && "format" in data;
    if (fenced && data["format"] !== "todo-delta/1") {
      throw new Error("INVALID_TODO_DELTA: unsupported snapshot format");
    }
    return {
      fenced,
      list: parse(
        fenced
          ? Object.fromEntries(
              Object.entries(data).filter(([key]: readonly [string, unknown]) => key !== "format"),
            )
          : data,
      ),
    };
  };

export { deltaData, snapshotData };
