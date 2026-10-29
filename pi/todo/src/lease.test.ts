import { describe, expect, it } from "vitest";
import { branchSnapshot } from "./state.ts";
import { first } from "./contract-fixture.ts";
import { leased } from "./lease-fixture.ts";

describe("abort leases", () => {
  it("holds until the execute promise ends after abort", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const lease = await leased(false, false);
    try {
      const aborting = lease.test.session.abort();
      await expect(lease.test.todo({ op: "drop", task: lease.id })).rejects.toThrow(
        "UNIT_ALREADY_BOUND",
      );
      await lease.finish();
      await aborting;
      await lease.test.todo({ op: "drop", task: lease.id });
      expect(branchSnapshot(lease.test.manager.getBranch()).tasks.at(first)?.status).toBe(
        "abandoned",
      );
    } finally {
      lease.close();
    }
  });
});

describe("direct leases", () => {
  it("ignores unrelated results and releases on matching end", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const lease = await leased(false, false);
    try {
      await expect(
        lease.test.todo({ evidence: "review", op: "done", task: lease.id }),
      ).rejects.toThrow("UNIT_ALREADY_BOUND");
      await lease.test.session.extensionRunner.emitToolResult({
        content: [],
        details: undefined,
        input: {},
        isError: false,
        toolCallId: "unrelated",
        toolName: "read",
        type: "tool_result",
      });
      await expect(lease.test.todo({ op: "drop", task: lease.id })).rejects.toThrow(
        "UNIT_ALREADY_BOUND",
      );
      await lease.finish();
      await lease.test.todo({ evidence: "review", op: "done", task: lease.id });
      expect(branchSnapshot(lease.test.manager.getBranch()).tasks.at(first)?.status).toBe(
        "completed",
      );
    } finally {
      lease.close();
    }
  });
});

describe("nested leases", () => {
  it("uses actual nested IDs and releases a throwing tool", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const lease = await leased(true, true);
    try {
      expect(lease.calls.at(first)).toContain("/");
      await expect(lease.test.todo({ op: "drop", task: lease.id })).rejects.toThrow(
        "UNIT_ALREADY_BOUND",
      );
      await lease.finish();
      await lease.test.todo({ evidence: "reviewed error", op: "done", task: lease.id });
      expect(branchSnapshot(lease.test.manager.getBranch()).tasks.at(first)?.status).toBe(
        "completed",
      );
    } finally {
      lease.close();
    }
  });
});
