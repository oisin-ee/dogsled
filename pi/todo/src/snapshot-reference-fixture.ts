import type { ExecutionRef, ReminderFact, TodoList, UnitRef } from "./wasm.ts";
import { REMINDER_ENTRY } from "./reminder.ts";
import { STATE_ENTRY } from "./state.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { parse } from "./wasm.ts";

// The full-snapshot implementation before deltas, retained as the independent oracle.
const originFor = (
  reference: Readonly<UnitRef>,
  entryId: string,
  origins: Readonly<Map<string, string>>,
): string => {
  const key = JSON.stringify([reference.sessionId, reference.unitId, reference.revision]),
    origin = origins.get(key) ?? entryId;
  origins.set(key, origin);
  return origin;
};
class ReferenceProvenance {
  readonly #origins = new Map<string, string>();
  readonly #seals = new Map<string, string>();

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
const referenceSnapshot = (branch: readonly SessionEntry[]): TodoList => {
    const provenance = new ReferenceProvenance();
    let current: TodoList = { tasks: [] };
    for (const entry of branch.filter(
      (candidate) => candidate.type === "custom" && candidate.customType === STATE_ENTRY,
    )) {
      if (entry.type === "custom") {
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
    }
    return current;
  },
  referenceLatest = (branch: readonly SessionEntry[]): unknown => {
    for (const entry of branch.toReversed()) {
      if (entry.type === "custom" && entry.customType === STATE_ENTRY) {
        return entry.data;
      }
    }
    return { tasks: [] };
  },
  referenceFacts = (branch: readonly SessionEntry[]): ReminderFact[] => {
    const facts: ReminderFact[] = [];
    for (const entry of branch) {
      if (entry.type === "message" && entry.message.role === "user") {
        facts.push({ kind: "user" });
      } else if (entry.type === "custom" && entry.customType === STATE_ENTRY) {
        facts.push({ kind: "snapshot", list: parse(entry.data) });
      } else if (entry.type === "custom_message" && entry.customType === REMINDER_ENTRY) {
        facts.push({ kind: "reminder" });
      }
    }
    return facts;
  };

export { referenceFacts, referenceLatest, referenceSnapshot };
