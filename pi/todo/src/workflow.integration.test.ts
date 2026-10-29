import { describe, expect, it } from "vitest";
import { branchSnapshot } from "./state.ts";
import { importFixture } from "./import-fixture.ts";
import { runtimeFixture } from "./runtime-fixture.ts";
import { value } from "./provider-fixture.ts";

/**
 * Requires a persisted import entry before a branch test.
 * @param {string | null} id Current branch leaf.
 * @returns {string} Known entry identity.
 */
const leaf = (id: string | null): string => {
    if (id === null) {
      throw new Error("Missing import entry");
    }
    return id;
  },
  one = 1;

describe("automatic worker import", () => {
  it(
    "activates the exact provider export without a model bookkeeping call",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const fixture = await importFixture(),
        test = await runtimeFixture(undefined, false, fixture);
      try {
        expect(test.session.messages).toStrictEqual([]);
        expect(value(test.provider().active())).toMatchObject({
          ref: fixture.origin.inspected.ref,
          seeded: true,
        });
        expect(value(test.provider().inspect(fixture.origin.id))).toStrictEqual(
          fixture.origin.inspected,
        );
        await test.call("bash", { command: "printf imported" });
        expect(
          test.session.messages.filter((message) => message.role === "toolResult").at(-one),
        ).toMatchObject({ isError: false });
      } finally {
        test.session.dispose();
        await fixture.close();
      }
    },
  );
});

describe("worker completion receipt", () => {
  it(
    "seals normal completion and resumes without the seed artifact",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const fixture = await importFixture(),
        test = await runtimeFixture(undefined, false, fixture);
      try {
        await test.todo({
          evidence: "Reviewed worker changes",
          op: "done",
          task: fixture.origin.id,
        });
        const receipt = value(test.provider().imported());
        await fixture.removeSeed();
        await test.session.extensionRunner.emit({ reason: "reload", type: "session_start" });
        expect({
          active: value(test.provider().active()),
          receipt: value(test.provider().imported()),
        }).toStrictEqual({ active: null, receipt });
      } finally {
        test.session.dispose();
        await fixture.close();
      }
    },
  );
});

describe("immutable worker scope", () => {
  it("rejects branch rewinds around a completion seal", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const fixture = await importFixture(),
      test = await runtimeFixture(undefined, false, fixture),
      originLeaf = leaf(test.manager.getLeafId());
    try {
      await test.todo({ evidence: "Reviewed", op: "done", task: fixture.origin.id });
      expect(value(test.provider().imported())).toMatchObject({
        seal: { disposition: "completed", entryId: test.manager.getLeafId() },
      });
      test.manager.branch(originLeaf);
      expect(test.provider().active()).toMatchObject({
        error: { code: "SEEDED_SCOPE_LOCKED" },
        ok: false,
      });
    } finally {
      test.session.dispose();
      await fixture.close();
    }
  });

  it(
    "rejects a seed-free branch without granting replacement scope",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const fixture = await importFixture(),
        test = await runtimeFixture(undefined, false, fixture);
      try {
        test.manager.resetLeaf();
        expect(test.provider().active()).toMatchObject({
          error: { code: "SEEDED_SCOPE_LOCKED" },
          ok: false,
        });
        await expect(test.todo({ items: ["Escape"], op: "init" })).rejects.toThrow(
          "SEEDED_SCOPE_LOCKED",
        );
      } finally {
        test.session.dispose();
        await fixture.close();
      }
    },
  );
});

describe("import rejection", () => {
  it(
    "rejects another repository without appending or authorizing work",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const fixture = await importFixture("github.com/other/oisinpi"),
        test = await runtimeFixture(undefined, false, fixture);
      try {
        expect(value(test.provider().imported())).toBeNull();
        expect(branchSnapshot(test.manager.getBranch()).tasks).toStrictEqual([]);
        await test.call("bash", { command: "printf forbidden" });
        expect(
          test.session.messages.filter((message) => message.role === "toolResult").at(-one),
        ).toMatchObject({ isError: true });
      } finally {
        test.session.dispose();
        await fixture.close();
      }
    },
  );

  it(
    "does not automatically import the old seed into a new session",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const fixture = await importFixture(),
        test = await runtimeFixture(undefined, false, fixture);
      try {
        test.manager.newSession();
        await test.session.extensionRunner.emit({ reason: "new", type: "session_start" });
        expect(value(test.provider().imported())).toBeNull();
        expect(value(test.provider().active())).toBeNull();
      } finally {
        test.session.dispose();
        await fixture.close();
      }
    },
  );
});
