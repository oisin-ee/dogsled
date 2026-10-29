import type { Action, ConfigReport, Event, Facts, Selection, State } from "./wasm.ts";
import type { Api, Model } from "@earendil-works/pi-ai";
import type { BoundaryResult, SessionBoundaryDraft } from "@earendil-works/pi-coding-agent";
import type { Context, Files, Host } from "./ports.ts";
import { branchRecords, calls } from "./branch.ts";
import type { CallEvidence } from "./branch.ts";
import { activeTask } from "./tasks.ts";
import { decide } from "./wasm.ts";

/**
 * Resolves an exact qualified ID with configured credentials.
 * @param {string} id Qualified ID.
 * @param {Context} ctx Context.
 * @returns {Model<Api> | undefined} Available model.
 */
const model = (id: string, ctx: Context): Model<Api> | undefined =>
    ctx.modelRegistry
      .getAvailable()
      .find((candidate) => `${candidate.provider}/${candidate.id}` === id),
  /**
   * Records pi's effective thinking after selection.
   * @param {State} state Proposed state.
   * @param {Selection | undefined} selected Effective selection.
   * @returns {State} Effective state.
   */
  effectiveState = (state: State, selected: Selection | undefined): State => {
    if (selected && state.kind === "handoff") {
      return Object.assign({}, state, { target: selected });
    }
    return selected && state.kind === "arm"
      ? Object.assign({}, state, { planner: selected })
      : state;
  },
  /**
   * Maps one action to a pi draft.
   * @param {Action} action Core action.
   * @param {Selection | undefined} selected Effective selection.
   * @returns {SessionBoundaryDraft[]} Zero or one draft.
   */
  drafts = (action: Action, selected: Selection | undefined): SessionBoundaryDraft[] => {
    switch (action.kind) {
      case "store": {
        return [
          {
            customType: `prewalk-${action.state.kind}`,
            data: effectiveState(action.state, selected),
            type: "custom",
          },
        ];
      }
      case "prompt": {
        return [
          {
            content: action.content,
            customType: action.name,
            display: false,
            type: "custom_message",
          },
        ];
      }
      case "scrub": {
        return [{ replacement: null, targetId: action.id, type: "context_edit" }];
      }
      case "select":
      case "notify": {
        return [];
      }
      default: {
        return [];
      }
    }
  },
  /**
   * Persists non-boundary entries. Scrubs persist at the next boundary.
   * @param {Host} pi Host authority.
   * @param {readonly SessionBoundaryDraft[]} entries Drafts.
   */
  persist = (pi: Host, entries: readonly SessionBoundaryDraft[]): void => {
    for (const entry of entries) {
      if (entry.type === "custom") {
        pi.appendEntry(entry.customType, entry.data);
      } else if (entry.type === "custom_message") {
        pi.sendMessage(entry);
      }
    }
  };

/** No in-memory arm, gate, handoff, or continuation state. */
class Adapter {
  readonly #pi: Host;
  readonly #files: Files;
  readonly #notified = new Set<string>();

  /**
   * @param {Host} pi Session authority.
   * @param {Files} files File readers.
   */
  public constructor(pi: Host, files: Files) {
    this.#pi = pi;
    this.#files = files;
  }

  /**
   * @param {Event} event Protocol event.
   * @param {Context} ctx Context.
   * @returns {Facts} Boundary input.
   */
  #facts(event: Event, ctx: Context): Facts {
    const report = this.#files.config(ctx),
      into = event.kind === "start" && event.reason === "startup" ? event.flags.into : undefined,
      target = into ?? report.config.target?.model,
      todo = this.#pi
        .getAllTools()
        .find((tool) => tool.name === "todo" && tool.exposure !== "hidden"),
      callable =
        todo &&
        (todo.exposure === "codemode" ||
          todo.exposure === "deferred" ||
          this.#pi.getActiveTools().includes("todo"));
    return {
      active_unit: activeTask(this.#pi.events),
      available: ctx.modelRegistry
        .getAvailable()
        .map((candidate) => `${candidate.provider}/${candidate.id}`),
      branch: branchRecords(ctx.sessionManager.getBranch()),
      current: ctx.model
        ? { model: `${ctx.model.provider}/${ctx.model.id}`, thinking: this.#pi.getThinkingLevel() }
        : undefined,
      default: this.#default(ctx),
      event,
      previous: [],
      report,
      target_available: target !== undefined && model(target, ctx) !== undefined,
      todo: Boolean(callable),
      todo_owner: todo?.sourceInfo.path ?? "none",
    };
  }

  /**
   * @param {Context} ctx Context.
   * @returns {Selection | undefined} Usable default.
   */
  #default(ctx: Context): Selection | undefined {
    const settings = this.#pi.getSettings(),
      id = `${settings.defaultProvider ?? ""}/${settings.defaultModel ?? ""}`;
    return model(id, ctx)
      ? { model: id, thinking: settings.defaultThinkingLevel ?? "medium" }
      : undefined;
  }

  /**
   * @param {ConfigReport} report Config report.
   * @param {Context} ctx Context.
   */
  #errors(report: ConfigReport, ctx: Context): void {
    for (const error of report.errors) {
      if (!this.#notified.has(error)) {
        this.#notified.add(error);
        ctx.ui.notify(error, "warning");
      }
    }
  }

