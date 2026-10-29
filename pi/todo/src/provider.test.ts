import { contract, first, second } from "./contract-fixture.ts";
import { describe, expect, it } from "vitest";
import { value, worker } from "./provider-fixture.ts";
import { branchSnapshot } from "./state.ts";

describe("provider claim replay", () => {
  it(
    "preserves exact exports and rejects conflicting executions",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const fixture = await worker(),
        provider = fixture.test.provider(),
        count = fixture.test.manager.getBranch().length;
      try {
        expect(
          value(
            provider.claim({
              execution: fixture.claimed.binding.execution,
              expected: fixture.inspected.ref,
            }),
          ),
        ).toStrictEqual(fixture.claimed);
        expect(fixture.test.manager.getBranch()).toHaveLength(count);
        expect(provider.active()).toStrictEqual({ ok: true, value: null });
        expect(
          provider.claim({
            execution: { executor: "dogsled", ownerId: "owner", requestId: "other" },
            expected: fixture.inspected.ref,
          }),
        ).toMatchObject({ error: { code: "UNIT_ALREADY_BOUND" }, ok: false });
      } finally {
        fixture.test.session.dispose();
      }
    },
  );
});

describe("provider acceptance", () => {
  it("requires explicit review after released worker evidence", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const fixture = await worker(),
      provider = fixture.test.provider(),
      settlement = {
        binding: fixture.claimed.binding,
        eventId: "returned",
        result: {
          evidence: {
            execution: fixture.claimed.binding.execution,
            resultId: "reply",
            summary: "untrusted work",
          },
          kind: "returned",
        },
        version: 1,
      };
    try {
      await expect(
        fixture.test.todo({ contract: contract(), op: "update", task: fixture.id }),
      ).rejects.toThrow("UNIT_ALREADY_BOUND");
      expect(provider.settle(settlement)).toStrictEqual({ ok: true, value: { applied: true } });
      expect(provider.settle(settlement)).toStrictEqual({ ok: true, value: { applied: false } });
      expect(branchSnapshot(fixture.test.manager.getBranch()).tasks.at(first)?.status).toBe(
        "in_progress",
      );
      await fixture.test.todo({
        evidence: "Tests and diff reviewed",
        op: "done",
        task: fixture.id,
      });
      expect(branchSnapshot(fixture.test.manager.getBranch()).tasks.at(first)?.status).toBe(
        "completed",
      );
    } finally {
      fixture.test.session.dispose();
    }
  });
});

describe("provider scope and origin", () => {
  it("allows overlapping local work and rejects inactive origin", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const fixture = await worker(),
      origin = Object.assign(structuredClone(fixture.inspected.ref), {
        contractEntryId: "other-branch",
      });
    try {
      await fixture.test.todo({
        items: [{ content: "Local", contract: contract(), key: "local" }],
        op: "append",
        start: "local",
      });
      expect(branchSnapshot(fixture.test.manager.getBranch()).tasks.at(second)?.status).toBe(
        "in_progress",
      );
      const count = fixture.test.manager.getBranch().length;
      expect(
        fixture.test.provider().settle({
          eventId: "not-started",
          execution: fixture.claimed.binding.execution,
          origin,
          result: { kind: "not_started", reason: "never launched" },
          version: 1,
        }),
      ).toMatchObject({ error: { code: "STALE_ORIGIN" }, ok: false });
      expect(fixture.test.manager.getBranch()).toHaveLength(count);
      expect(
        branchSnapshot(fixture.test.manager.getBranch()).tasks.at(first)?.unit?.binding,
      ).toStrictEqual(fixture.claimed.binding);
    } finally {
      fixture.test.session.dispose();
    }
  });
});

/**
 * Closes a held worker as the lead, then replays its late settlement.
 * @param {Function} close Builds the closing todo request for a unit ID.
 */
type Closing = (
  id: string,
) => Parameters<Awaited<ReturnType<typeof worker>>["test"]["todo"]>[typeof first];
const closeHeld = async (close: Closing): Promise<void> => {
  const fixture = await worker(),
    { binding } = fixture.claimed,
    count = (): number => fixture.test.manager.getBranch().length,
    settlement = {
      binding,
      eventId: "late",
      result: {
        evidence: { execution: binding.execution, resultId: "reply", summary: "late" },
        kind: "returned",
      },
      version: 1,
    };
  try {
    await fixture.test.todo(close(fixture.id));
    const unit = branchSnapshot(fixture.test.manager.getBranch()).tasks.at(first)?.unit,
      before = count();
    expect(unit?.binding).toBeUndefined();
    expect(unit?.released).toStrictEqual([binding.execution]);
    expect(fixture.test.provider().settle(settlement)).toStrictEqual({
      ok: true,
      value: { applied: false },
    });
    expect(count()).toBe(before);
    await fixture.test.todo({
      items: [{ content: "Local", contract: contract(), key: "local" }],
      op: "append",
      start: "local",
    });
  } finally {
    fixture.test.session.dispose();
  }
};

describe("lead closes a held worker", () => {
  it("done releases the binding and late settlements are no-ops", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    await closeHeld((task) => ({ evidence: "merged", op: "done", task }));
  });

  it("drop releases the binding and late settlements are no-ops", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    await closeHeld((task) => ({ op: "drop", task }));
  });

  it("done needs a note", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const fixture = await worker();
    try {
      await expect(fixture.test.todo({ op: "done", task: fixture.id })).rejects.toThrow(
        "ACCEPTANCE_REQUIRED",
      );
    } finally {
      fixture.test.session.dispose();
    }
  });
});
