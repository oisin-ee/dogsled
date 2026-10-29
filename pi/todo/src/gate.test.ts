import { describe, expect, it } from "vitest";
import {
  emptyParameters,
  emptyResult,
  runtimeFixture,
  selectableFixture,
} from "./runtime-fixture.ts";
import { first, firstId, last } from "./contract-fixture.ts";
import type { ExtensionFactory } from "@earendil-works/pi-coding-agent";
import { branchSnapshot } from "./state.ts";

/**
 * Reads finalized transcript results.
 * @param {Readonly<RuntimeFixture>} test Real runtime fixture.
 * @returns {readonly unknown[]} Final results.
 */
const results = (test: {
    readonly session: { readonly messages: readonly { readonly role: string }[] };
  }): readonly unknown[] =>
    test.session.messages.filter((message) => message.role === "toolResult"),
  /**
   * Adds a guard after pi-todo and records actual result-hook delivery.
   * @param {string[]} seen Observed invocation IDs.
   * @returns {ExtensionFactory} Runtime guard.
   */
  laterGuard =
    (seen: Readonly<Pick<string[], "push">>): ExtensionFactory =>
    (pi) => {
      pi.on("tool_call", (event: Readonly<{ toolName: string }>) =>
        event.toolName === "bash" ? { block: true, reason: "Later guard" } : undefined,
      );
      pi.on("tool_result", (event: Readonly<{ toolName: string; toolCallId: string }>) => {
        if (event.toolName === "bash") {
          seen.push(event.toolCallId);
        }
      });
    };

describe("todo owner collision", () => {
  it("leaves another owner's calls and capability unchanged", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const test = await runtimeFixture((pi) => {
      pi.registerTool({
        description: "Other owner",
        execute: async () => emptyResult(),
        label: "Other",
        name: "todo",
        parameters: emptyParameters,
      });
    }, true);
    try {
      expect(() => test.provider()).toThrow("exactly one synchronous provider");
      await test.call("bash", { command: "printf collision" });
      expect(results(test).at(last)).toMatchObject({ isError: false });
    } finally {
      test.session.dispose();
    }
  });
});

describe("missing activation", () => {
  it("allows Dogsled completion while keeping shell blocked", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    let ran = false;
    const test = await runtimeFixture((pi) => {
      pi.registerTool({
        description: "Report worker completion",
        execute: async () => {
          ran = true;
          return emptyResult();
        },
        label: "Done",
        name: "mcp__dogsled__done",
        parameters: emptyParameters,
      });
    });
    try {
      await test.call("mcp__dogsled__done", {});
      expect(ran).toBe(true);
      expect(results(test).at(last)).toMatchObject({ isError: false });

      await test.call("bash", { command: "printf should-not-run" });
      expect(JSON.stringify(results(test).at(last))).toContain(
        "Start a complete self-assigned leaf with todo before shell, code, mutation, or unknown tools.",
      );
    } finally {
      test.session.dispose();
    }
  });

  it("blocks direct and nested shell without local work", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const test = await runtimeFixture();
    try {
      await test.call("bash", { command: "printf should-not-run" });
      expect(results(test)).toMatchObject([{ isError: true }]);
      await test.call("codemode", {
        code: 'return await tools.bash({command:"printf should-not-run"})',
      });
      expect(JSON.stringify(results(test))).toContain("Start a complete self-assigned leaf");
      expect(test.provider().imported()).toStrictEqual({ ok: true, value: null });
    } finally {
      test.session.dispose();
    }
  });
});

describe("structured reads", () => {
  it("allows explicit reads but rejects unknown MCP suffixes", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    let ran = false;
    const test = await runtimeFixture((pi) => {
      pi.registerTool({
        description: "Unknown mutation",
        execute: async () => {
          ran = true;
          return emptyResult();
        },
        label: "Unknown",
        name: "mcp__other__read",
        parameters: emptyParameters,
      });
    });
    try {
      await test.call("mcp__other__read", {});
      expect(ran).toBe(false);
      await test.call("read", { path: new URL("index.ts", import.meta.url).pathname });
      expect(results(test).at(last)).toMatchObject({ isError: false });
    } finally {
      test.session.dispose();
    }
  });
});

