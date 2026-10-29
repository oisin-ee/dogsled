import { describe, expect, it } from "vitest";
import { harness, nudge, result, turn } from "./fixture.ts";
import type { Harness } from "./fixture.ts";

interface Acquisition {
  readonly reply: (provider: unknown) => void;
}
/**
 * Validates a capability acquisition.
 * @param {unknown} request Event payload.
 * @returns {boolean} Whether the callback is callable.
 */
const acquisition = (request: unknown): request is Acquisition =>
    typeof request === "object" &&
    request !== null &&
    "reply" in request &&
    typeof request.reply === "function",
  /**
   * Supplies a synchronous extension capability, not a forged todo result.
   * @param {Harness} test Runtime test host.
   * @param {boolean} active Whether a validated local leaf exists.
   */
  activation = (test: Harness, active: boolean): void => {
    test.events.on("oisin.tasks.acquire.v1", (request: unknown) => {
      if (acquisition(request)) {
        request.reply({ active: () => ({ ok: true, value: active ? { seeded: true } : null }) });
      }
    });
  };

describe("provider activation evidence", () => {
  it("hands off only after the edit, without a worker todo call", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    activation(test, true);
    await nudge(test);
    expect(test.selections).toStrictEqual([]);
    await turn(test, [result("read", false)]);
    expect(test.selections).toStrictEqual([]);
    await turn(test, [result("edit", false)]);
    expect(test.selections).toStrictEqual(["test/worker"]);
    expect(test.branch).toContainEqual(
      expect.objectContaining({ customType: "prewalk-handoff", type: "custom" }),
    );
  });

  it(
    "does not treat a successful todo view as activation when the provider is present",
    { timeout: 5000 },
    async () => {
      expect.hasAssertions();
      const test = harness();
      activation(test, false);
      await nudge(test);
      await turn(test, [result("todo", false), result("write", false)]);
      expect(test.selections).toStrictEqual([]);
    },
  );

  it("preserves successful-todo fallback without the provider", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    await nudge(test);
    await turn(test, [result("todo", false), result("edit", false)]);
    expect(test.selections).toStrictEqual(["test/worker"]);
  });

  it("does not arm a disabled prewalk from task activation", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness();
    activation(test, true);
    await turn(test, [result("edit", false)]);
    expect(test.selections).toStrictEqual([]);
  });
});
