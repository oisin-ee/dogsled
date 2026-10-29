import type { TodoContext, TodoHost } from "./lifecycle.ts";
import { ownsTodo, replay, start } from "./lifecycle.ts";

/** Owns transient expansion for one extension instance, never task state. */
class TodoPresentation {
  #expanded = false;
  #external = false;

  /**
   * Replays current branch tasks without changing expansion.
   * @param {TodoContext} ctx Current branch and UI.
   */
  public refresh(ctx: TodoContext): void {
    if (!this.#external) {
      replay(ctx, this.#expanded);
    }
  }

  /**
   * Collapses when the session starts or the selected branch changes.
   * @param {TodoContext} ctx Current branch and UI.
   */
  public reset(ctx: TodoContext): void {
    this.#expanded = false;
    this.refresh(ctx);
  }

  /**
   * Starts collapsed, or leaves another todo owner untouched.
   * @param {TodoHost} pi Registered tool metadata.
   * @param {TodoContext} ctx Current branch and UI.
   */
  public start(pi: TodoHost, ctx: TodoContext): void {
    this.#expanded = false;
    this.#external = false;
    start(pi, ctx);
  }

  /**
   * Yields only this extension's widget to an acknowledged work presenter.
   * @param {TodoContext} ctx Current terminal UI.
   */
  public claim(ctx: TodoContext): void {
    this.#external = true;
    ctx.ui.setWidget("pi-todo", undefined);
  }

  /**
   * Restores the standalone view without changing task state.
   * @param {TodoContext} ctx Current terminal UI.
   */
  public release(ctx: TodoContext): void {
    this.#external = false;
    this.refresh(ctx);
  }

  /**
   * Reveals or hides secondary groups without writing session state.
   * @param {TodoHost} pi Registered tool metadata.
   * @param {TodoContext} ctx Current branch and UI.
   */
  public toggle(pi: TodoHost, ctx: TodoContext): void {
    if (ownsTodo(pi)) {
      this.#expanded = !this.#expanded;
      this.refresh(ctx);
    }
  }
}

export { TodoPresentation };
