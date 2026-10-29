import type { Api, AssistantMessage, Model } from "@earendil-works/pi-ai";
import type { Context, Effects } from "./ports.ts";
import type {
  ModelRouteRequest,
  SessionBoundaryDraft,
  SessionEntry,
} from "@earendil-works/pi-coding-agent";
import { emptyHealth, updateHealth } from "./health.ts";
import type { Health } from "./health.ts";
import { Recovery } from "./recovery.ts";
import { Router } from "./router.ts";
import type { State } from "./policy.ts";
import { VIRTUAL_MODEL_STATE_ENTRY } from "@earendil-works/pi-coding-agent";
import { retryAssistantCall } from "@earendil-works/pi-ai/utils/retry";
import { target } from "./policy.ts";

const ZERO = 0,
  ONE = 1,
  LAST = -1,
  START = 10_000,
  /**
   * Creates a physical catalog entry for the fake provider.
   * @param {string} reference Model reference.
   * @returns {Model<Api>} Model.
   */
  model = (reference: string): Model<Api> => {
    const parsed = target(reference);
    return {
      api: "openai-responses",
      baseUrl: "https://example.test",
      contextWindow: 128_000,
      cost: { cacheRead: 0, cacheWrite: 0, input: 0, output: 0 },
      id: parsed.id,
      input: ["text"],
      maxTokens: 16_384,
      name: parsed.id,
      provider: parsed.provider,
      reasoning: true,
    };
  },
  /**
   * Makes a fake provider failure with the actual dispatched model identity.
   * @param {string} reference Model reference.
   * @param {string} errorMessage Provider text.
   * @returns {AssistantMessage} Failed response.
   */
  response = (reference: string, errorMessage: string): AssistantMessage => {
    const physical = model(reference);
    return {
      api: physical.api,
      content: [],
      errorMessage,
      model: physical.id,
      provider: physical.provider,
      role: "assistant",
      stopReason: "error",
      timestamp: 0,
      usage: {
        cacheRead: 0,
        cacheWrite: 0,
        cost: { cacheRead: 0, cacheWrite: 0, input: 0, output: 0, total: 0 },
        input: 0,
        output: 0,
        totalTokens: 0,
      },
    };
  };

/** Fake provider host with real routing, boundary recovery, and Pi retry policy. */
class Harness {
  public readonly branch: SessionEntry[] = [];
  public readonly dispatched: string[] = [];
  public readonly waits: number[] = [];
  public readonly router: Router;
  public readonly recovery: Recovery;
  public readonly targets: readonly string[];
  public readonly abort = new AbortController();
  public health: Health = emptyHealth();
  public quota = "";
  public unlisted: readonly string[] = [];
  public registry: readonly string[] | undefined = undefined;
  public quotaReads = ZERO;
  public now = START;
  #state: State | undefined = undefined;
  #failed: ModelRouteRequest<State>["failed"] = undefined;

  /**
   * Connects fake transport effects without replacing policy.
   * @param {readonly string[]} targets Ordered tier.
   */
  public constructor(targets: readonly string[]) {
    this.targets = targets;
    const effects: Effects = {
      change: async (change) => {
        this.health = updateHealth(this.health, change);
      },
      now: () => this.now,
      quota: async () => {
        this.quotaReads += ONE;
        return this.quota;
      },
      read: async () => this.health,
      wait: async (delay, signal) => {
        signal?.throwIfAborted();
        this.waits.push(Math.max(ZERO, delay));
        this.now += Math.max(ZERO, delay);
      },
    };
    this.router = new Router(effects);
    this.recovery = new Recovery(this.router, new Map([["test", targets]]));
  }

  /**
   * Returns Pi authority over the fake branch.
   * @returns {Context} Context.
   */
  public context(): Context {
    return {
      model: model("tier/test"),
      modelRegistry: {
        getAvailable: () =>
          (this.registry ?? this.targets)
            .filter((reference) => !this.unlisted.includes(reference))
            .map((reference) => model(reference)),
      },
      sessionManager: { getBranch: () => this.branch },
      signal: this.abort.signal,
    };
  }

