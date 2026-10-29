import { describe, expect, it } from "vitest";
import { createEventBus } from "@earendil-works/pi-coding-agent";
import { registerSeedSource } from "./seed-fixture.ts";
import { seedPath } from "./seed-transport.ts";

/**
 * Supplies transport responses through the actual synchronous event bus.
 * @param {readonly unknown[]} responses Executor responses.
 * @returns {ReturnType<typeof createEventBus>} Fixture bus.
 */
const transport = (responses: readonly unknown[]): ReturnType<typeof createEventBus> => {
  const events = createEventBus();
  for (const response of responses) {
    registerSeedSource(events, response);
  }
  return events;
};

describe("executor seed transport", () => {
  it("does not import without a managed source", { timeout: 1000 }, () => {
    expect.hasAssertions();
    expect(seedPath(transport([]))).toBeUndefined();
    expect(seedPath(transport([{ path: null, version: 1 }]))).toBeUndefined();
  });

  it("reads the single executor source", { timeout: 1000 }, () => {
    expect.hasAssertions();
    expect(seedPath(transport([{ path: "/seed.json", version: 1 }]))).toBe("/seed.json");
  });

  it.each([
    [null],
    [{ path: "/seed.json", version: 2 }],
    [{ path: "", version: 1 }],
    [{ path: 1, version: 1 }],
    [{ extra: true, path: "/seed.json", version: 1 }],
    [
      { path: "/one", version: 1 },
      { path: "/two", version: 1 },
    ],
  ])(
    "rejects malformed or ambiguous transport %j",
    { timeout: 1000 },
    (...responses: readonly unknown[]) => {
      expect.hasAssertions();
      expect(() => seedPath(transport(responses))).toThrow("IMPORT_FAILED");
    },
  );
});
