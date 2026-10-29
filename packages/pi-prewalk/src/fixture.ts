import type { Api, Model, ToolResultMessage } from "@earendil-works/pi-ai";
import type { Context, Host } from "./ports.ts";
import type { SessionBoundaryDraft, SessionEntry, ToolInfo } from "@earendil-works/pi-coding-agent";
import { Adapter } from "./adapter.ts";
import { calls } from "./branch.ts";
import { configure } from "./wasm.ts";
import { createEventBus } from "@earendil-works/pi-coding-agent";

/** Exact thinking replacement used by lifecycle tests. */
interface Replacement {
  readonly id: string;
  readonly parentId: null;
  readonly thinkingLevel: "low";
  readonly timestamp: string;
  readonly type: "thinking_level_change";
}
/** Read-only observations and explicit test-session operations. */
interface Harness {
  readonly adapter: Pick<Adapter, "run" | "newSession" | "turn">;
  readonly events: Host["events"];
  readonly apply: (entries: readonly SessionBoundaryDraft[] | undefined) => void;
  readonly branch: readonly SessionEntry[];
  readonly config: (source: string) => void;
  readonly ctx: Context;
  readonly newBranch: (replacement?: Replacement) => void;
  readonly notices: readonly string[];
  readonly selections: readonly string[];
  readonly todo: (active: boolean) => void;
}

/**
 * Creates one complete registry model.
 * @param {string} id Model ID.
 * @returns {Model<Api>} Model.
 */
const model = (id: string): Model<Api> => ({
  api: "openai-responses",
  baseUrl: "https://example.test",
  contextWindow: 128_000,
  cost: { cacheRead: 0, cacheWrite: 0, input: 0, output: 0 },
  id,
  input: ["text"],
  maxTokens: 16_384,
  name: id,
  provider: "test",
  reasoning: true,
});

/** In-memory pi authority. The protocol itself runs in the real WASM module. */
class Session {
  readonly #branch: SessionEntry[] = [];
  readonly #events = createEventBus();
  readonly #previous: SessionEntry[] = [];
  readonly #notices: string[] = [];
  readonly #selections: string[] = [];
  readonly #models = [model("planner"), model("worker"), model("default")];
  #active = "planner";
  #source = '{"target":{"model":"test/worker","thinking":"medium"}}';
  #todo = true;
  #thinking: ReturnType<Host["getThinkingLevel"]> = "high";

