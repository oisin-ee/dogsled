import { describe, expect, it } from "vitest";
import { harness } from "./fixture.ts";
import { parse_state } from "./wasm.ts";

const ONE = 1;

describe("startup flags", () => {
  it("no-prewalk suppresses enabled only until new", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    test.config('{"enabled":true,"target":{"model":"test/worker","thinking":"medium"}}');
    await test.adapter.run(
      {
        flags: { into: undefined, no_prewalk: true, prewalk: false },
        kind: "start",
        reason: "startup",
      },
      test.ctx,
    );
    await test.adapter.run({ kind: "input" }, test.ctx);
    expect(test.branch).toMatchObject([{ customType: "prewalk-off" }]);
    test.newBranch();
    await test.adapter.newSession("old", test.ctx);
    expect(test.branch).toMatchObject([{ customType: "prewalk-arm" }]);
  });

  it("into overrides config and arms a startup resume", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await test.adapter.run(
      {
        flags: { into: "test/default", no_prewalk: false, prewalk: false },
        kind: "start",
        reason: "startup",
      },
      test.ctx,
    );
    expect(test.branch).toMatchObject([
      { customType: "prewalk-arm", data: { target: { model: "test/default" } } },
    ]);
    await test.adapter.run(
      {
        flags: { into: undefined, no_prewalk: false, prewalk: false },
        kind: "start",
        reason: "startup",
      },
      test.ctx,
    );
    expect(test.branch.toReversed()).toMatchObject([
      { customType: "prewalk-off" },
      { customType: "prewalk-arm" },
    ]);
  });
});

describe("flag conflicts", () => {
  it("conflicting flags warn and stay unarmed", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await test.adapter.run(
      {
        flags: { into: "test/worker", no_prewalk: true, prewalk: false },
        kind: "start",
        reason: "startup",
      },
      test.ctx,
    );
    expect(test.branch).toMatchObject([{ customType: "prewalk-off" }]);
    expect(test.notices).toHaveLength(ONE);
  });
});

describe("state validation and no-op", () => {
  it("rejects a malformed stored arm in Rust", { timeout: 5000 }, () => {
    expect.hasAssertions();
    expect(() => parse_state({ kind: "arm" })).toThrow("missing field");
  });

  it("same model and thinking disarms without switching", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    test.config('{"target":{"model":"test/planner","thinking":"high"}}');
    await test.adapter.run({ command: "", kind: "command" }, test.ctx);
    expect(test.branch).toMatchObject([{ customType: "prewalk-off" }]);
    expect(test.selections).toStrictEqual([]);
  });
});
