import { describe, expect, it } from "vitest";
import { emptyHealth, parseHealth, updateHealth } from "./health.ts";

const NOW = 10_000,
  FIRST_QUOTA = 310_000,
  SECOND_QUOTA = 610_000,
  RESET = 999_000;

describe("shared health policy", () => {
  it("doubles quota cooldowns without affecting provider circuits", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const first = updateHealth(emptyHealth(), {
        key: "codex",
        kind: "quota",
        now: NOW,
        reset: undefined,
      }),
      second = updateHealth(first, { key: "codex", kind: "quota", now: NOW, reset: undefined });
    expect(first.pools["codex"]?.until).toBe(FIRST_QUOTA);
    expect(second.pools["codex"]?.until).toBe(SECOND_QUOTA);
    expect(second.circuits).toStrictEqual({});
    expect(parseHealth(JSON.stringify(second))).toStrictEqual(second);
  });

  it("uses a snapshot reset instead of the default delay", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const health = updateHealth(emptyHealth(), {
      key: "codex",
      kind: "quota",
      now: NOW,
      reset: RESET,
    });
    expect(health.pools["codex"]?.until).toBe(RESET);
    expect(() => parseHealth("{}")).toThrow("Invalid pi-tiers health state");
  });
});
