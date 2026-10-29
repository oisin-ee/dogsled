import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { TodoContext } from "./lifecycle.ts";
import type { WorkHost } from "./work.ts";
import { ownsTodo } from "./lifecycle.ts";
import { workUnitGuidance } from "./guidance.ts";

/** The approved Dogsled plan and reviewer feedback. */
interface ApprovedPlan {
  readonly cwd: string;
  readonly planFilePath: string;
  readonly planContent: string;
  readonly feedback?: string;
}

type PlanHost = WorkHost & Readonly<Pick<ExtensionAPI, "sendMessage">>;
type PlanContext = TodoContext & { readonly cwd: string };

/**
 * Continues the approved plan through the existing todo tool, not a second parser or store.
 * @param {ApprovedPlan} plan The approved document and reviewer feedback.
 * @returns {string} Instructions followed by the exact handoff data.
 */
const continuation = (plan: ApprovedPlan): string =>
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

/** Continues approved plans without owning todo state or execution progress. */
class PlanApprovedHandoff {
  readonly #host: PlanHost;

  /**
   * Keeps the todo ownership and conversation API.
   * @param {PlanHost} host The Pi methods used by the handoff.
   */
  public constructor(host: PlanHost) {
    this.#host = host;
  }

  /**
   * Continues a plan with the current session context.
   * @param {ApprovedPlan} plan The approved document and feedback.
   * @param {PlanContext} ctx The current branch and working directory.
   */
  public approve(plan: ApprovedPlan, ctx: PlanContext): void {
    if (plan.cwd !== ctx.cwd) {
      ctx.ui.notify(
        "pi-todo: The approved plan belongs to another working directory. No execution started.",
        "error",
      );
    } else if (ownsTodo(this.#host)) {
      this.#host.sendMessage(
        { content: continuation(plan), customType: "todo-plan-approved", display: true },
        { deliverAs: "followUp", triggerTurn: true },
      );
    } else {
      ctx.ui.notify("pi-todo: Another extension owns todo. No execution started.", "error");
    }
  }
}

export { PlanApprovedHandoff };
export type { ApprovedPlan, PlanContext, PlanHost };
