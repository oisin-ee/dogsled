import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { TodoContext } from "./lifecycle.ts";
import type { WorkHost } from "./work.ts";
import { ownsTodo } from "./lifecycle.ts";
import { workUnitGuidance } from "./guidance.ts";

/** The supported Dogsled plan approval event. */
interface ApprovedPlan {
  readonly cwd: string;
  readonly planFilePath: string;
  readonly planContent: string;
  readonly feedback?: string;
}

type PlanHost = WorkHost & Readonly<Pick<ExtensionAPI, "sendMessage">>;
type PlanContext = TodoContext & { readonly cwd: string };

/**
 * Validates the event before it enters the model context.
 * @param {unknown} value The shared event payload.
 * @returns {boolean} Whether the required handoff fields are present.
 */
const isApprovedPlan = (value: unknown): value is ApprovedPlan =>
    typeof value === "object" &&
    value !== null &&
    "cwd" in value &&
    typeof value.cwd === "string" &&
    "planFilePath" in value &&
    typeof value.planFilePath === "string" &&
    value.planFilePath.trim() !== "" &&
    "planContent" in value &&
    typeof value.planContent === "string" &&
    value.planContent.trim() !== "" &&
    (!("feedback" in value) || value.feedback === undefined || typeof value.feedback === "string"),
  /**
   * Continues the approved plan through the existing todo tool, not a second parser or store.
   * @param {ApprovedPlan} plan The approved document and reviewer feedback.
   * @returns {string} Instructions followed by the exact handoff data.
   */
  continuation = (plan: ApprovedPlan): string =>
    [
      "The user approved this plan in Dogsled plan mode. Continue execution in this session.",
      "Use the todo tool as the only task tracker.",
      "First call todo with op=view to inspect the current branch list.",
      "Before implementation, represent remaining outcomes with optional bounded subtasks in that list. Use parent plus exact task for child operations.",
      workUnitGuidance,
      "If the list is empty, call todo with op=init and the remaining approved tasks in order.",
      "Otherwise, reuse matching tasks and append missing tasks. Preserve unrelated work and completed progress.",
      "If approved work remains, explicitly start its open units with op=start. Independent units can stay active together. Never restart completed work because the plan was approved again.",
      "If no approved work remains, report completion without changing tasks. Otherwise, update tasks with op=done or op=drop as work proceeds.",
      "Resolve a relative planFilePath against cwd. The approved document and reviewer feedback follow as JSON:",
      JSON.stringify({
        cwd: plan.cwd,
        feedback: plan.feedback,
        planContent: plan.planContent,
        planFilePath: plan.planFilePath,
      }),
    ].join("\n");

/** Owns the approval subscription, never the todo list or execution progress. */
class PlanApprovedHandoff {
  readonly #host: PlanHost;
  #unsubscribe: (() => void) | undefined;

  /**
   * Keeps the supported event bus and conversation API.
   * @param {PlanHost} host The Pi methods used by the handoff.
   */
  public constructor(host: PlanHost) {
    this.#host = host;
  }

  /**
   * Subscribes with the current session context after startup or replacement.
   * @param {PlanContext} ctx The current branch and working directory.
   */
  public start(ctx: PlanContext): void {
    this.stop();
    this.#unsubscribe = this.#host.events.on("dogsled:plan-approved", (value: unknown): void => {
      if (!isApprovedPlan(value)) {
        ctx.ui.notify("pi-todo: Invalid Dogsled plan approval. No execution started.", "error");
      } else if (value.cwd !== ctx.cwd) {
        ctx.ui.notify(
          "pi-todo: The approved plan belongs to another working directory. No execution started.",
          "error",
        );
      } else if (ownsTodo(this.#host)) {
        this.#host.sendMessage(
          { content: continuation(value), customType: "todo-plan-approved", display: true },
          { deliverAs: "followUp", triggerTurn: true },
        );
      } else {
        ctx.ui.notify("pi-todo: Another extension owns todo. No execution started.", "error");
      }
    });
  }

  /** Removes the previous session's listener before replacement or shutdown. */
  public stop(): void {
    this.#unsubscribe?.();
    this.#unsubscribe = undefined;
  }
}

export { PlanApprovedHandoff };
export type { PlanContext, PlanHost };