  /**
   * @param {Event} event Event.
   * @param {Context} ctx Context.
   * @param {readonly SessionBoundaryDraft[] | undefined} prior Existing boundary drafts.
   * @returns {Promise<BoundaryResult>} Boundary result.
   */
  public async run(
    event: Event,
    ctx: Context,
    prior?: readonly SessionBoundaryDraft[],
  ): Promise<BoundaryResult> {
    const facts = this.#facts(event, ctx);
    this.#errors(facts.report, ctx);
    return this.#apply(facts, ctx, prior);
  }

  /**
   * Excludes this turn's persisted results from the historical gate evidence.
   * @param {TurnEndEvent} event Completed turn.
   * @param {Context} ctx Context.
   * @returns {Promise<BoundaryResult>} Boundary result.
   */
  public async turn(
    event: Readonly<{
      toolResults: readonly CallEvidence[];
      toolResultEntryIds: readonly string[];
      entries: readonly SessionBoundaryDraft[];
    }>,
    ctx: Context,
  ): Promise<BoundaryResult> {
    const facts = this.#facts(
      { calls: event.toolResults.flatMap((result) => calls(result)), kind: "turn" },
      ctx,
    );
    facts.branch = branchRecords(
      ctx.sessionManager
        .getBranch()
        .filter((entry) => !event.toolResultEntryIds.includes(entry.id)),
    );
    this.#errors(facts.report, ctx);
    return this.#apply(facts, ctx, event.entries);
  }

  /**
   * @param {string | undefined} path Previous session file.
   * @param {Context} ctx Fresh context.
   * @returns {Promise<void>} Completion.
   */
  public async newSession(path: string | undefined, ctx: Context): Promise<void> {
    const facts = this.#facts(
      {
        flags: { into: undefined, no_prewalk: false, prewalk: false },
        kind: "start",
        reason: "new",
      },
      ctx,
    );
    this.#errors(facts.report, ctx);
    try {
      facts.previous = path === undefined ? [] : branchRecords(this.#files.previous(path));
    } catch (error) {
      ctx.ui.notify(
        `Prewalk: cannot read previous session: ${error instanceof Error ? error.message : String(error)}`,
        "warning",
      );
    }
    await this.#apply(facts, ctx);
  }

  /**
   * @param {Selection} selection Selection.
   * @param {Context} ctx Context.
   * @returns {Promise<Selection | undefined>} Effective selection, or failure.
   */
  async #select(selection: Selection, ctx: Context): Promise<Selection | null> {
    const resolved = model(selection.model, ctx);
    if (!resolved || !(await this.#pi.setModel(resolved))) {
      ctx.ui.notify(
        `Prewalk: cannot select ${selection.model}. Active model stays unchanged.`,
        "warning",
      );
      return null;
    }
    this.#pi.setThinkingLevel(selection.thinking);
    return { model: selection.model, thinking: this.#pi.getThinkingLevel() };
  }

  /**
   * @param {readonly Action[]} actions Decisions.
   * @param {Context} ctx Context.
   */
  static #notices(actions: readonly Action[], ctx: Context): void {
    for (const action of actions) {
      if (action.kind === "notify") {
        ctx.ui.notify(action.message, "info");
      }
    }
  }

  /**
   * Maps a protocol result to pi. Core decisions contain at most one selection.
   * @param {Facts} facts Input.
   * @param {Context} ctx Context.
   * @param {readonly SessionBoundaryDraft[] | undefined} prior Existing boundary drafts.
   * @returns {Promise<BoundaryResult>} Boundary result.
   */
  async #apply(
    facts: Facts,
    ctx: Context,
    prior?: readonly SessionBoundaryDraft[],
  ): Promise<BoundaryResult> {
    const decision = decide(facts),
      selection = decision.actions.find((action) => action.kind === "select"),
      selected = selection ? await this.#select(selection.selection, ctx) : undefined,
      entries = [
        ...(prior ?? []),
        ...decision.actions.flatMap((action) => drafts(action, selected ?? undefined)),
      ];
    if (selection && !selected) {
      if (facts.event.kind === "start" && facts.event.reason === "new") {
        return this.#apply(Object.assign({}, facts, { previous: [] }), ctx, prior);
      }
      return { entries: [...(prior ?? [])] };
    }
    Adapter.#notices(decision.actions, ctx);
    if (prior === undefined) {
      persist(this.#pi, entries);
    }
    return decision.again ? { continue: true, entries } : { entries };
  }
}

export { Adapter };