describe("local-only execution", () => {
  it("requires no external executor", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const test = await runtimeFixture();
    try {
      await test.create();
      expect(test.provider().active()).toMatchObject({ ok: true, value: { seeded: false } });
      await test.call("bash", { command: "printf allowed" });
      expect(results(test).at(last)).toMatchObject({ isError: false });
    } finally {
      test.session.dispose();
    }
  });
});

describe("planning selections", () => {
  it.each(["plan-claude", "plan-gpt"])(
    "allows direct shell under tier/%s without activation",
    { timeout: 20_000 },
    async (id) => {
      expect.hasAssertions();
      const test = await selectableFixture([{ id, kind: "virtual", provider: "tier" }]);
      try {
        await test.select("tier", id);
        await test.call("bash", { command: "printf planner" });
        expect(results(test).at(last)).toMatchObject({ isError: false });
        expect(test.provider().active()).toMatchObject({ ok: true, value: null });
      } finally {
        test.session.dispose();
      }
    },
  );

  it.each(["plan-claude", "plan-gpt"])(
    "allows nested shell under tier/%s without activation",
    { timeout: 20_000 },
    async (id) => {
      expect.hasAssertions();
      const test = await selectableFixture([{ id, kind: "virtual", provider: "tier" }]);
      try {
        await test.select("tier", id);
        await test.call("codemode", {
          code: 'return await tools.bash({command:"printf planner"})',
        });
        expect(JSON.stringify(results(test))).not.toContain("Start a complete self-assigned leaf");
        expect(results(test).at(last)).toMatchObject({ isError: false });
      } finally {
        test.session.dispose();
      }
    },
  );
});

describe("planning near matches", () => {
  it.each([
    ["another tier selection", { id: "exec-claude", kind: "virtual", provider: "tier" }],
    [
      "the same id under another provider",
      { id: "plan-claude", kind: "virtual", provider: "faux" },
    ],
    [
      "a display name that only looks like a planner",
      { id: "Plan-Claude", kind: "virtual", provider: "tier" },
    ],
    [
      "a physical model that only looks like a planner",
      { id: "plan-claude", kind: "physical", provider: "faux" },
    ],
  ] as const)("keeps %s gated", { timeout: 20_000 }, async (_label, selection) => {
    expect.hasAssertions();
    const test = await selectableFixture([selection]);
    try {
      await test.select(selection.provider, selection.id);
      await test.call("bash", { command: "printf gated" });
      expect(JSON.stringify(results(test).at(last))).toContain(
        "Start a complete self-assigned leaf",
      );
      expect(results(test).at(last)).toMatchObject({ isError: true });
    } finally {
      test.session.dispose();
    }
  });
});

describe("planning selection changes", () => {
  it("re-gates work after the selection leaves a planner", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const test = await selectableFixture([
      { id: "plan-claude", kind: "virtual", provider: "tier" },
    ]);
    try {
      await test.select("tier", "plan-claude");
      await test.call("bash", { command: "printf planner" });
      expect(results(test).at(last)).toMatchObject({ isError: false });
      await test.select("faux", "faux-1");
      await test.call("bash", { command: "printf writer" });
      expect(JSON.stringify(results(test).at(last))).toContain(
        "Start a complete self-assigned leaf",
      );
    } finally {
      test.session.dispose();
    }
  });

  it("allows a planner selection after a gated selection", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const test = await selectableFixture([
      { id: "plan-claude", kind: "virtual", provider: "tier" },
    ]);
    try {
      await test.call("bash", { command: "printf writer" });
      expect(results(test).at(last)).toMatchObject({ isError: true });
      await test.select("tier", "plan-claude");
      await test.call("bash", { command: "printf planner" });
      expect(results(test).at(last)).toMatchObject({ isError: false });
    } finally {
      test.session.dispose();
    }
  });
});

describe("later guard blocks", () => {
  it("releases direct and nested leases without tool_result", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const seen: string[] = [],
      test = await runtimeFixture(laterGuard(seen));
    try {
      await test.create();
      await test.call("bash", { command: "printf blocked" });
      await test.call("codemode", { code: 'return await tools.bash({command:"printf blocked"})' });
      const id = firstId(branchSnapshot(test.manager.getBranch()));
      await test.call("todo", { evidence: "Reviewed blocked execution", op: "done", task: id });
      expect([
        seen,
        branchSnapshot(test.manager.getBranch()).tasks.at(first)?.status,
      ]).toStrictEqual([[], "completed"]);
    } finally {
      test.session.dispose();
    }
  });
});
