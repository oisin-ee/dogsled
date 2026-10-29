import { contract, first, firstId, fixture, rootId, second, third } from "./contract-fixture.ts";
import { describe, expect, it } from "vitest";
import { parse } from "./wasm.ts";

describe("contract prerequisites", () => {
  it(
    "resolves local keys and requires completed rather than dropped prerequisites",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const test = fixture(),
        list = test.call({
          items: [
            { content: "First", contract: contract(), key: "first" },
            {
              content: "Second",
              contract: Object.assign(contract(), { dependsOn: ["first"] }),
              key: "second",
            },
          ],
          op: "init",
          start: "first",
        }),
        initialId = firstId(list),
        nextId = rootId(list, second);
      expect(list.tasks.at(second)?.unit?.draft.dependsOn).toStrictEqual([initialId]);
      expect(() => test.call({ op: "start", task: nextId })).toThrow("DEPENDENCY_OPEN");
      test.call({ evidence: "tests passed", op: "done", task: initialId });
      expect(test.call({ op: "start", task: nextId }).tasks.at(second)?.status).toBe("in_progress");
      test.call({ op: "drop", task: nextId });
      const added = test.call({
          items: [
            {
              content: "Third",
              contract: Object.assign(contract(), { dependsOn: [nextId] }),
              key: "third",
            },
          ],
          op: "append",
        }),
        addedId = rootId(added, third);
      expect(() => test.call({ op: "start", task: addedId })).toThrow("DEPENDENCY_OPEN");
    },
  );
});

describe("dependency graph rejection", () => {
  it.each([{ dependsOn: ["missing"] }, { dependsOn: ["first"] }])(
    "rejects unknown or cyclic prerequisites %j atomically",
    { timeout: 5000 },
    ({ dependsOn }: { readonly dependsOn: readonly string[] }) => {
      expect.hasAssertions();
      const test = fixture();
      expect(() =>
        test.call({
          items: [
            { content: "First", contract: Object.assign(contract(), { dependsOn }), key: "first" },
          ],
          op: "init",
        }),
      ).toThrow(/UNIT_NOT_FOUND|DEPENDENCY_CYCLE/u);
      expect(test.entries).toStrictEqual([]);
    },
  );

  it("rejects two-node cycles and duplicate runtime identities", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    expect(() =>
      test.call({
        items: [
          {
            content: "First",
            contract: Object.assign(contract(), { dependsOn: ["second"] }),
            key: "first",
          },
          {
            content: "Second",
            contract: Object.assign(contract(), { dependsOn: ["first"] }),
            key: "second",
          },
        ],
        op: "init",
      }),
    ).toThrow("DEPENDENCY_CYCLE");
    const list = test.call({ items: ["First"], op: "init" }),
      task = list.tasks.at(first);
    expect(() => parse({ tasks: [task, Object.assign({}, task, { content: "Second" })] })).toThrow(
      "INVALID_IDENTITY",
    );
  });
});

describe("scope and assignment", () => {
  it.each(["/absolute", "../escape", "src/../escape", "src/**", "src//file", String.raw`C:\root`])(
    "rejects ownership path %s",
    { timeout: 5000 },
    (path) => {
      expect.hasAssertions();
      const test = fixture(),
        draft = Object.assign(contract(), {
          scope: { generates: [], owns: [path], reads: [], repositoryId: "repo" },
        });
      expect(() =>
        test.call({ items: [{ content: "Build", contract: draft }], op: "init" }),
      ).toThrow("INVALID_SCOPE");
      expect(test.entries).toStrictEqual([]);
    },
  );
});

describe("concurrent scope", () => {
  it(
    "rejects duplicate ownership and overlapping or disjoint concurrent local work",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const test = fixture();
      expect(() =>
        test.call({
          items: [
            {
              content: "Bad",
              contract: Object.assign(contract(), {
                scope: { generates: [], owns: ["src", "src/"], reads: [], repositoryId: "repo" },
              }),
            },
          ],
          op: "init",
        }),
      ).toThrow("duplicate path");
      const list = test.call({
          items: [
            { content: "First", contract: contract(), key: "first" },
            { content: "Second", contract: contract() },
          ],
          op: "init",
          start: "first",
        }),
        nextId = rootId(list, second);
      expect(() => test.call({ op: "start", task: nextId })).toThrow("LOCAL_UNIT_ACTIVE");
      test.call({
        contract: Object.assign(contract(), {
          scope: { generates: [], owns: [], reads: ["src"], repositoryId: "repo" },
        }),
        op: "update",
        task: nextId,
      });
      expect(() => test.call({ op: "start", task: nextId })).toThrow("LOCAL_UNIT_ACTIVE");
    },
  );
});

describe("profile assignments", () => {
  it("retains worker profile intent without activating or launching it", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      list = test.call({
        items: [
          {
            content: "Worker",
            contract: Object.assign(contract(), {
              assignment: { kind: "worker" as const, profile: "general" },
            }),
          },
        ],
        op: "init",
      });
    expect(() => test.call({ op: "start", task: firstId(list) })).toThrow("WRONG_ASSIGNMENT");
    expect(list.tasks.at(first)?.unit?.draft.assignment).toStrictEqual({
      kind: "worker",
      profile: "general",
    });
    expect(test.entries).toHaveLength(second);
  });
});
