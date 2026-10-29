import { arm, harness, nudge, result, turn } from "./fixture.ts";
import { describe, expect, it } from "vitest";
import type { ToolResultMessage } from "@earendil-works/pi-ai";
import { projectContext } from "./projection.ts";

const ONE = 1,
  NUDGE = 1;

describe("prewalk boundaries", () => {
  it("nudges before switching on nested todo and failed edit", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness(),
      nested: ToolResultMessage = Object.assign(result("codemode", false), {
        nestedCalls: {
          calls: [
            { id: "1", name: "todo", status: "ok" },
            { id: "2", name: "edit", status: "error" },
          ],
          complete: true,
        },
      });
    await arm(test);
    await turn(test, [result("read", false)]);
    expect(test.branch).toMatchObject([
      { customType: "prewalk-arm" },
      { customType: "prewalk-plan", display: false },
    ]);
    await turn(test, [nested]);
    expect(test.branch).toMatchObject([
      { customType: "prewalk-arm" },
      { customType: "prewalk-plan" },
      {
        customType: "prewalk-handoff",
        data: {
          planner: { model: "test/planner", thinking: "high" },
          target: { model: "test/worker", thinking: "medium" },
        },
      },
      { replacement: null, targetId: test.branch[NUDGE]?.id, type: "context_edit" },
      { customType: "prewalk-checklist", display: false },
    ]);
    await turn(test, [nested]);
    expect(test.selections).toStrictEqual(["test/worker"]);
  });
});

describe("nested gate", () => {
  it("keeps the gate closed for failed and unfinished nested todo", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await nudge(test);
    await turn(test, [
      Object.assign(result("codemode", false), {
        nestedCalls: {
          calls: [
            { id: "1", name: "todo", status: "error" },
            { id: "2", name: "todo", status: "unfinished" },
            { id: "3", name: "write", status: "ok" },
          ],
          complete: false,
        },
      }),
    ]);
    expect(test.selections).toStrictEqual([]);
  });

  it("continues once while armed", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await nudge(test);
    const first = await test.adapter.run({ kind: "settle" }, test.ctx, []);
    test.apply(first.entries);
    expect(first).toMatchObject({
      continue: true,
      entries: [{ customType: "prewalk-continue", display: false }],
    });
    await expect(test.adapter.run({ kind: "settle" }, test.ctx, [])).resolves.toStrictEqual({
      entries: [],
    });
  });
});

describe("prewalk cancellation", () => {
  it("off while idle removes planning context immediately", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await nudge(test);
    await test.adapter.run({ command: "off", kind: "command" }, test.ctx);
    expect(test.selections).toStrictEqual([]);
    expect(
      projectContext(test.branch, [
        {
          content: "plan",
          customType: "prewalk-plan",
          display: false,
          role: "custom",
          timestamp: 0,
        },
      ]).messages,
    ).toStrictEqual([]);
    const boundary = await test.adapter.run({ kind: "settle" }, test.ctx, []);
    expect(boundary.entries).toStrictEqual([
      { replacement: null, targetId: test.branch[NUDGE]?.id, type: "context_edit" },
    ]);
    test.apply(boundary.entries);
    await expect(test.adapter.run({ kind: "settle" }, test.ctx, [])).resolves.toMatchObject({
      entries: [],
    });
  });
});

describe("live config", () => {
  it("re-reads config at input and cancels after a live disable", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await test.adapter.run(
      {
        flags: { into: undefined, no_prewalk: false, prewalk: false },
        kind: "start",
        reason: "startup",
      },
      test.ctx,
    );
    test.config('{"enabled":true,"target":{"model":"test/worker","thinking":"low"}}');
    await test.adapter.run({ kind: "input" }, test.ctx);
    expect(test.branch.toReversed()).toMatchObject([
      { customType: "prewalk-arm" },
      { customType: "prewalk-off" },
    ]);
    test.config('{"enabled":false}');
    await test.adapter.run({ kind: "input" }, test.ctx);
    expect(test.branch.toReversed()).toMatchObject([
      { customType: "prewalk-off" },
      { customType: "prewalk-arm" },
      { customType: "prewalk-off" },
    ]);
  });
});

describe("config errors", () => {
  it("reports a config error only once", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    test.config("{");
    await test.adapter.run({ kind: "input" }, test.ctx);
    await test.adapter.run({ kind: "input" }, test.ctx);
    expect(test.notices).toHaveLength(ONE);
  });
});
