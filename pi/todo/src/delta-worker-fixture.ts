import { DELTA_ENTRY, branchSnapshot } from "./state.ts";
import { contract, first, firstId, last } from "./contract-fixture.ts";
import type { RuntimeFixture } from "./runtime-fixture.ts";
import { apply } from "./wasm.ts";
import { importFixture } from "./import-fixture.ts";
import { leafTask } from "./delta.ts";
import { runtimeFixture } from "./runtime-fixture.ts";
import { value } from "./provider-fixture.ts";

const workers = 100,
  workerDrafts = (): { content: string; contract: ReturnType<typeof contract> }[] =>
    Array.from({ length: workers }, (_unused, index) => ({
      content: `Worker ${index}`,
      contract: Object.assign(contract(), {
        assignment: { kind: "worker" as const, profile: "general" },
      }),
    })),
  claimScenario = async (): Promise<{ actual: unknown; origin: string }> => {
    const runtime = await runtimeFixture();
    try {
      await runtime.todo({ items: workerDrafts(), op: "init" });
      const unitId = firstId(branchSnapshot(runtime.manager.getBranch())),
        provider = runtime.provider(),
        inspected = value(provider.inspect(unitId)),
        claimed = value(
          provider.claim({
            execution: { executor: "dogsled", ownerId: "owner", requestId: "delta-claim" },
            expected: inspected.ref,
          }),
        ),
        claimEntry = runtime.manager.getBranch().at(last),
        settled = provider.settle({
          binding: claimed.binding,
          eventId: "delta-returned",
          result: {
            evidence: {
              execution: claimed.binding.execution,
              resultId: "reply",
              summary: "Worker result",
            },
            kind: "returned",
          },
          version: 1,
        }),
        settleEntry = runtime.manager.getBranch().at(last),
        task = branchSnapshot(runtime.manager.getBranch()).tasks.at(first);
      return {
        actual: {
          claimEntry,
          evidence: task?.unit?.evidence?.length,
          origin: task?.unit?.reference.contractEntryId,
          settleEntry,
          settled,
          task,
        },
        origin: inspected.ref.contractEntryId,
      };
    } finally {
      runtime.session.dispose();
    }
  },
  rewind = (
    runtime: Readonly<Pick<RuntimeFixture, "provider">> & {
      readonly manager: Readonly<Pick<RuntimeFixture["manager"], "branch" | "resetLeaf">>;
    },
    origin: string,
  ): { active: unknown; imported: unknown } => {
    runtime.manager.branch(origin);
    const active = runtime.provider().active();
    runtime.manager.resetLeaf();
    return { active, imported: runtime.provider().imported() };
  },
  sealScenario = async (): Promise<{ receipt: unknown; rewound: unknown; sealId: string }> => {
    const fixture = await importFixture(),
      runtime = await runtimeFixture(undefined, false, fixture),
      { manager } = runtime,
      origin = manager.getLeafId();
    try {
      const sealed = apply(
          branchSnapshot(manager.getBranch()),
          { evidence: "Reviewed delta seal", op: "done", task: fixture.origin.id },
          { mutationId: "delta-seal", sessionId: manager.getSessionId() },
        ).list,
        root = sealed.tasks.at(first);
      if (root === undefined || origin === null) {
        throw new Error("Missing imported root");
      }
      const sealId = manager.appendCustomEntry(DELTA_ENTRY, {
          changed: [{ id: fixture.origin.id, task: leafTask(root) }],
          orders: [],
          removed: [],
          version: 1,
        }),
        receipt = value(runtime.provider().imported());
      return { receipt, rewound: rewind(runtime, origin), sealId };
    } finally {
      runtime.session.dispose();
      await fixture.close();
    }
  };

export { claimScenario, sealScenario };
