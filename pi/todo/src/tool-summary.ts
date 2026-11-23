import type { SectionChild, SectionItem, TodoList, TodoSection } from "./wasm.ts";
import { sections, view } from "./wasm.ts";

/** Read-only views of the core's section projection. */
type Child = Readonly<SectionChild>;
type Item = Readonly<Omit<SectionItem, "children">> & { readonly children: readonly Child[] };
type Section = Readonly<Omit<TodoSection, "items">> & { readonly items: readonly Item[] };

const marks = {
    abandoned: "[-]",
    completed: "[x]",
    in_progress: "[/]",
    pending: "[ ]",
  },
  // The core guarantees unique, immutable labels within each set of siblings, even for legacy items.
  byLabel = <Row extends Child>(items: readonly Row[]): Map<string, Row> =>
    new Map(items.map((item) => [item.text, item])),
  changed = (previous: Child | undefined, item: Child): boolean =>
    previous?.status !== item.status ||
    previous.description !== item.description ||
    previous.unit !== item.unit,
  rows = (item: Child, indent: string, review = false): string[] => [
    `${indent}${marks[item.status]} ${item.text}${review ? " · Ready for review" : ""}`,
    ...(item.description === undefined ? [] : [`${indent}  ${item.description}`]),
    ...(item.unit === undefined ? [] : [`${indent}    ${item.unit}`]),
  ],
  changedRows = (previous: Item | undefined, item: Item): string[] => {
    const children = byLabel(previous?.children ?? []),
      childRows = item.children.flatMap((child) =>
        changed(children.get(child.text), child) ? rows(child, "  ") : [],
      );
    if (changed(previous, item) || previous?.review !== item.review) {
      return [...rows(item, "", item.review), ...childRows];
    }
    return childRows;
  },
  items = (list: TodoList): Map<string, Item> =>
    byLabel(sections(list).flatMap((section: Section) => section.items)),
  /**
   * Shows the full counts and only rows that changed, in list order.
   * @param {TodoList} before The branch snapshot before the call.
   * @param {TodoList} after The branch snapshot after the call.
   * @returns {string} The count header and the changed rows.
   */
  mutationSummary = (before: TodoList, after: TodoList): string => {
    const [header] = view(after).split("\n"),
      previous = items(before),
      current = items(after),
      changes = after.tasks.flatMap((task) => {
        const item = current.get(task.content);
        return item === undefined ? [] : changedRows(previous.get(task.content), item);
      });
    return [header, ...changes].join("\n");
  };

export { mutationSummary };
