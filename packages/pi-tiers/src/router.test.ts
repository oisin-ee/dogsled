import { describe, expect, it } from "vitest";
import { Harness } from "./fixture.ts";

const TARGETS = ["proxy/gpt-first", "proxy/gpt-second", "proxy/claude-next", "proxy/minimax-last"],
  FIRST = 0,
  SECOND = 1,
  THIRD = 2,
  COUNT = 3,
  LAST = -1,
  BACKOFF = 2000,
  UNLISTED = "proxy/gpt-first";

describe("gateway and quota recovery", () => {
  it("opens a gateway circuit without cooling models", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    await test.dispatch();
    await expect(test.settle("503 Bad Gateway")).resolves.toBe(true);
    expect(test.health.pools).toStrictEqual({});
    expect(test.health.circuits["proxy"]?.failures).toBe(SECOND);
    await expect(test.dispatch("continuation")).resolves.toBe(TARGETS[FIRST]);
    expect(test.waits.at(LAST)).toBe(BACKOFF);
  });

  it("excludes the quota pool and appends only recovery drafts", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    await test.dispatch();
    await expect(test.settle("You have hit your ChatGPT usage limit")).resolves.toBe(true);
    await expect(test.dispatch("continuation")).resolves.toBe(TARGETS[THIRD]);
    expect(test.health.pools["codex"]?.until).toBeGreaterThan(test.now);
    expect(test.branch.filter((entry) => entry.type === "context_edit")).toMatchObject([
      { replacement: null },
    ]);
  });
});

describe("native retry integration", () => {
  it("uses native retries first for overloaded and then recovers", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    await test.native("Codex error: Our servers are currently overloaded");
    expect(test.dispatched).toStrictEqual([TARGETS[FIRST], TARGETS[FIRST]]);
    await expect(test.settle("Codex error: Our servers are currently overloaded")).resolves.toBe(
      true,
    );
    await expect(test.dispatch("continuation")).resolves.toBe(TARGETS[SECOND]);
  });

  it("skips a 429 pool during native retry", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    await test.native("rate_limit 429");
    expect(test.dispatched).toStrictEqual([TARGETS[FIRST], TARGETS[THIRD]]);
  });
});

describe("terminal failures", () => {
  it("stops for unknown models and aborts", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    await test.dispatch();
    await expect(test.settle("unknown model")).resolves.toBe(false);
    await expect(test.settle("HTTP 503", "aborted")).resolves.toBe(false);
    test.abort.abort();
    await expect(test.settle("HTTP 429")).resolves.toBe(false);
    expect(test.health).toStrictEqual({ circuits: {}, pools: {} });
  });

  it("bounds recovery per user turn", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    await test.dispatch();
    await expect(test.settle("HTTP 503")).resolves.toBe(true);
    await test.dispatch("user", false);
    await expect(test.settle("HTTP 503")).resolves.toBe(true);
    await test.dispatch("user", false);
    await expect(test.settle("HTTP 503")).resolves.toBe(false);
    expect(test.dispatched).toHaveLength(COUNT);
  });
});

describe("unlisted targets", () => {
  it("skips a model the registry does not list", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    test.unlisted = [UNLISTED];
    await expect(test.dispatch()).resolves.toBe(TARGETS[SECOND]);
    await expect(test.settle("You have hit your ChatGPT usage limit")).resolves.toBe(true);
    await expect(test.dispatch("continuation")).resolves.toBe(TARGETS[THIRD]);
    expect(test.health.circuits).toStrictEqual({});
  });

  it(
    "leaves a persisted choice once its model is no longer listed",
    { timeout: 5000 },
    async () => {
      expect.hasAssertions();
      const test = new Harness(TARGETS);
      await expect(test.dispatch()).resolves.toBe(UNLISTED);
      test.unlisted = [UNLISTED];
      await expect(test.dispatch("continuation")).resolves.toBe(TARGETS[SECOND]);
    },
  );

  it("dispatches the registry's id when only the case differs", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(["proxy/minimax-m3.1-flash", "proxy/gpt-next"]);
    test.registry = ["proxy/MiniMax-M3.1-Flash", "proxy/gpt-next"];
    await expect(test.dispatch()).resolves.toBe("proxy/MiniMax-M3.1-Flash");
    await expect(test.settle("HTTP 429 rate_limit")).resolves.toBe(true);
    expect(test.health.pools["minimax"]?.until).toBeGreaterThan(test.now);
    await expect(test.dispatch("continuation")).resolves.toBe("proxy/gpt-next");
  });

  it("stops when no tier target is listed", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = new Harness(TARGETS);
    test.unlisted = TARGETS;
    await expect(test.dispatch()).rejects.toThrow("No available target in tier.");
  });
});

describe("provider server errors", () => {
  it(
    "moves to the next target when one provider fails server-side",
    { timeout: 5000 },
    async () => {
      expect.hasAssertions();
      const test = new Harness(["proxy/minimax-m3.1-flash", "proxy/gpt-next"]);
      await expect(test.dispatch()).resolves.toBe("proxy/minimax-m3.1-flash");
      await expect(
        test.settle("empty_stream: upstream stream closed before first payload"),
      ).resolves.toBe(true);
      await expect(test.dispatch("continuation")).resolves.toBe("proxy/gpt-next");
    },
  );
});

describe("quota preflight", () => {
  it(
    "skips exhausted snapshot pools before dispatch and caches the snapshot",
    { timeout: 5000 },
    async () => {
      expect.hasAssertions();
      const test = new Harness(TARGETS);
      test.quota = JSON.stringify({
        pools: [{ account: "one", pool: "codex", resets_ms: 100_000, used_pct: 100 }],
        version: 1,
      });
      await expect(test.dispatch()).resolves.toBe(TARGETS[THIRD]);
      await expect(test.dispatch("continuation")).resolves.toBe(TARGETS[THIRD]);
      expect(test.quotaReads).toBe(SECOND);
    },
  );
});
