import { describe, expect, it } from "vitest";
import { branchSnapshot } from "./state.ts";
import { first } from "./contract-fixture.ts";
import { leased } from "./lease-fixture.ts";

describe("branch-local invocation leases", () => {
  it("does not hold unrelated work on another branch", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const lease = await leased(true, false);
    try {
      await lease.unrelated();
      expect(branchSnapshot(lease.test.manager.getBranch()).tasks.at(first)?.status).toBe(
        "completed",
      );
      lease.revisit();
      await expect(lease.test.todo({ op: "drop", task: lease.id })).rejects.toThrow(
        "UNIT_ALREADY_BOUND",
      );
      await lease.finish();
    } finally {
      lease.close();
    }
  });
});

describe("return to leased origin", () => {
  it("blocks closure until the matching execution ends", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const lease = await leased(true, false);
    try {
      lease.revisit();
      await expect(
        lease.test.todo({ evidence: "Premature review", op: "done", task: lease.id }),
      ).rejects.toThrow("UNIT_ALREADY_BOUND");
      await expect(lease.test.todo({ op: "drop", task: lease.id })).rejects.toThrow(
        "UNIT_ALREADY_BOUND",
      );
      await lease.finish();
      await lease.test.todo({ evidence: "Reviewed finished call", op: "done", task: lease.id });
      expect(branchSnapshot(lease.test.manager.getBranch()).tasks.at(first)?.status).toBe(
        "completed",
      );
    } finally {
      lease.close();
    }
  });
});
