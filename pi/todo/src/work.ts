import type { TodoContext, TodoHost } from "./lifecycle.ts";
import { ownsTodo, show } from "./lifecycle.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { TODO_SHORTCUT } from "./widget.ts";
import type { TodoDisplay } from "./display.ts";
import type { TodoPresentation } from "./presentation.ts";
import { display } from "./display.ts";
import { latestSnapshot } from "./state.ts";
import { parse } from "./wasm.ts";

type WorkHost = TodoHost;

type WorkCommands = Readonly<Pick<ExtensionAPI, "registerCommand" | "registerShortcut">>;
const initialRevision = 0;

interface WorkContext extends TodoContext {
  readonly sessionManager: TodoContext["sessionManager"] & {
    readonly getSessionId: () => string;
    readonly getLeafId: () => string | null;
  };
}

interface TodoPresenter {
  readonly show: (snapshot: {
    readonly session: string;
    readonly branch: string | null;
    readonly revision: number;
    readonly display: TodoDisplay;
  }) => boolean;
  readonly open: () => void;
  readonly stop: () => void;
}

/** Publishes validated branch data; presentation claims never write task state. */
class TodoWork {
  readonly #host: WorkHost;
  readonly #presentation: Readonly<TodoPresentation>;
  #context: WorkContext | undefined;
  #revision = initialRevision;
  #presenter: TodoPresenter | undefined;
  #claimed = false;

  /**
   * Keeps tool ownership and the existing standalone presentation owner.
   * @param {WorkHost} host Pi tool ownership.
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
   * Starts only when this extension owns the terminal todo tool.
   * @param {WorkContext} ctx Current Pi session.
   */
  public start(ctx: WorkContext): void {
    if (ctx.mode !== "tui" || !ownsTodo(this.#host)) {
      this.stop();
      return;
    }
    if (this.#claimed) {
      this.#release();
    }
    this.#context = ctx;
    this.publish(ctx);
  }

  /**
   * Attaches the shared presenter and publishes an already started session.
   * @param {TodoPresenter} presenter Shared work display owner.
   * @returns {Function} Releases this attachment.
   */
  public attach(presenter: TodoPresenter): () => void {
    if (this.#presenter !== undefined) {
      this.#release();
    }
    this.#presenter = presenter;
    if (this.#context !== undefined) {
      this.publish(this.#context);
    }
    return (): void => {
      if (this.#presenter === presenter) {
        this.#presenter = undefined;
        this.#release();
      }
    };
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
    const list = parse(latestSnapshot(ctx.sessionManager.getBranch())),
      claimed = this.#presenter.show({
        branch: ctx.sessionManager.getLeafId(),
        display: display(list),
        revision: this.#revision,
        session: ctx.sessionManager.getSessionId(),
      });
    if (claimed) {
      this.#claimed = true;
      this.#presentation.claim(ctx);
    } else if (this.#claimed) {
      this.#release();
    }
  }

  /**
   * Requests the read-only Todos view without changing expansion or task state.
   * @returns {boolean} Whether a shared presenter owns the view.
   */
  public open(): boolean {
    if (!this.#claimed || this.#presenter === undefined) {
      return false;
    }
    this.#presenter.open();
    return true;
  }

  /** Stops shared presentation and restores the standalone display. */
  public stop(): void {
    this.#presenter?.stop();
    this.#presenter = undefined;
    this.#release();
    this.#context = undefined;
  }

  /** Restores the standalone display when shared presentation ends. */
  #release(): void {
    this.#claimed = false;
    if (this.#context !== undefined) {
      this.#presentation.release(this.#context);
    }
  }
}

export { TodoWork };
export type { TodoPresenter, WorkContext, WorkHost };
