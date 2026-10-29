import type {
  BoundaryResult,
  ExtensionContext,
  ExtensionUIContext,
  SessionBoundaryDraft,
  SessionEntry,
  Theme,
} from "@earendil-works/pi-coding-agent";
import { REMINDER_ENTRY, reminder } from "./reminder.ts";
import { parse, view, widget } from "./wasm.ts";
import { TodoWidget } from "./widget.ts";
import { isOwnSource } from "./source.ts";
import { latestSnapshot } from "./state.ts";

interface TodoContext {
  readonly mode: ExtensionContext["mode"];
  readonly sessionManager: { readonly getBranch: () => readonly SessionEntry[] };
  readonly ui: Readonly<Pick<ExtensionUIContext, "notify" | "setWidget">>;
}
interface SettleEvent {
  readonly outcome: "completed" | "aborted" | "error";
  readonly continue: boolean;
  readonly context: { readonly canContinue: boolean };
  readonly entries: readonly SessionBoundaryDraft[];
}
interface TodoHost {
  readonly getAllTools: () => readonly {
    readonly name: string;
    readonly sourceInfo: { readonly path: string };
  }[];
}

/**
 * Whether this source owns the todo registration.
 * @param {TodoHost} pi Tool metadata reader.
 * @returns {boolean} Whether this source won.
 */
const ownsTodo = (pi: TodoHost): boolean =>
    pi.getAllTools().some((tool) => tool.name === "todo" && isOwnSource(tool.sourceInfo.path)),
  /**
   * Renders the selected branch snapshot, and removes the widget when the list is empty.
   * @param {TodoContext} ctx Branch and UI methods.
   * @param {boolean} expanded Whether to reveal secondary task groups.
   */
  replay = (ctx: TodoContext, expanded = false): void => {
    if (ctx.mode !== "tui") {
      return;
    }
    const list = parse(latestSnapshot(ctx.sessionManager.getBranch())),
      rows = widget(list, expanded);
    ctx.ui.setWidget(
      "pi-todo",
      rows === undefined
        ? undefined
        : (_tui: unknown, theme: Readonly<Pick<Theme, "fg" | "bold">>): TodoWidget =>
            new TodoWidget(rows, expanded, theme),
    );
  },
  /**
   * Applies the completion decision at pi's final actionable boundary.
   * @param {TodoHost} pi Tool metadata reader.
   * @param {SettleEvent} event The boundary facts.
   * @param {TodoContext} ctx Branch and UI methods.
   * @returns {BoundaryResult} The unchanged boundary or an added visible reminder.
   */
  settle = (pi: TodoHost, event: SettleEvent, ctx: TodoContext): BoundaryResult => {
    if (event.outcome !== "completed" || event.continue || !ownsTodo(pi)) {
      return {};
    }
    const content = reminder(ctx.sessionManager.getBranch(), event.entries);
    return typeof content === "string"
      ? {
          continue: true,
          entries: [
            ...event.entries,
            { content, customType: REMINDER_ENTRY, display: true, type: "custom_message" },
          ],
        }
      : {};
  },
  /**
   * Prints the branch list for /todos.
   * @param {TodoContext} ctx Branch and UI methods.
   */
  show = (ctx: TodoContext): void => {
    const list = parse(latestSnapshot(ctx.sessionManager.getBranch()));
    ctx.ui.notify(view(list), "info");
  },
  /**
   * Replays or reports the winning source at startup.
   * @param {TodoHost} pi Tool metadata reader.
   * @param {TodoContext} ctx Branch and UI methods.
   */
  start = (pi: TodoHost, ctx: TodoContext): void => {
    if (ownsTodo(pi)) {
      replay(ctx);
    } else {
      const owner = pi.getAllTools().find((tool) => tool.name === "todo"),
        source = owner ? owner.sourceInfo.path : "another source";
      ctx.ui.notify(`pi-todo is inactive: todo belongs to ${source}.`, "warning");
    }
  };

export { ownsTodo, replay, settle, show, start };
export type { SettleEvent, TodoContext, TodoHost };
