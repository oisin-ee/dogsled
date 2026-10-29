import type {
  AgentBeforeSettleEvent,
  BoundaryResult,
  SessionEntry,
} from "@earendil-works/pi-coding-agent";
import type { Failure, State } from "./policy.ts";
import { RECOVERY, branchState } from "./branch.ts";
import type { Context } from "./ports.ts";
import type { Router } from "./router.ts";
import type { Tiers } from "./config.ts";
import { classify } from "./policy.ts";
import { listed } from "./router.ts";

const ZERO = 0,
  ONE = 1;

/** Final-boundary recovery runs only after Pi's automatic retry policy ends. */
class Recovery {
  readonly #router: Readonly<Pick<Router, "failed" | "next">> & {
    readonly responses: Readonly<Router["responses"]>;
  };
  readonly #tiers: Readonly<Tiers>;

  /**
   * Connects boundary recovery to routing policy.
   * @param {Readonly<Router>} router Routing owner.
   * @param {Readonly<Tiers>} tiers Ordered targets.
   */
  public constructor(
    router: Readonly<Pick<Router, "failed" | "next">> & {
      readonly responses: Readonly<Router["responses"]>;
    },
    tiers: Readonly<Tiers>,
  ) {
    this.#router = router;
    this.#tiers = tiers;
  }

  /**
   * Applies a classified failure to the recovery budget and target choice.
   * @param {Failure} failure Failure class.
   * @param {State} state Branch state.
   * @param {readonly string[]} targets Tier targets.
   * @returns {Promise<State | undefined>} Recovery state, when permitted.
   */
  async #next(
    failure: Failure,
    state: State,
    targets: readonly string[],
  ): Promise<State | undefined> {
    await this.#router.failed(failure, state.chosen);
    if (failure === "request" || state.budget === ZERO) {
      return undefined;
    }
    const attempted = failure === "gateway" ? state.attempted : [...state.attempted, state.chosen],
      next = Object.assign({}, state, { attempted, budget: state.budget - ONE }),
      chosen = failure === "gateway" ? state.chosen : await this.#router.next(targets, next);
    return chosen === undefined ? undefined : Object.assign({}, next, { chosen });
  }

  /**
   * Omits only the failed assistant and appends a state-only recovery marker.
   * @param {AgentBeforeSettleEvent} event Final actionable boundary.
   * @param {Context} ctx Pi authority.
   * @returns {Promise<BoundaryResult | undefined>} Bounded recovery, or no action.
   */
  public async settle(
    event: AgentBeforeSettleEvent,
    ctx: Context,
  ): Promise<BoundaryResult | undefined> {
    if (
      event.outcome !== "error" ||
      ctx.signal?.aborted === true ||
      ctx.model?.provider !== "tier"
    ) {
      return undefined;
    }
    const tier = ctx.model.id,
      state = branchState(ctx.sessionManager.getBranch(), tier),
      tierTargets = this.#tiers.get(tier),
      targets = tierTargets === undefined ? undefined : listed(tierTargets, ctx),
      message = event.context.contextMessages.findLast((item) => item.role === "assistant"),
      entry = event.context.contextEntries.findLast(
        (item: Readonly<{ sourceEntry: SessionEntry; messages: readonly unknown[] }>) =>
          item.messages.includes(message),
      );
    if (
      state === undefined ||
      targets === undefined ||
      entry === undefined ||
      message?.role !== "assistant" ||
      message.stopReason !== "error" ||
      `${message.provider}/${message.model}` !== state.chosen
    ) {
      return undefined;
    }
    const text = message.errorMessage ?? "",
      next = await this.#next(classify(text, this.#router.responses.status(text)), state, targets);
    if (next === undefined || Boolean(ctx.signal?.aborted)) {
      return undefined;
    }
    return {
      continue: true,
      entries: [
        { replacement: null, targetId: entry.sourceEntry.id, type: "context_edit" },
        {
          customType: RECOVERY,
          data: { modelId: tier, provider: "tier", state: next },
          type: "custom",
        },
      ],
    };
  }
}

export { Recovery };
