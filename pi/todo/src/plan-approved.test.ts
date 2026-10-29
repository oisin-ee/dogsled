import type { PlanContext, PlanHost } from "./plan-approved.ts";
import { describe, expect, it, vi } from "vitest";
import { PlanApprovedHandoff } from "./plan-approved.ts";

const oneCall = 1,
  firstMessage = 0,
  approved = {
    cwd: "/project",
    feedback: "Keep the public API.",
    planContent: "# Plan\n\nInspect the caller, then implement and verify the change.",
    planFilePath: "plans/change.md",
  },
  /**
   * Creates the real handoff with a typed host and a fresh session context.
   * @param {boolean} collision Whether another package owns the todo tool.
   * @returns {object} The host, context, and handoff under test.
   */
  fixture = (
    collision = false,
  ): {
    readonly ctx: PlanContext;
    readonly host: PlanHost;
    readonly handoff: Readonly<PlanApprovedHandoff>;
    readonly messages: readonly unknown[];
  } => {
    const messages: unknown[] = [],
      host: PlanHost = {
        getAllTools: () =>
          collision
            ? []
            : [
                {
                  name: "todo",
                  sourceInfo: {
                    path: decodeURIComponent(new URL("index.ts", import.meta.url).pathname),
                  },
                },
              ],
        sendMessage: vi.fn<PlanHost["sendMessage"]>(
          (message: { readonly content: unknown }): void => {
            messages.push(message.content);
          },
        ),
      },
      ctx: PlanContext = {
        cwd: "/project",
        mode: "tui",
        sessionManager: { getBranch: () => [] },
        ui: { notify: vi.fn<() => void>(), setWidget: vi.fn<() => void>() },
      };
    return { ctx, handoff: new PlanApprovedHandoff(host), host, messages };
  };

describe("approved plan continuation", () => {
  it(
    "queues the exact approved document and feedback through the todo tool",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const test = fixture();
      test.handoff.approve(approved, test.ctx);
      expect(test.host.sendMessage).toHaveBeenCalledExactlyOnceWith(
        {
          content: test.messages.at(firstMessage),
          customType: "todo-plan-approved",
          display: true,
        },
        { deliverAs: "followUp", triggerTurn: true },
      );
      expect(test.messages.at(firstMessage)).toContain(JSON.stringify(approved));
      expect(test.messages.at(firstMessage)).toContain("First call todo with op=view");
      expect(test.messages.at(firstMessage)).toContain(
        "Preserve unrelated work and completed progress.",
      );
      expect(test.ctx.ui.notify).not.toHaveBeenCalled();
    },
  );

  it("works outside the terminal and without reviewer feedback", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(),
      plan = {
        cwd: approved.cwd,
        planContent: approved.planContent,
        planFilePath: approved.planFilePath,
      };
    test.handoff.approve(plan, Object.assign({}, test.ctx, { mode: "rpc" as const }));
    expect(test.host.sendMessage).toHaveBeenCalledTimes(oneCall);
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
  });
});

describe("approved plan continuation text", () => {
  it("uses Dogsled plan mode without checklist markers", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.approve(approved, test.ctx);
    expect(test.messages.at(firstMessage)).toContain(
      "The user approved this plan in Dogsled plan mode. Continue execution in this session.",
    );
    expect(test.messages.at(firstMessage)).not.toContain("[DONE:");
  });
});

describe("work unit guidance", () => {
  it("includes bounded delegation and parent review", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.approve(approved, test.ctx);
    expect(test.messages.at(firstMessage)).toContain("one agent session");
    expect(test.messages.at(firstMessage)).toContain("parent integration review");
  });
});

describe("completed plan reapproval", () => {
  it("does not instruct the agent to reopen completed tasks", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.approve(approved, test.ctx);
    expect(test.messages.at(firstMessage)).toContain(
      "If the list is empty, call todo with op=init",
    );
    expect(test.messages.at(firstMessage)).toContain(
      "Never restart completed work because the plan was approved again.",
    );
    expect(test.messages.at(firstMessage)).toContain(
      "If no approved work remains, report completion without changing tasks.",
    );
  });
});

describe("handoff lifecycle", () => {
  it("does not retain a subscription after a direct handoff", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.approve(approved, test.ctx);
    expect(test.host.sendMessage).toHaveBeenCalledTimes(oneCall);
  });

  it("uses the replacement session context", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.approve(approved, Object.assign({}, test.ctx, { cwd: "/other" }));
    expect(test.host.sendMessage).not.toHaveBeenCalled();
    expect(test.ctx.ui.notify).toHaveBeenCalledWith(
      expect.stringContaining("another working directory"),
      "error",
    );
    test.handoff.approve(
      Object.assign({}, approved, { cwd: "/other" }),
      Object.assign({}, test.ctx, { cwd: "/other" }),
    );
    expect(test.host.sendMessage).toHaveBeenCalledTimes(oneCall);
  });
});

describe("handoff rejection", () => {
  it("does not send another todo owner instructions", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(true);
    test.handoff.approve(approved, test.ctx);
    expect(test.host.sendMessage).not.toHaveBeenCalled();
    expect(test.ctx.ui.notify).toHaveBeenCalledWith(
      expect.stringContaining("Another extension owns todo"),
      "error",
    );
  });
});
