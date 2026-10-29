import { BUDGET, classify, target } from "./policy.ts";
import type { Context, Effects, Request } from "./ports.ts";
import type { Failure, State } from "./policy.ts";
import { branchState, newUserTurn } from "./branch.ts";
import { exhaustedUntil, parseQuota } from "./quota.ts";
import type { Health } from "./health.ts";
import type { ModelRoute } from "@earendil-works/pi-coding-agent";
import { Responses } from "./responses.ts";
import type { Window } from "./quota.ts";

const ZERO = 0,
  ONE = 1,
  CACHE_MS = 60_000,
  /**
   * Resolves tier targets to the references the model registry lists. Provider model
   * ids differ in case from the tier file (`MiniMax-M3.1-Flash`), so matching ignores
   * case and returns the registry's exact reference. A target that is not listed yet
   * behaves like an unavailable target instead of a terminal error.
   * @param {readonly string[]} targets Ordered tier.
   * @param {Context} ctx Pi authority.
   * @returns {readonly string[]} Listed registry references, in tier order.
   */
  listed = (targets: readonly string[], ctx: Context): readonly string[] => {
    const available = new Map(
      ctx.modelRegistry.getAvailable().map((candidate) => {
        const reference = `${candidate.provider}/${candidate.id}`;
        return [reference.toLowerCase(), reference];
      }),
    );
    return targets.flatMap((reference) => {
      const parsed = target(reference),
        found = available.get(`${parsed.provider}/${parsed.id}`.toLowerCase());
      return found === undefined ? [] : [found];
    });
  };

/** Sticky routing with host-wide quota and gateway health. */
class Router {
  readonly #effects: Effects;
  public readonly responses = new Responses();
  #quota: readonly Window[] = [];
  #quotaUntil = ZERO;

  /**
   * Connects policy to host effects.
   * @param {Effects} effects External authority.
   */
  public constructor(effects: Effects) {
    this.#effects = effects;
  }

  /**
   * Refreshes only the local cached quota snapshot, at most once per minute.
   * @returns {Promise<void>} Completion.
   */
  async #refresh(): Promise<void> {
    if (this.#effects.now() >= this.#quotaUntil) {
      this.#quota = parseQuota(await this.#effects.quota());
      this.#quotaUntil = this.#effects.now() + CACHE_MS;
    }
  }

  /**
   * Checks both explicit cooldowns and account windows.
   * @param {string} reference Model reference.
   * @param {Health} health Shared snapshot.
   * @returns {boolean} Availability.
   */
  #available(reference: string, health: Health): boolean {
    const { pool } = target(reference),
      now = this.#effects.now();
    return (
      (health.pools[pool]?.until ?? ZERO) <= now &&
      exhaustedUntil(this.#quota, pool, now) === undefined
    );
  }

  /**
   * Chooses the first unattempted available target without changing state.
   * @param {readonly string[]} targets Ordered tier.
   * @param {State} state Branch state.
   * @returns {Promise<string | undefined>} Next target.
   */
  public async next(targets: readonly string[], state: State): Promise<string | undefined> {
    await this.#refresh();
    const health = await this.#effects.read();
    return targets.find(
      (candidate) => !state.attempted.includes(candidate) && this.#available(candidate, health),
    );
  }

  /**
   * Records only gateway circuits or quota cooldowns.
   * @param {Failure} failure Classified failure.
   * @param {string} reference Failed physical model.
   * @returns {Promise<void>} Completion.
   */
  public async failed(failure: Failure, reference: string): Promise<void> {
    if (failure === "gateway" || failure === "quota") {
      await this.#refresh();
      const parsed = target(reference),
        now = this.#effects.now();
      await this.#effects.change({
        key: failure === "gateway" ? parsed.provider : parsed.pool,
        kind: failure,
        now,
        reset: failure === "quota" ? exhaustedUntil(this.#quota, parsed.pool, now) : undefined,
      });
    }
  }

  /**
   * Keeps native transient retries on their current physical model.
   * @param {Request} request Pi request.
   * @param {State} state Current branch state.
   * @returns {Promise<State>} State after a native retry failure.
   */
  async #retry(request: Request, state: State): Promise<State> {
    const { failed } = request;
    if (failed === undefined) {
      return state;
    }
    const message = failed.message.errorMessage ?? "",
      failure = classify(message, this.responses.status(message));
    if (failed.message.stopReason === "aborted" || failure === "request") {
      throw new Error(message || "Tier request stopped.");
    }
    await this.failed(failure, state.chosen);
    if (failure === "quota" && state.budget === ZERO) {
      throw new Error("Tier recovery budget exhausted.");
    }
    return failure === "quota"
      ? Object.assign({}, state, {
          attempted: [...state.attempted, state.chosen],
          budget: state.budget - ONE,
        })
      : state;
  }

  /**
   * Dispatches with the selected thinking level after checking shared health.
   * @param {Request} request Pi request.
   * @param {Context} ctx Pi authority.
   * @param {State} state Chosen state.
   * @returns {Promise<ModelRoute<State>>} Physical route.
   */
  async #dispatch(request: Request, ctx: Context, state: State): Promise<ModelRoute<State>> {
    const parsed = target(state.chosen),
      model = ctx.modelRegistry
        .getAvailable()
        .find((candidate) => candidate.provider === parsed.provider && candidate.id === parsed.id);
    if (model === undefined) {
      throw new Error(`Unknown model: ${state.chosen}`);
    }
    const health = await this.#effects.read(),
      until = health.circuits[parsed.provider]?.until ?? ZERO;
    await this.#effects.wait(until - this.#effects.now(), request.signal);
    request.signal?.throwIfAborted();
    return { model, state, thinkingLevel: request.thinkingLevel };
  }

  /**
   * Routes a tier without fabricating conversation messages.
   * @param {readonly string[]} tier Ordered physical models, listed or not.
   * @param {Request} request Pi request.
   * @param {Context} ctx Pi authority.
   * @returns {Promise<ModelRoute<State>>} Physical dispatch.
   */
  public async route(
    tier: readonly string[],
    request: Request,
    ctx: Context,
  ): Promise<ModelRoute<State>> {
    request.signal?.throwIfAborted();
    await this.#refresh();
    const targets = listed(tier, ctx),
      previous =
        request.reason === "direct"
          ? undefined
          : (branchState(ctx.sessionManager.getBranch(), request.model.id) ?? request.state),
      initial = previous ?? { attempted: [], budget: BUDGET, chosen: targets[ZERO] ?? "" },
      current =
        request.reason === "user" && newUserTurn(ctx.sessionManager.getBranch())
          ? Object.assign({}, initial, { attempted: [], budget: BUDGET })
          : initial,
      state = await this.#retry(request, current),
      health = await this.#effects.read(),
      chosen =
        targets.includes(state.chosen) &&
        !state.attempted.includes(state.chosen) &&
        this.#available(state.chosen, health)
          ? state.chosen
          : await this.next(targets, state);
    if (chosen === undefined) {
      throw new Error("No available target in tier.");
    }
    return this.#dispatch(request, ctx, Object.assign({}, state, { chosen }));
  }
}

export { Router, listed };
