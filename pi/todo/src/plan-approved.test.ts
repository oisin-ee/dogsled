import type { PlanContext, PlanHost } from "./plan-approved.ts";
import { describe, expect, it, vi } from "vitest";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { PlanApprovedHandoff } from "./plan-approved.ts";

type Listener = (value: unknown) => void;
const channel = "dogsled:plan-approved",
  oneCall = 1,
  firstMessage = 0,
  approved = {
    cwd: "/project",
    feedback: "Keep the public API.",
    planContent: "# Plan\n\nInspect the caller, then implement and verify the change.",
    planFilePath: "plans/change.md",
  },
  /**
   * Creates a synchronous event bus, including real unsubscribe behavior.
   * @returns {ExtensionAPI["events"]} The supported extension event seam.
   */
  eventBus = (): ExtensionAPI["events"] => {
    const listeners = new Map<string, Set<Listener>>();
    return {
      emit: (name, value): void => {
        for (const listener of listeners.get(name) ?? []) {
          listener(value);
        }
      },
      on: (name, listener): (() => void) => {
        const group = listeners.get(name) ?? new Set<Listener>();
        group.add(listener);
        listeners.set(name, group);
        return (): void => {
          group.delete(listener);
        };
      },
    };
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
        events: eventBus(),
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
      test.handoff.start(test.ctx);
      test.host.events.emit(channel, approved);
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
      test.handoff.stop();
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
    test.handoff.start(Object.assign({}, test.ctx, { mode: "rpc" as const }));
    test.host.events.emit(channel, plan);
    expect(test.host.sendMessage).toHaveBeenCalledTimes(oneCall);
    expect(test.ctx.ui.setWidget).not.toHaveBeenCalled();
    test.handoff.stop();
  });
});

describe("approved plan continuation text", () => {
  it("uses Dogsled plan mode without checklist markers", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.start(test.ctx);
    test.host.events.emit(channel, approved);
    expect(test.messages.at(firstMessage)).toContain(
      "The user approved this plan in Dogsled plan mode. Continue execution in this session.",
    );
    expect(test.messages.at(firstMessage)).not.toContain("[DONE:");
    test.handoff.stop();
  });
});

describe("work unit guidance", () => {
  it("includes bounded delegation and parent review", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.start(test.ctx);
    test.host.events.emit(channel, approved);
    expect(test.messages.at(firstMessage)).toContain("one agent session");
    expect(test.messages.at(firstMessage)).toContain("parent integration review");
    test.handoff.stop();
  });
});

describe("completed plan reapproval", () => {
  it("does not instruct the agent to reopen completed tasks", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.start(test.ctx);
    test.host.events.emit(channel, approved);
    expect(test.messages.at(firstMessage)).toContain(
      "If the list is empty, call todo with op=init",
    );
    expect(test.messages.at(firstMessage)).toContain(
      "Never restart completed work because the plan was approved again.",
    );
    expect(test.messages.at(firstMessage)).toContain(
      "If no approved work remains, report completion without changing tasks.",
    );
    test.handoff.stop();
  });
});

describe("handoff lifecycle", () => {
  it("replaces subscriptions and removes them at shutdown", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.start(test.ctx);
    test.handoff.start(test.ctx);
    test.host.events.emit(channel, approved);
    expect(test.host.sendMessage).toHaveBeenCalledTimes(oneCall);
    test.handoff.stop();
    test.handoff.stop();
    test.host.events.emit(channel, approved);
    expect(test.host.sendMessage).toHaveBeenCalledTimes(oneCall);
  });

  it("uses the replacement session context", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.start(test.ctx);
    test.handoff.start(Object.assign({}, test.ctx, { cwd: "/other" }));
    test.host.events.emit(channel, approved);
    expect(test.host.sendMessage).not.toHaveBeenCalled();
    expect(test.ctx.ui.notify).toHaveBeenCalledWith(
      expect.stringContaining("another working directory"),
      "error",
    );
    test.host.events.emit(channel, Object.assign({}, approved, { cwd: "/other" }));
    expect(test.host.sendMessage).toHaveBeenCalledTimes(oneCall);
    test.handoff.stop();
  });
});

describe("handoff rejection", () => {
  it.each([
    null,
    {},
    Object.assign({}, approved, { planContent: " " }),
    Object.assign({}, approved, { planFilePath: "" }),
    Object.assign({}, approved, { feedback: false }),
  ])("rejects an invalid payload: %j", { timeout: 5000 }, (value) => {
    expect.hasAssertions();
    const test = fixture();
    test.handoff.start(test.ctx);
    test.host.events.emit(channel, value);
    expect(test.host.sendMessage).not.toHaveBeenCalled();
    expect(test.ctx.ui.notify).toHaveBeenCalledWith(
      expect.stringContaining("Invalid Dogsled plan approval"),
      "error",
    );
    test.handoff.stop();
  });

  it("does not send another todo owner instructions", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const test = fixture(true);
    test.handoff.start(test.ctx);
    test.host.events.emit(channel, approved);
    expect(test.host.sendMessage).not.toHaveBeenCalled();
    expect(test.ctx.ui.notify).toHaveBeenCalledWith(
      expect.stringContaining("Another extension owns todo"),
      "error",
    );
    test.handoff.stop();
  });
});
