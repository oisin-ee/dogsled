import type { SectionChild, SectionItem, Subtask, TodoList, TodoSection } from "./wasm.ts";
import { sections, view } from "./wasm.ts";
import type { Theme } from "@earendil-works/pi-coding-agent";
import { TodoView } from "./embedded.ts";

/** Host-owned layout and rendering services for an inline todo view. */
interface ViewHost {
  readonly theme: Readonly<Pick<Theme, "fg" | "bold">>;
  readonly height: () => number;
  readonly requestRender: () => void;
}

/** A provider-owned view. The host forwards updates without interpreting state. */
interface HostedView {
  readonly render: (width: number) => string[];
  readonly handleInput: (input: string) => void;
  readonly invalidate: () => void;
  readonly update: (state: unknown) => void;
  readonly dispose: () => void;
}

/** Read-only core projections exposed to a presenter. */
interface DisplaySection extends Readonly<Omit<TodoSection, "items">> {
  readonly items: readonly (Readonly<Omit<SectionItem, "children">> & {
    readonly children: readonly Readonly<SectionChild>[];
  })[];
}

/** Transient presentation data, never persisted in a session entry. */
interface TodoDisplay {
  readonly empty: boolean;
  readonly summary: string;
  readonly current: string;
  readonly text: string;
  /** Status groups owned by the core: in_progress, pending, completed, abandoned. */
  readonly sections: readonly DisplaySection[];
  readonly state: unknown;
  readonly createView: (host: ViewHost) => HostedView;
}

const emptyCount = 0,
  headerIndex = 0,
  /**
   * Projects the core's summary and supplies its reusable renderer.
   * @param {TodoList} list The validated current branch snapshot.
   * @returns {TodoDisplay} An in-process presentation capability.
   */
  display = (list: TodoList): TodoDisplay => {
    const text = view(list),
      active = list.tasks.flatMap((task) => [
        ...(task.status === "in_progress" ? [task.content] : []),
        ...(task.subtasks ?? [])
          .filter(
            (child: Readonly<Pick<Subtask, "content" | "status">>) =>
              child.status === "in_progress",
          )
          .map(
            (child: Readonly<Pick<Subtask, "content" | "status">>) =>
              `${task.content} → ${child.content}`,
          ),
      ]),
      current =
        active.length > emptyCount
          ? active.join("; ")
          : (list.tasks.find((task) => task.status === "pending")?.content ?? "");
    return {
      createView: (host) => new TodoView(list, host),
      current,
      empty: list.tasks.length === emptyCount,
      sections: sections(list),
      state: list,
      summary: text.split("\n").at(headerIndex) ?? "",
      text,
    };
  };

export { display };
export type { HostedView, TodoDisplay, ViewHost };
