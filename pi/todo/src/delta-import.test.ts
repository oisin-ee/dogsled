import { claimScenario, sealScenario } from "./delta-worker-fixture.ts";
import { describe, expect, it } from "vitest";
import { DELTA_ENTRY } from "./state.ts";

const one = 1;

describe("delta provider persistence", () => {
  it("claims and settles without moving revision origins", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const scenario = await claimScenario();
    expect(scenario.actual).toMatchObject({
      claimEntry: { customType: DELTA_ENTRY },
      evidence: one,
      origin: scenario.origin,
      settleEntry: { customType: DELTA_ENTRY },
      settled: { ok: true, value: { applied: true } },
      task: { status: "in_progress" },
    });
  });
});

describe("delta import protection", () => {
  it("finds a delta-only seal on an abandoned branch", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const scenario = await sealScenario();
    expect(scenario.receipt).toMatchObject({ seal: { entryId: scenario.sealId } });
    expect(scenario.rewound).toMatchObject({
      active: { error: { code: "SEEDED_SCOPE_LOCKED" }, ok: false },
      imported: { error: { code: "SEEDED_SCOPE_LOCKED" }, ok: false },
    });
  });
});
