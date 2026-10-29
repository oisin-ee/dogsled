import type { TodoContext, TodoHost } from "./lifecycle.ts";
import { ownsTodo, show } from "./lifecycle.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { TODO_SHORTCUT } from "./widget.ts";
import type { TodoPresentation } from "./presentation.ts";
import { display } from "./display.ts";
import { latestSnapshot } from "./state.ts";
import { parse } from "./wasm.ts";

interface WorkHost extends TodoHost {
  readonly events: Readonly<ExtensionAPI["events"]>;
}

type WorkCommands = Readonly<Pick<ExtensionAPI, "registerCommand" | "registerShortcut">>;
const protocolVersion = 3,
  initialRevision = 0;

interface WorkContext extends TodoContext {
  readonly sessionManager: TodoContext["sessionManager"] & {
    readonly getSessionId: () => string;
    readonly getLeafId: () => string | null;
  };
}

/** The identity of one work presenter requesting this provider's snapshot. */
interface Presenter {
  readonly presenter: string;
  readonly request: string;
  readonly version: typeof protocolVersion;
}

/**
 * Checks a presentation request without trusting an extension's event payload.
 * @param {unknown} value Event payload.
 * @returns {boolean} Whether the envelope uses the supported protocol.
 */
const isPresenter = (value: unknown): value is Presenter =>
  typeof value === "object" &&
  value !== null &&
  "version" in value &&
  value.version === protocolVersion &&
  "presenter" in value &&
  typeof value.presenter === "string" &&
  "request" in value &&
  typeof value.request === "string";

/** Publishes validated branch data; presentation claims never write task state. */
class TodoWork {
  readonly #host: WorkHost;
  readonly #presentation: Readonly<TodoPresentation>;
  #context: WorkContext | undefined;
  #publisher = "";
  #revision = initialRevision;
  #presenter: Presenter | undefined;
  #claimed = false;
  #subscriptions: (() => void)[] = [];

  /**
   * Keeps the event bus and the existing standalone presentation owner.
   * @param {WorkHost} host Pi event bus and tool ownership.
   * @param {Readonly<TodoPresentation>} presentation Standalone display owner.
   */
  public constructor(host: WorkHost, presentation: Readonly<TodoPresentation>) {
    this.#host = host;
    this.#presentation = presentation;
  }

  /**
   * Registers view entrypoints with a shared or standalone presentation target.
   * @param {WorkCommands} commands Pi command registration.
   */
  public register(commands: WorkCommands): void {
    commands.registerShortcut(TODO_SHORTCUT, {
      description: "Show todo details.",
      handler: (ctx): void => {
        this.#toggle(ctx);
      },
    });
    commands.registerCommand("todos", {
      description: "Show todo details; /todos list prints the full branch list.",
      handler: async (args, ctx): Promise<void> => {
        if (ownsTodo(this.#host)) {
          if (args.trim() === "list" || ctx.mode !== "tui") {
            show(ctx);
          } else {
            this.#toggle(ctx);
          }
        }
      },
    });
  }

  /**
   * Opens the shared view or toggles the existing standalone view.
   * @param {TodoContext} ctx Current UI and branch.
   */
  #toggle(ctx: TodoContext): void {
    if (!this.open()) {
      this.#presentation.toggle(this.#host, ctx);
    }
  }

  /**
   * Subscribes only when this extension owns the terminal todo tool.
   * @param {WorkContext} ctx Current Pi session.
   */
  public start(ctx: WorkContext): void {
    this.stop();
    if (ctx.mode !== "tui" || !ownsTodo(this.#host)) {
      return;
    }
    this.#context = ctx;
    this.#publisher = crypto.randomUUID();
    this.#revision = initialRevision;
    this.#subscriptions = [
      this.#host.events.on("oisin.work.request", (value: unknown): void => {
        if (isPresenter(value)) {
          this.#presenter = value;
          if (this.#context !== undefined) {
            this.publish(this.#context);
          }
        }
      }),
      this.#host.events.on("oisin.work.claim", (value: unknown): void => {
        this.#claim(value);
      }),
      this.#host.events.on("oisin.work.release", (value: unknown): void => {
        if (this.#matches(value)) {
          this.#release();
        }
      }),
    ];
    this.#host.events.emit("oisin.todo.ready", {
      publisher: this.#publisher,
      version: protocolVersion,
    });
  }

  /**
   * Publishes the actual branch snapshot after tool or branch changes.
   * @param {WorkContext} ctx Current Pi session, never a stored file's last entry.
   */
  public publish(ctx: WorkContext): void {
    if (this.#context === undefined || this.#presenter === undefined) {
      return;
    }
    this.#context = ctx;
    this.#revision += 1;
    const list = parse(latestSnapshot(ctx.sessionManager.getBranch()));
    this.#host.events.emit("oisin.todo.snapshot", {
      branch: ctx.sessionManager.getLeafId(),
      display: display(list),
      presenter: this.#presenter.presenter,
      publisher: this.#publisher,
      request: this.#presenter.request,
      revision: this.#revision,
      session: ctx.sessionManager.getSessionId(),
      version: protocolVersion,
    });
  }

  /**
   * Requests the read-only Todos view without changing expansion or task state.
   * @returns {boolean} Whether a shared presenter owns the view.
   */
  public open(): boolean {
    if (!this.#claimed || this.#presenter === undefined) {
      return false;
    }
    this.#host.events.emit("oisin.work.open", {
      presenter: this.#presenter.presenter,
      request: this.#presenter.request,
      version: protocolVersion,
      view: "todos",
    });
    return true;
  }

  /** Releases subscriptions and tells the presenter to discard this provider. */
  public stop(): void {
    for (const unsubscribe of this.#subscriptions) {
      unsubscribe();
    }
    this.#subscriptions = [];
    if (this.#context !== undefined) {
      this.#host.events.emit("oisin.todo.stop", {
        publisher: this.#publisher,
        version: protocolVersion,
      });
    }
    this.#release();
    this.#context = undefined;
  }

  /**
   * Accepts only an acknowledgement of the latest published snapshot.
   * @param {unknown} value Claim envelope.
   */
  #claim(value: unknown): void {
    if (
      this.#matches(value) &&
      "publisher" in value &&
      value.publisher === this.#publisher &&
      "revision" in value &&
      value.revision === this.#revision &&
      this.#context !== undefined
    ) {
      this.#claimed = true;
      this.#presentation.claim(this.#context);
    }
  }

  /**
   * Matches a live presenter, excluding delayed messages from replaced sessions.
   * @param {unknown} value Event envelope.
   * @returns {boolean} Whether the envelope belongs to the current presenter.
   */
  #matches(value: unknown): value is Presenter {
    return (
      isPresenter(value) &&
      value.presenter === this.#presenter?.presenter &&
      value.request === this.#presenter.request
    );
  }

  /** Restores the standalone display when shared presentation ends. */
  #release(): void {
    this.#claimed = false;
    this.#presenter = undefined;
    if (this.#context !== undefined) {
      this.#presentation.release(this.#context);
    }
  }
}

export { TodoWork };
export type { WorkContext, WorkHost };
