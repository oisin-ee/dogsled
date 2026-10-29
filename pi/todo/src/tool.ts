import { STATE_ENTRY, branchSnapshot } from "./state.ts";
import { apply, view } from "./wasm.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { Static } from "typebox";
import type { TodoList } from "./wasm.ts";
import type { parameters } from "./schema.ts";
import { workUnitGuidance } from "./guidance.ts";

/** One `todo` call, as the schema in `index.ts` admits it. */
type TodoParams = Static<typeof parameters>;

/** What a call reads from and writes to the session. */
interface TodoSession {
  readonly sessionId: string;
  readonly branch: () => readonly SessionEntry[];
  readonly append: (customType: string, list: TodoList) => void;
}

/** The tool result: the summary for the model and the list for rendering. */
interface TodoResult {
  readonly content: [{ readonly type: "text"; readonly text: string }];
  readonly details: TodoList;
}

/**
 * The result for `list`.
 * @param {TodoList} list A valid list.
 * @param {boolean} creation Whether the call created work.
 * @returns {TodoResult} The summary and the list.
 */
const result = (list: TodoList, creation: boolean): TodoResult => ({
    content: [
      { text: creation ? `${view(list)}\n\n${workUnitGuidance}` : view(list), type: "text" },
    ],
    details: list,
  }),
  /**
   * Runs one call. A rejected operation throws the core's message, so pi marks
   * the result failed, and nothing is stored.
   * @param {TodoParams} params The validated tool arguments.
   * @param {TodoSession} session The branch to read and the entry writer.
   * @returns {TodoResult} The result for the model and the renderer.
   */
  runTodo = (params: TodoParams, session: TodoSession): TodoResult => {
    const current = branchSnapshot(session.branch()),
      { list, persist } = apply(current, params, {
        mutationId: crypto.randomUUID(),
        sessionId: session.sessionId,
      });
    if (persist) {
      session.append(STATE_ENTRY, list);
    }
    return result(
      persist ? branchSnapshot(session.branch()) : list,
      params.op === "init" || params.op === "append",
    );
  };

export { runTodo };
export type { TodoParams, TodoResult, TodoSession };
