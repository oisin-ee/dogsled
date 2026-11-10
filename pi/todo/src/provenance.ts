import type { ExecutionRef, TodoList, UnitRef } from "./wasm.ts";

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

/** Derives entry provenance without rewriting the imported lead reference. */
class Provenance {
  readonly #origins = new Map<string, string>();
  readonly #seals = new Map<string, string>();

  public list(list: TodoList): TodoList {
    const current = structuredClone(list);
    for (const task of current.tasks.flatMap((root) => [root, ...(root.subtasks ?? [])])) {
      if (task.unit !== undefined) {
        this.resolve(task.unit, "");
      }
    }
    return current;
  }

  public resolve(
    unit: {
      readonly reference: Readonly<UnitRef>;
      readonly imported?:
        | {
            readonly seed: { readonly execution: Readonly<ExecutionRef> };
            readonly seal?: { readonly entryId: string } | undefined;
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

export { Provenance };
