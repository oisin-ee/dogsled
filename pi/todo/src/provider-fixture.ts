import type { ClaimResult, UnitContract } from "./wasm.ts";
import { contract, firstId } from "./contract-fixture.ts";
import type { Result } from "./provider.ts";
import type { RuntimeFixture } from "./runtime-fixture.ts";
import { branchSnapshot } from "./state.ts";
import { runtimeFixture } from "./runtime-fixture.ts";

/**
 * Requires successful provider output without unchecked assertions.
 * @param {Result<Value>} result Capability response.
 * @returns {Value} Validated success value.
 */
const value = <Value>(result: Result<Value>): Value => {
    if (!result.ok) {
      throw new Error(result.error.message);
    }
    return result.value;
  },
  /**
   * Creates and claims one worker through real registration.
   * @param {string} repository Logical repository identity.
   * @returns {Promise<object>} Origin runtime and frozen handoff.
   */
  worker = async (
    repository = "oisinpi",
  ): Promise<{
    test: RuntimeFixture;
    id: string;
    inspected: UnitContract;
    claimed: ClaimResult;
  }> => {
    const test = await runtimeFixture(),
      draft = contract();
    draft.assignment = { kind: "worker", profile: "general" };
    if (draft.scope !== undefined) {
      draft.scope.repositoryId = repository;
    }
    await test.todo({ items: [{ content: "Worker", contract: draft }], op: "init" });
    const id = firstId(branchSnapshot(test.manager.getBranch())),
      inspected = value(test.provider().inspect(id)),
      claimed = value(
        test.provider().claim({
          execution: { executor: "dogsled", ownerId: "owner", requestId: "request" },
          expected: inspected.ref,
        }),
      );
    return { claimed, id, inspected, test };
  };

export { value, worker };
