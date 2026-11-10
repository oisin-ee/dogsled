import type { TaskChange, TodoDelta } from "./delta.ts";
import type { OrderChange } from "./order-index.ts";
import { deltaVersion } from "./delta.ts";

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
    return data;
  };

export { deltaData };
