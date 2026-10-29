import type { Request, TodoList } from "./wasm.ts";
import { apply, parse, view, widget } from "./wasm.ts";
import { childId, contract, first, firstId, fixture, second } from "./contract-fixture.ts";
import { describe, expect, it } from "vitest";
import type { Fixture } from "./contract-fixture.ts";
import { TodoView } from "./embedded.ts";
import { display } from "./display.ts";
import { runTodo } from "./tool.ts";

const width = 160,
  height = 30,
  outcome = { content: "Ship", subtasks: ["Implement and test", "Integrate and test"] },
  context = { mutationId: "boundary", sessionId: "test-session" },
  create = (): TodoList => fixture().call({ items: [outcome, "Docs"], op: "init" }),
  host = {
    height: (): number => height,
    requestRender: (): void => {
      /* No terminal in this boundary test. */
    },
    theme: {
      bold: (text: string): string => text,
      fg: (_color: string, text: string): string => text,
    },
  },
  /**
   * Creates a hierarchy through the real tool for display and review tests.
   * @returns {object} Branch caller, active list and globally unique child IDs.
   */
  hierarchy = (): {
    build: string;
    initial: TodoList;
    parent: string;
    review: string;
    test: Fixture;
  } => {
    const test = fixture(),
      initial = test.call({
        items: [
          {
            content: "Ship",
            subtasks: [
              { content: "Implement and test", contract: contract(), key: "build" },
              "Integrate and test",
            ],
          },
          "Docs",
        ],
        op: "init",
        start: "build",
      });
    return {
      build: childId(initial, first),
      initial,
      parent: firstId(initial),
      review: childId(initial, second),
      test,
    };
  };

describe("hierarchical WASM callers", () => {
  it("preserves hierarchy and rejects executable parents", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const { initial, parent, test } = hierarchy(),
      component = new TodoView(initial, host);
    expect(display(initial).current).toContain("Ship → Implement and test");
    expect(component.render(width).join("\n")).toContain("● Implement and test");
    expect(() => test.call({ op: "start", task: parent })).toThrow("NOT_EXECUTABLE");
    expect(() => test.call({ evidence: "review", op: "done", task: parent })).toThrow(
      "open subtasks",
    );
    component.dispose();
  });

  it("keeps explicit integration review after child closure", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const { build, parent, review, test } = hierarchy();
    test.call({ evidence: "tests passed", op: "done", task: build });
    const ready = test.call({ op: "drop", task: review });
    expect(view(ready)).toContain("Ready for review");
    expect(view(ready)).toContain("1/2 subtasks completed");
    expect(widget(ready, true)).toContainEqual(expect.objectContaining({ kind: "done" }));
    expect(() => test.call({ op: "done", task: parent })).toThrow("ACCEPTANCE_REQUIRED");
    expect(
      test
        .call({ evidence: "Reviewed combined deliverable", op: "done", task: parent })
        .tasks.at(first)?.status,
    ).toBe("completed");
  });
});

describe("strict operation input", () => {
  it.each([
    { op: "view", unexpected: true },
    { items: [{ content: "Root", subtasks: [], unexpected: true }], op: "init" },
  ] as const)("rejects unknown fields without persisting %j", { timeout: 5000 }, (params) => {
    expect.hasAssertions();
    const stored: TodoList[] = [];
    expect(() =>
      runTodo(params, {
        append: (_kind, list) => {
          stored.push(list);
        },
        branch: () => [],
        sessionId: "test-session",
      }),
    ).toThrow(/unknown field|untagged enum Item/u);
    expect(stored).toStrictEqual([]);
  });
});

describe("strict WASM operation input", () => {
  it("rejects unknown request and outcome keys directly in WASM", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const request: Request = {
        items: undefined,
        op: "view",
        parent: undefined,
        task: undefined,
      },
      invalidRequest = Object.assign({}, request, { unexpected: true }),
      invalidOutcome = Object.assign({}, outcome, { unexpected: true });
    expect(() => apply(create(), invalidRequest, context)).toThrow("unknown field `unexpected`");
    expect(() =>
      apply(
        create(),
        Object.assign({}, request, {
          items: [invalidOutcome],
          op: "init" as const,
        }),
        context,
      ),
    ).toThrow("untagged enum Item");
  });

  it(
    "rejects duplicate request keys even when the first value is absent",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const request = {
        items: undefined,
        op: "view" as const,
        parent: undefined,
        task: undefined,
        [Symbol.iterator]: (): IterableIterator<unknown> =>
          [
            ["op", "view"],
            ["task", undefined],
            ["task", undefined],
          ].values(),
      };
      expect(() => apply(create(), request, context)).toThrow("duplicate field `task`");
    },
  );
});

// Iterable pairs preserve duplicate keys that ordinary JavaScript objects cannot represent.
describe("duplicate snapshot keys", () => {
  it.each([
    {
      tasks: [],
      [Symbol.iterator]: (): IterableIterator<unknown> =>
        [
          ["tasks", []],
          ["tasks", []],
        ].values(),
    },
    {
      tasks: [
        [
          ["content", "Root"],
          ["content", "Root"],
          ["status", "pending"],
        ],
      ],
    },
    {
      tasks: [
        {
          content: "Root",
          status: "pending",
          subtasks: [
            [
              ["content", "Child"],
              ["status", "pending"],
              ["status", "pending"],
            ],
          ],
        },
      ],
    },
  ])("rejects repeated map entries %j", { timeout: 5000 }, (snapshot: unknown) => {
    expect.hasAssertions();
    expect(() => parse(snapshot)).toThrow("duplicate field");
  });
});

describe("snapshot validation", () => {
  it("enumerates unknown list keys through the real WASM map visitor", { timeout: 5000 }, () => {
    expect.hasAssertions();
    expect(() => parse({ tasks: [], unexpected: true })).toThrow("unknown field `unexpected`");
  });

  it("reads old snapshots without normalization", { timeout: 5000 }, () => {
    expect.hasAssertions();
    expect(parse({ tasks: [{ content: "Old", status: "in_progress" }] })).toStrictEqual({
      tasks: [{ content: "Old", status: "in_progress", subtasks: [] }],
    });
  });
});

describe("closed parent invariants", () => {
  it.each(["completed", "abandoned"])(
    "rejects a %s parent with open children",
    { timeout: 5000 },
    (status) => {
      expect.hasAssertions();
      const snapshot = {
        tasks: [
          { content: "Closed", status, subtasks: [{ content: "Open", status: "in_progress" }] },
        ],
      };
      expect(() => parse(snapshot)).toThrow("open subtasks");
    },
  );
});

describe("malformed snapshots", () => {
  it.each([
    { tasks: [], unexpected: true },
    { tasks: [{ content: "Root", status: "pending", unexpected: true }] },
    {
      tasks: [
        {
          content: "Root",
          status: "pending",
          subtasks: [{ content: "Child", status: "pending", unexpected: true }],
        },
      ],
    },
    { tasks: [{ content: " ", status: "pending" }] },
    {
      tasks: [
        { content: "Same", status: "pending" },
        { content: "Same", status: "completed" },
      ],
    },
    {
      tasks: [
        {
          content: "Root",
          status: "pending",
          subtasks: [{ content: "Child", status: "pending", subtasks: [] }],
        },
      ],
    },
  ])("rejects malformed state %j", { timeout: 5000 }, (snapshot: unknown) => {
    expect.hasAssertions();
    expect(() => parse(snapshot)).toThrow(/Missing|Duplicate|Invalid|open subtasks|unknown field/u);
  });
});
