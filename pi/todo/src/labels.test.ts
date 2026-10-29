import { describe, expect, it } from "vitest";
import { first, firstId, fixture, second } from "./contract-fixture.ts";
import { sections, view } from "./wasm.ts";

describe("headline and description", () => {
  it("stores, sets, and clears descriptions", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      list = test.call({
        items: [
          { content: "Ship", description: " Why and how ", subtasks: [{ content: "Build" }] },
          "Plain",
        ],
        op: "init",
      });
    expect(list.tasks.at(first)?.description).toBe("Why and how");
    expect(list.tasks.at(second)?.description).toBeUndefined();
    const id = firstId(list),
      set = test.call({ description: "Added", op: "update", task: id });
    expect(set.tasks.at(first)?.description).toBe("Added");
    expect(set.tasks.at(first)?.content).toBe("Ship");
    expect(
      test.call({ description: "  ", op: "update", task: id }).tasks.at(first),
    ).not.toHaveProperty("description");
  });

  it("carries descriptions in sections and the model view", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const list = fixture().call({
      items: [{ content: "Ship", description: "Detail line", key: "ship" }],
      op: "init",
    });
    expect(sections(list).at(second)?.items.at(first)?.description).toBe("Detail line");
    expect(view(list)).toContain("[ ] Ship\n  Detail line");
  });
});
