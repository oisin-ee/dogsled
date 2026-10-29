import { describe, expect, it } from "vitest";
import { emptyParameters, emptyResult, runtimeFixture } from "./runtime-fixture.ts";
import { first } from "./contract-fixture.ts";
import { worker } from "./provider-fixture.ts";

describe("explicit structured reads", () => {
  it.each([
    "ctx_search",
    "ctx_stats",
    "mcp__codebase_memory__search_graph",
    "mcp__codebase_memory__get_code_snippet",
    "mcp__codebase_memory__trace_path",
    "mcp__codebase_memory__detect_changes",
  ])("permits %s without local activation", { timeout: 20_000 }, async (name) => {
    expect.hasAssertions();
    let executed = false;
    const test = await runtimeFixture((pi) => {
      pi.registerTool({
        description: "Structured read",
        execute: async () => {
          executed = true;
          return emptyResult();
        },
        label: "Read",
        name,
        parameters: emptyParameters,
      });
    });
    try {
      await test.call(name, {});
      expect(executed).toBe(true);
    } finally {
      test.session.dispose();
    }
  });
});

describe("execution and index operations", () => {
  it.each([
    "ctx_execute",
    "ctx_execute_file",
    "ctx_index",
    "lsp_fix",
    "mcp__codebase_memory__index_repository",
  ])("rejects %s without local activation", { timeout: 20_000 }, async (name) => {
    expect.hasAssertions();
    let executed = false;
    const test = await runtimeFixture((pi) => {
      pi.registerTool({
        description: "Execution",
        execute: async () => {
          executed = true;
          return emptyResult();
        },
        label: "Execute",
        name,
        parameters: emptyParameters,
      });
    });
    try {
      await test.call(name, {});
      expect(executed).toBe(false);
      expect(
        test.session.messages.filter((message) => message.role === "toolResult").at(first),
      ).toMatchObject({ isError: true });
    } finally {
      test.session.dispose();
    }
  });
});

describe("delegated activation", () => {
  it("cannot authorize lead shell execution", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const fixture = await worker();
    try {
      await fixture.test.call("bash", { command: "printf forbidden" });
      expect(
        fixture.test.session.messages.filter((message) => message.role === "toolResult").at(first),
      ).toMatchObject({ isError: true });
    } finally {
      fixture.test.session.dispose();
    }
  });
});
