import { arm, harness, nudge, result, turn } from "./fixture.ts";
import { describe, expect, it } from "vitest";

const ONE = 1;

describe("prewalk lifecycle", () => {
  it("resumes unarmed unless a startup flag arms", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await arm(test);
    await test.adapter.run(
      {
        flags: { into: undefined, no_prewalk: false, prewalk: false },
        kind: "start",
        reason: "resume",
      },
      test.ctx,
    );
    expect(test.branch.toReversed()).toMatchObject([
      { customType: "prewalk-off" },
      { customType: "prewalk-arm" },
    ]);
    await arm(test);
    expect(test.branch.toReversed()).toMatchObject([
      { customType: "prewalk-arm" },
      { customType: "prewalk-off" },
      { customType: "prewalk-arm" },
    ]);
  });
});

describe("new session restore", () => {
  it("restores the planner from the outgoing branch on new", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    test.todo(false);
    await nudge(test);
    await turn(test, [result("write", true)]);
    test.newBranch();
    test.config('{"enabled":true,"target":{"model":"test/worker","thinking":"medium"}}');
    await test.adapter.newSession("previous", test.ctx);
    expect(test.selections).toStrictEqual(["test/worker", "test/planner"]);
    expect(test.branch).toMatchObject([
      { customType: "prewalk-arm", data: { planner: { model: "test/planner", thinking: "high" } } },
    ]);
  });

  it("does not restore after a later thinking selection", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    test.todo(false);
    await nudge(test);
    await turn(test, [result("edit", true)]);
    test.newBranch({
      id: "changed",
      parentId: null,
      thinkingLevel: "low",
      timestamp: "",
      type: "thinking_level_change",
    });
    await test.adapter.newSession("previous", test.ctx);
    expect(test.selections).toStrictEqual(["test/worker"]);
  });
});

describe("prewalk commands", () => {
  it("restart selects the saved default and arms", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await test.adapter.run({ command: "restart", kind: "command" }, test.ctx);
    expect(test.selections).toStrictEqual(["test/default"]);
    expect(test.branch).toMatchObject([
      { customType: "prewalk-arm", data: { planner: { thinking: "low" } } },
    ]);
  });

  it("retains matching arms and rejects a mismatched restart", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await arm(test);
    await test.adapter.run({ command: "", kind: "command" }, test.ctx);
    await test.adapter.run({ command: "restart", kind: "command" }, test.ctx);
    expect(test.branch).toHaveLength(ONE);
    test.config('{"target":{"model":"test/planner","thinking":"medium"}}');
    await test.adapter.run({ command: "restart", kind: "command" }, test.ctx);
    expect(test.selections).toStrictEqual([]);
  });

  it("status includes planner, target, gate, and todo owner", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await arm(test);
    await test.adapter.run({ command: "status", kind: "command" }, test.ctx);
    expect(test.notices.join("\n")).toContain(
      "planner test/planner; target test/worker; gate closed; todo owner test/todo",
    );
  });

  it("warns and stays unarmed when target is unavailable", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    test.config('{"target":{"model":"missing/model","thinking":"medium"}}');
    await arm(test);
    expect(test.branch).toStrictEqual([]);
    expect(test.notices).toHaveLength(ONE);
  });
});
