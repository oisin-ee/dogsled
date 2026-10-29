import { describe, expect, it } from "vitest";
import { harness, nudge, result } from "./fixture.ts";

const ONE = 1;

describe("turn ordering", () => {
  it("does not use a later persisted todo to gate an earlier edit", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness(),
      edit = result("edit", false),
      todo = result("todo", false),
      branch = [
        {
          id: "edit-result",
          message: edit,
          parentId: null,
          timestamp: "",
          type: "message" as const,
        },
        {
          id: "todo-result",
          message: todo,
          parentId: null,
          timestamp: "",
          type: "message" as const,
        },
      ],
      ctx = Object.assign({}, test.ctx, {
        sessionManager: { getBranch: () => [...test.branch, ...branch] },
      });
    await nudge(test);
    await expect(
      test.adapter.turn(
        {
          entries: [],
          toolResultEntryIds: ["edit-result", "todo-result"],
          toolResults: [edit, todo],
        },
        ctx,
      ),
    ).resolves.toStrictEqual({ entries: [] });
    expect(test.selections).toStrictEqual([]);
  });

  it("preserves another extension's drafts and continuation", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const test = harness(),
      prior = [{ customType: "todo-reminder", data: {}, type: "custom" as const }];
    await expect(test.adapter.run({ kind: "settle" }, test.ctx, prior)).resolves.toStrictEqual({
      entries: prior,
    });
    expect(prior).toHaveLength(ONE);
  });
});
