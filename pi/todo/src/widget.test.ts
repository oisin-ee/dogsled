import type { TodoList, WidgetRow } from "./wasm.ts";
import { describe, expect, it, vi } from "vitest";
import { init, parse, widget } from "./wasm.ts";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { TodoWidget } from "./widget.ts";

const normalWidth = 80,
  narrowWidth = 40,
  minimumWidth = 1,
  wideGlyphWidth = 2,
  compactWidth = 12,
  wideWidth = 100,
  emptyLength = 0,
  theme = {
    bold: (text: string): string => `\u001B[1m${text}\u001B[22m`,
    fg: (_color: string, text: string): string => text,
  },
  /**
   * Requires a visible widget for a nonempty test list.
   * @param {TodoList} list Test tasks.
   * @param {boolean} expanded Whether to reveal every group.
   * @returns {WidgetRow[]} Rust presentation rows.
   */
  rows = (list: TodoList, expanded: boolean): WidgetRow[] => {
    const result = widget(list, expanded);
    if (result === undefined) {
      throw new Error("Expected widget rows for a nonempty list");
    }
    return result;
  },
  /**
   * Creates explicitly active work for rendering tests.
   * @param {string} task First task label.
   * @param {readonly string[]} pending Other pending tasks.
   * @returns {TodoList} Active snapshot.
   */
  started = (task: string, pending: readonly string[]): TodoList =>
    parse({
      tasks: [
        { content: task, status: "in_progress" },
        ...pending.map((content) => ({ content, status: "pending" })),
      ],
    });

describe("todo rendering", () => {
  it("renders active work and reveals grouped details only on expansion", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const list = started("Build the widget", ["Check the layout"]),
      collapsed = new TodoWidget(rows(list, false), false, theme),
      expanded = new TodoWidget(rows(list, true), true, theme),
      details = expanded
        .render(normalWidth)
        .map((line) => stripTerminalSequences(line))
        .join("\n");
    expect(
      collapsed.render(normalWidth).map((line) => stripTerminalSequences(line).trimEnd()),
    ).toStrictEqual([" In progress · 1", " ● Build the widget", " Alt+T · upcoming / done"]);
    expect(details).toContain("Upcoming · 1");
    expect(details).toContain("○ Check the layout");
    expect(details).toContain("Alt+T · collapse");
  });

  it.each([minimumWidth, wideGlyphWidth, compactWidth, narrowWidth, wideWidth])(
    "fits wide characters and long labels into %i columns",
    { timeout: 5000 },
    (width) => {
      expect.hasAssertions();
      const content = "Review 日本語 tasks 👩‍💻 with a long label and café combining marks",
        component = new TodoWidget(rows(init([content]), false), false, theme),
        lines = component.render(width);
      expect(lines.length).toBeGreaterThan(emptyLength);
      expect(lines.every((line) => visibleWidth(line) <= width)).toBe(true);
    },
  );

  it("reapplies theme tokens after invalidation", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const fg = vi.fn<typeof theme.fg>(theme.fg),
      component = new TodoWidget(rows(started("Active", []), false), false, {
        bold: theme.bold,
        fg,
      });
    component.render(normalWidth);
    expect(fg).toHaveBeenCalledWith("accent", "● Active");
    fg.mockClear();
    component.invalidate();
    component.render(narrowWidth);
    expect(fg).toHaveBeenCalledWith("accent", "● Active");
    expect(fg).toHaveBeenCalledWith("dim", "Alt+T · upcoming / done");
  });
});
