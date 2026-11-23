import { apply, view } from "./wasm.ts";
import { branchSnapshot, saveState } from "./state.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { Static } from "typebox";
import type { TodoDelta } from "./state.ts";
import type { TodoList } from "./wasm.ts";
import { mutationSummary } from "./tool-summary.ts";
import type { parameters } from "./schema.ts";

/** One `todo` call, as the schema in `index.ts` admits it. */
type TodoParams = Static<typeof parameters>;

/** What a call reads from and writes to the session. */
interface TodoSession {
  readonly sessionId: string;
  readonly branch: () => readonly SessionEntry[];
  readonly append: (customType: string, data: TodoList | TodoDelta, list: TodoList) => void;
}

/** The tool result: the summary for the model and the list for rendering. */
interface TodoResult {
  readonly content: [{ readonly type: "text"; readonly text: string }];
  readonly details: TodoList;
}

/**
 * Runs one call. A rejected operation throws the core's message, so pi marks
 * the result failed, and nothing is stored.
 * @param {TodoParams} params The validated tool arguments.
 * @param {TodoSession} session The branch to read and the entry writer.
 * @returns {TodoResult} The result for the model and the renderer.
 */
const runTodo = (params: TodoParams, session: TodoSession): TodoResult => {
  const current = branchSnapshot(session.branch()),
    { list, persist } = apply(current, params, {
      mutationId: crypto.randomUUID(),
      sessionId: session.sessionId,
    });
  if (persist) {
    saveState(current, list, session);
  }
  const details = persist ? branchSnapshot(session.branch()) : list;
  return {
    content: [
      {
        text: params.op === "view" ? view(details) : mutationSummary(current, details),
        type: "text",
      },
    ],
    details,
  };
};

export { runTodo };
export type { TodoParams, TodoResult, TodoSession };
