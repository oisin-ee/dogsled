import { describe, expect, it } from "vitest";
import piTodo from "./index.ts";
import { registration } from "./registration-fixture.ts";

describe("extension registration", () => {
  it("registers all operations and the branch lifecycle", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const registered = registration();
    piTodo(registered.api);
    expect(registered.tools).toMatchObject([
      {
        name: "todo",
        parameters: {
          properties: {
            op: { enum: ["init", "start", "done", "drop", "append", "view", "update"] },
          },
        },
      },
    ]);
    expect(registered.events).toStrictEqual([
      "tool_call",
      "tool_execution_end",
      "session_start",
      "session_shutdown",
      "session_tree",
      "agent_before_settle",
    ]);
    expect(registered.commands).toStrictEqual(["todos"]);
    expect(registered.shortcuts).toStrictEqual(["alt+t"]);
  });
});