  /**
   * Persists an actionable boundary draft.
   * @param {SessionBoundaryDraft} draft Draft.
   */
  public append(
    draft:
      | Readonly<Extract<SessionBoundaryDraft, { type: "custom" }>>
      | Readonly<{ type: "context_edit"; targetId: string; replacement: null }>,
  ): void {
    this.branch.push(
      Object.assign({ id: crypto.randomUUID(), parentId: null, timestamp: "" }, draft),
    );
  }

  /**
   * Builds a Pi request from the fake session's persisted state.
   * @param {ModelRouteRequest<State>["reason"]} reason Route reason.
   * @returns {ModelRouteRequest<State>} Request.
   */
  #request(reason: ModelRouteRequest<State>["reason"]): ModelRouteRequest<State> {
    const request: ModelRouteRequest<State> = {
      messages: [],
      model: model("tier/test"),
      reason,
      signal: this.abort.signal,
      thinkingLevel: "high",
    };
    if (this.#state !== undefined) {
      request.state = this.#state;
    }
    if (reason === "retry" && this.#failed !== undefined) {
      request.failed = this.#failed;
    }
    return request;
  }

  /**
   * Makes a request through the real router.
   * @param {ModelRouteRequest<State>["reason"]} reason Pi route reason.
   * @param {boolean} freshUser Whether to append a real user entry.
   * @returns {Promise<string>} Dispatched reference.
   */
  public async dispatch(
    reason: ModelRouteRequest<State>["reason"] = "user",
    freshUser = reason === "user",
  ): Promise<string> {
    if (freshUser) {
      this.branch.push({
        id: crypto.randomUUID(),
        message: { content: "Real user prompt", role: "user", timestamp: 0 },
        parentId: null,
        timestamp: "",
        type: "message",
      });
    }
    const result = await this.router.route(this.targets, this.#request(reason), this.context()),
      reference = `${result.model.provider}/${result.model.id}`;
    this.#state = result.state;
    this.append({
      customType: VIRTUAL_MODEL_STATE_ENTRY,
      data: { modelId: "test", provider: "tier", state: result.state },
      type: "custom",
    });
    this.dispatched.push(reference);
    return reference;
  }

  /**
   * Runs native retry against an always-failing fake provider.
   * @param {string} text Provider error.
   * @returns {Promise<AssistantMessage>} Final native response.
   */
  public async native(text: string): Promise<AssistantMessage> {
    return retryAssistantCall(
      async () => {
        const reference = await this.dispatch(this.#failed === undefined ? "user" : "retry"),
          message = response(reference, text);
        this.#failed = { message, model: model(reference) };
        return message;
      },
      { baseDelayMs: 0, enabled: true, maxRetries: 1 },
      this.abort.signal,
    );
  }

  /**
   * Presents an error at Pi's final actionable boundary and persists its drafts.
   * @param {string} text Provider error text.
   * @param {"error" | "aborted"} outcome Run outcome.
   * @returns {Promise<boolean>} Whether recovery requests continuation.
   */
  public async settle(text: string, outcome: "error" | "aborted" = "error"): Promise<boolean> {
    const message = response(this.dispatched.at(LAST) ?? "fake/unknown", text),
      entry: SessionEntry = {
        id: crypto.randomUUID(),
        message,
        parentId: null,
        timestamp: "",
        type: "message",
      };
    this.branch.push(entry);
    const result = await this.recovery.settle(
      {
        context: {
          canContinue: false,
          contextEntries: [{ messages: [message], sourceEntry: entry }],
          contextMessages: [message],
          llmMessages: [message],
          pendingMessages: [],
        },
        continue: false,
        entries: [],
        outcome,
        type: "agent_before_settle",
      },
      this.context(),
    );
    for (const draft of result?.entries ?? []) {
      if (draft.type === "custom") {
        this.append(draft);
      }
      if (draft.type === "context_edit" && draft.replacement === null) {
        this.append({ replacement: null, targetId: draft.targetId, type: "context_edit" });
      }
    }
    return result?.continue === true;
  }
}

export { Harness, response };
