import { contract, first, firstId, fixture, second } from "./contract-fixture.ts";
import { describe, expect, it } from "vitest";
import { branchSnapshot } from "./state.ts";
import { runTodo } from "./tool.ts";

describe("executable todo contracts", () => {
  it("creates and starts atomically with stable runtime provenance", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      list = test.call({
        items: [{ content: "Build", contract: contract(), key: "build" }],
        op: "init",
        start: "build",
      }),
      id = firstId(list),
      origin = test.entries.at(first)?.id;
    expect(list.tasks.at(first)?.unit).toMatchObject({
      localSessionId: "test-session",
      reference: { contractEntryId: origin, revision: 1, sessionId: "test-session", unitId: id },
    });
    expect(test.entries).toMatchObject([
      {
        data: { tasks: [{ unit: { reference: { contractEntryId: "" } } }] },
      },
    ]);
    test.call({ evidence: "moon passed", op: "done", task: id });
    expect(branchSnapshot(test.entries).tasks.at(first)?.unit?.reference.contractEntryId).toBe(
      origin,
    );
    expect(test.call({ op: "view" }).tasks.at(first)?.status).toBe("completed");
  });
});

describe("lead closure", () => {
  it("completes an unstarted leaf with a note and requires the note", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      id = firstId(test.call({ items: [{ content: "Build", contract: contract() }], op: "init" }));
    expect(() => test.call({ op: "done", task: id })).toThrow("ACCEPTANCE_REQUIRED");
    expect(
      test.call({ evidence: "merged elsewhere", op: "done", task: id }).tasks.at(first),
    ).toMatchObject({
      status: "completed",
      unit: { acceptanceEvidence: "merged elsewhere" },
    });
  });
});

describe("closed revisions", () => {
  it("does not reopen completed work", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      id = firstId(
        test.call({
          items: [{ content: "Build", contract: contract(), key: "build" }],
          op: "init",
          start: "build",
        }),
      );
    test.call({ evidence: "passed", op: "done", task: id });
    expect(() => test.call({ op: "start", task: id })).toThrow("UNIT_CLOSED");
  });

  it(
    "keeps failed mutations atomic and rejects label selectors after creation",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const test = fixture(),
        id = firstId(test.call({ items: ["Build"], op: "init" })),
        before = [...test.entries];
      expect(() => test.call({ op: "start", task: id })).toThrow("CONTRACT_INCOMPLETE");
      expect(() => test.call({ op: "drop", task: "Build" })).toThrow("not found");
      expect(() => test.call({ items: ["a", "a"], op: "append" })).toThrow("Duplicate");
      expect(test.entries).toStrictEqual(before);
    },
  );
});

describe("contract revisions", () => {
  it("updates drafts with a new revision and freezes local work", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      id = firstId(test.call({ items: ["Build"], op: "init" }));
    test.call({ contract: contract(), op: "update", task: id });
    const revision = test.call({ op: "view" }).tasks.at(first)?.unit?.reference;
    expect(revision).toMatchObject({
      contractEntryId: test.entries.at(second)?.id,
      revision: 2,
      unitId: id,
    });
    test.call({ op: "start", task: id });
    expect(() => test.call({ op: "done", task: id })).toThrow("ACCEPTANCE_REQUIRED");
    expect(
      test.call({ evidence: "All assertions passed", op: "done", task: id }).tasks.at(first)?.unit,
    ).toMatchObject({ acceptanceEvidence: "All assertions passed", reference: revision });
  });

  it("blocks contract mutation and replacement while active", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      id = firstId(
        test.call({
          items: [{ content: "Build", contract: contract(), key: "build" }],
          op: "init",
          start: "build",
        }),
      );
    expect(() => test.call({ contract: contract(), op: "update", task: id })).toThrow(
      "UNIT_FROZEN",
    );
    expect(() => test.call({ items: ["Replacement"], op: "init" })).toThrow("LOCAL_UNIT_ACTIVE");
  });
});

describe("branch-local migration", () => {
  it("preserves legacy statuses and requires update then explicit start", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture([
      {
        tasks: [
          { content: "Old active", status: "in_progress" },
          { content: "Done", status: "completed" },
        ],
      },
    ]);
    expect(test.call({ op: "view" }).tasks.at(first)?.unit).toBeUndefined();
    const list = test.call({ contract: contract(), op: "update", task: "Old active" }),
      id = firstId(list);
    expect(list.tasks.map((task) => task.status)).toStrictEqual(["in_progress", "completed"]);
    expect(list.tasks.at(first)?.unit?.localSessionId).toBeUndefined();
    test.call({ op: "start", task: id });
    test.call({ op: "drop", task: id });
    expect(test.call({ op: "view" }).tasks.at(second)?.status).toBe("completed");
  });
});

describe("branch provenance", () => {
  it("reads branch provenance without circular append or global caches", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      id = firstId(test.call({ items: ["Build"], op: "init" })),
      originalEntry = test.entries.at(first)?.id;
    test.call({ contract: contract(), op: "update", task: id });
    const revisedEntry = test.entries.at(second)?.id;
    expect(branchSnapshot(test.entries).tasks.at(first)?.unit?.reference.contractEntryId).toBe(
      revisedEntry,
    );
    test.entries.pop();
    expect(branchSnapshot(test.entries).tasks.at(first)?.unit?.reference.contractEntryId).toBe(
      originalEntry,
    );
    test.call({ contract: contract(), op: "update", task: id });
    expect(branchSnapshot(test.entries).tasks.at(first)?.unit?.reference.contractEntryId).not.toBe(
      revisedEntry,
    );
  });

  it("shows empty branches and rejects damaged snapshots", { timeout: 5000 }, () => {
    expect.hasAssertions();
    expect(runTodo({ op: "view" }, fixture().session).content).toStrictEqual([
      { text: "Todo list is empty.", type: "text" },
    ]);
    expect(() =>
      fixture([{ tasks: [{ content: "a", status: "lost" }] }]).call({ op: "view" }),
    ).toThrow("unknown variant");
  });
});
