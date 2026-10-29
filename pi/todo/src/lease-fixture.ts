import { contract, firstId } from "./contract-fixture.ts";
import { emptyParameters, emptyResult, runtimeFixture } from "./runtime-fixture.ts";
import { branchSnapshot } from "./state.ts";

/**
 * Completes an unrelated unit on a fresh branch.
 * @param {Readonly<object>} test Runtime task and branch operations.
 */
const unrelated = async (test: {
  readonly manager: {
    readonly resetLeaf: () => void;
    readonly getBranch: () => ReturnType<
      Awaited<ReturnType<typeof runtimeFixture>>["manager"]["getBranch"]
    >;
  };
  readonly todo: Awaited<ReturnType<typeof runtimeFixture>>["todo"];
}): Promise<void> => {
  test.manager.resetLeaf();
  await test.todo({
    items: [{ content: "Other branch", contract: contract(), key: "other" }],
    op: "init",
    start: "other",
  });
  const list = branchSnapshot(test.manager.getBranch());
  await test.todo({ evidence: "Reviewed unrelated work", op: "done", task: firstId(list) });
};

interface Leased {
  readonly test: Awaited<ReturnType<typeof runtimeFixture>>;
  readonly id: string;
  readonly calls: readonly string[];
  readonly revisit: () => void;
  readonly unrelated: () => Promise<void>;
  readonly finish: () => Promise<void>;
  readonly close: () => void;
}

/**
 * Owns the controlled execution boundary used by lease regressions.
 * @param {boolean} nested Runs through real codemode when true.
 * @param {boolean} fail Throws at the actual execute boundary when true.
 * @returns {Promise<object>} Running tool and deterministic completion controls.
 */
const leased = async (nested: boolean, fail: boolean): Promise<Leased> => {
  const entered = Promise.withResolvers<boolean>(),
    finished = Promise.withResolvers<boolean>(),
    calls: string[] = [],
    test = await runtimeFixture((pi) => {
      pi.registerTool({
        description: "Controlled mutation",
        execute: async (id) => {
          calls.push(id);
          entered.resolve(true);
          await finished.promise;
          if (fail) {
            throw new Error("Mutation failed");
          }
          return emptyResult();
        },
        label: "Mutation",
        name: "mutation",
        parameters: emptyParameters,
      });
    });
  await test.create();
  const id = firstId(branchSnapshot(test.manager.getBranch())),
    running = nested
      ? test.call("codemode", { code: "return await tools.mutation({})" })
      : test.call("mutation", {});
  await entered.promise;
  const origin = test.manager.getLeafId();
  if (origin === null) {
    throw new Error("Expected leased origin branch leaf");
  }
  return {
    calls,
    close: (): void => {
      finished.resolve(true);
      test.session.dispose();
    },
    finish: async (): Promise<void> => {
      finished.resolve(true);
      await running;
    },
    id,
    revisit: (): void => {
      test.manager.resetLeaf();
      test.manager.branch(origin);
    },
    test,
    unrelated: async (): Promise<void> => unrelated(test),
  };
};

export { leased };