  /**
   * Appends a boundary draft.
   * @param {SessionBoundaryDraft} entry Draft.
   */
  #append(entry: SessionBoundaryDraft): void {
    if (
      entry.type === "custom" ||
      entry.type === "custom_message" ||
      entry.type === "context_edit"
    ) {
      this.#branch.push(
        Object.assign({ id: crypto.randomUUID(), parentId: null, timestamp: "" }, entry),
      );
    }
  }

  /**
   * Configures a callable nested todo tool.
   * @returns {ToolInfo[]} Tools.
   */
  #tools(): ToolInfo[] {
    return this.#todo
      ? [
          {
            description: "todo",
            exposure: "codemode",
            name: "todo",
            parameters: {},
            sourceInfo: {
              origin: "top-level",
              path: "test/todo",
              scope: "temporary",
              source: "extension",
            },
          },
        ]
      : [];
  }

  /**
   * Creates host authority with observable model changes.
   * @returns {Host} Host.
   */
  #host(): Host {
    return {
      appendEntry: (customType, data) => {
        this.#append({ customType, data, type: "custom" });
      },
      events: this.#events,
      getActiveTools: () => ["read", "edit", "write"],
      getAllTools: () => this.#tools(),
      getSettings: () => ({
        defaultModel: "default",
        defaultProvider: "test",
        defaultThinkingLevel: "low",
      }),
      getThinkingLevel: () => this.#thinking,
      sendMessage: () => {
        throw new Error("Protocol prompts must use boundary drafts.");
      },
      setModel: async (next: Model<Api>) => {
        this.#active = next.id;
        this.#selections.push(`${next.provider}/${next.id}`);
        return true;
      },
      setThinkingLevel: (next) => {
        this.#thinking = next;
      },
    };
  }

  /**
   * Creates read-only context over this session.
   * @returns {Context} Context.
   */
  #context(): Context {
    const active = (): Model<Api> | undefined =>
      this.#models.find((candidate) => candidate.id === this.#active);
    return {
      cwd: "/scratch",
      isProjectTrusted: () => true,
      get model(): Model<Api> | undefined {
        return active();
      },
      modelRegistry: { getAvailable: (): Model<Api>[] => this.#models },
      sessionManager: { getBranch: (): SessionEntry[] => this.#branch },
      ui: {
        notify: (message: string): void => {
          this.#notices.push(message);
        },
      },
    };
  }

  /**
   * Moves the active branch into the previous-session reader.
   * @param {Replacement | undefined} replacement Later thinking change.
   */
  #newBranch(replacement: Replacement | undefined): void {
    this.#previous.push(...this.#branch);
    if (replacement) {
      this.#previous.push(Object.assign({}, replacement));
    }
    this.#branch.length = 0;
  }

  /**
   * Exposes only the operations and observations tests need.
   * @returns {Harness} Test interface.
   */
  public harness(): Harness {
    return {
      adapter: new Adapter(this.#host(), {
        config: (): ReturnType<typeof configure> => configure(this.#source),
        previous: (): SessionEntry[] => this.#previous,
      }),
      apply: (entries) => {
        for (const entry of entries ?? []) {
          this.#append(entry);
        }
      },
      branch: this.#branch,
      config: (source) => {
        this.#source = source;
      },
      ctx: this.#context(),
      events: this.#events,
      newBranch: (replacement) => {
        this.#newBranch(replacement);
      },
      notices: this.#notices,
      selections: this.#selections,
      todo: (active) => {
        this.#todo = active;
      },
    };
  }
}

/**
 * Creates a test session.
 * @returns {Harness} Session.
 */
const harness = (): Harness => new Session().harness(),
  /**
   * Arms from startup flags.
   * @param {Harness} test Session.
   * @returns {Promise<void>} Completion.
   */
  arm = async (test: Harness): Promise<void> => {
    await test.adapter.run(
      {
        flags: { into: undefined, no_prewalk: false, prewalk: true },
        kind: "start",
        reason: "startup",
      },
      test.ctx,
    );
  },
  /**
   * Advances a turn and persists its drafts.
   * @param {Harness} test Session.
   * @param {readonly CallResult[]} results Tool results.
   * @returns {Promise<void>} Completion.
   */
  turn = async (test: Harness, results: readonly CallResult[]): Promise<void> => {
    const boundary = await test.adapter.run(
      { calls: results.flatMap((entry) => calls(entry)), kind: "turn" },
      test.ctx,
      [],
    );
    test.apply(boundary.entries);
  },
  /**
   * Arms and injects the first nudge.
   * @param {Harness} test Session.
   * @returns {Promise<void>} Completion.
   */
  nudge = async (test: Harness): Promise<void> => {
    await arm(test);
    await turn(test, []);
  },
  /**
   * Makes a completed tool result.
   * @param {string} name Tool name.
   * @param {boolean} error Error status.
   * @returns {ToolResultMessage} Result.
   */
  result = (name: string, error: boolean): ToolResultMessage => ({
    content: [],
    isError: error,
    role: "toolResult",
    timestamp: 0,
    toolCallId: "call",
    toolName: name,
  });

/** Tool fields that the protocol reads. */
interface CallResult {
  readonly toolName: string;
  readonly isError: boolean;
  readonly nestedCalls?: {
    readonly calls: readonly Readonly<{ id: string; name: string; status: string }>[];
    readonly complete: boolean;
  };
}

export { arm, harness, nudge, result, turn };
export type { Harness };
