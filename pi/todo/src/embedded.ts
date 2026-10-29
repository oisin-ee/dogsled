import { Key, ScrollView, matchesKey } from "@earendil-works/pi-tui";
import { parse, widget } from "./wasm.ts";
import type { TodoList } from "./wasm.ts";
import { TodoWidget } from "./widget.ts";
import type { ViewHost } from "./display.ts";

const minimumHeight = 1,
  previousLine = -1,
  nextLine = 1;

/** Inline viewport over the same renderer used by the standalone widget. */
class TodoView {
  readonly #host: ViewHost;
  #list: TodoList;
  #expanded = true;
  #disposed = false;
  readonly #body: TodoWidget;
  readonly #scroll: ScrollView;

  /**
   * Creates an expansion-local view without event subscriptions or task writes.
   * @param {TodoList} list Validated branch tasks.
   * @param {ViewHost} host Host layout and rendering services.
   */
  public constructor(list: TodoList, host: ViewHost) {
    this.#host = host;
    this.#list = list;
    this.#body = this.#content();
    this.#scroll = new ScrollView(this.#body, { overscroll: "contain", scrollbar: "hidden" });
  }

  /**
   * Creates the shared widget content.
   * @returns {TodoWidget} Theme-aware rows.
   */
  #content(): TodoWidget {
    const body = new TodoWidget(
      widget(this.#list, this.#expanded) ?? [],
      this.#expanded,
      this.#host.theme,
    );
    body.setShortcut("Alt+I");
    return body;
  }

  /**
   * Applies a provider snapshot while preserving the viewport.
   * @param {unknown} state The opaque state from the display capability.
   */
  public update(state: unknown): void {
    if (!this.#disposed) {
      this.#list = parse(state);
      this.#body.setRows(widget(this.#list, this.#expanded) ?? [], this.#expanded);
      this.invalidate();
    }
  }

  /**
   * Renders a bounded, scrollable view with the active host theme.
   * @param {number} width Available terminal columns.
   * @returns {string[]} Visible terminal lines.
   */
  public render(width: number): string[] {
    if (this.#disposed) {
      return [];
    }
    const height = Math.max(minimumHeight, this.#host.height());
    this.#scroll.updateLayout(this.#body.render(width).length, height, this.#host.requestRender);
    return this.#scroll
      .render(width)
      .slice(this.#scroll.scrollTop, this.#scroll.scrollTop + height);
  }

  /**
   * Handles only navigation forwarded by the inline host.
   * @param {string} input A translated host shortcut.
   */
  public handleInput(input: string): void {
    if (this.#disposed) {
      return;
    }
    if (matchesKey(input, Key.enter)) {
      this.#expanded = !this.#expanded;
      this.#body.setRows(widget(this.#list, this.#expanded) ?? [], this.#expanded);
    } else if (matchesKey(input, Key.up)) {
      this.#scroll.scrollBy(previousLine);
    } else if (matchesKey(input, Key.down)) {
      this.#scroll.scrollBy(nextLine);
    }
    this.#host.requestRender();
  }

  /** Invalidates both native components after a theme or data change. */
  public invalidate(): void {
    this.#body.invalidate();
    this.#scroll.invalidate();
  }

  /** Ends this view's lifetime without modifying task state. */
  public dispose(): void {
    this.#disposed = true;
  }
}

export { TodoView };
