import { DELTA_ENTRY, STATE_ENTRY, branchSnapshot } from "./state.ts";
import { apply, importUnit, imported } from "./wasm.ts";
import { entry, taskAt } from "./delta-fixture.ts";
import { referenceFacts, referenceSnapshot } from "./snapshot-reference-fixture.ts";
import { firstId } from "./contract-fixture.ts";
import { leafTask } from "./delta.ts";
import { reminderFacts } from "./reminder.ts";
import { worker } from "./provider-fixture.ts";

const sealReference = async (): Promise<{ actual: unknown; expected: unknown; seal: unknown }> => {
  const source = await worker("github.com/oisin-ee/oisinpi");
  try {
    const seed = JSON.stringify({
        binding: source.claimed.binding,
        execution: source.claimed.binding.execution,
        exported: source.claimed.exported,
        version: 1,
      }),
      importedList = importUnit(
        { tasks: [] },
        seed,
        "worker-session",
        "github.com/oisin-ee/oisinpi",
      ).list,
      branch = [entry("import", STATE_ENTRY, importedList)],
      full = [entry("import", STATE_ENTRY, importedList)],
      unitId = firstId(importedList),
      sealed = apply(
        branchSnapshot(branch),
        { evidence: "All unit checks pass", op: "done", task: unitId },
        { mutationId: "seal", sessionId: "worker-session" },
      ).list,
      delta = {
        changed: [{ id: unitId, task: leafTask(taskAt(sealed)) }],
        orders: [],
        removed: [],
        version: 1,
      };
    branch.push(entry("seal", DELTA_ENTRY, delta), entry("repeat", DELTA_ENTRY, delta));
    full.push(entry("seal", STATE_ENTRY, sealed), entry("repeat", STATE_ENTRY, sealed));
    return {
      actual: {
        facts: reminderFacts(branch, []),
        list: branchSnapshot(branch),
        reference: taskAt(branchSnapshot(branch)).unit?.reference,
      },
      expected: {
        facts: referenceFacts(full),
        list: referenceSnapshot(full),
        reference: source.claimed.binding.contract,
      },
      seal: imported(branchSnapshot(branch), "worker-session")?.seal,
    };
  } finally {
    source.test.session.dispose();
  }
};

export { sealReference };
