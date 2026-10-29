import { describe, expect, it, vi } from "vitest";
import { init, parse } from "./wasm.ts";
import { TodoView } from "./embedded.ts";
import { display } from "./display.ts";
import { stripTerminalSequences } from "@earendil-works/pi-tui";

const width = 60,
  height = 20,
  theme = {
    bold: (text: string): string => text,
    fg: (_color: string, text: string): string => text,
  },
  host = { height: (): number => height, requestRender: vi.fn<() => void>(), theme };

describe("hosted todo rendering", () => {
  it(
    "renders the shared rows, updates, and disposes without task writes",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const list = init(["Build the interface", "Verify integration"]),
        component = new TodoView(list, host);
      expect(component.render(width).join("\n")).toContain("Verify integration");
      component.handleInput("\r");
      expect(component.render(width).join("\n")).not.toContain("Verify integration");
      component.update(
        parse({ tasks: [{ content: "A different branch", status: "in_progress" }] }),
      );
      expect(component.render(width).join("\n")).toContain("A different branch");
      component.dispose();
      expect(component.render(width)).toStrictEqual([]);
    },
  );

  it(
    "provides summaries and a real renderer without exposing them as task mutations",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const presentation = display(init(["Deliver the change"])),
        component = presentation.createView(host);
      expect(presentation.summary).toBe("Todo list: 0/1 completed");
      expect(presentation.current).toBe("Deliver the change");
      expect(
        component
          .render(width)
          .map((line) => stripTerminalSequences(line))
          .join("\n"),
      ).toContain("Deliver the change");
      component.dispose();
    },
  );
});

describe("hosted viewport lifecycle", () => {
  it("keeps a bounded viewport and rejects malformed updates", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const viewport = 2,
      component = new TodoView(
        init(["First", "Second", "Third"]),
        Object.assign({}, host, {
          height: (): number => viewport,
        }),
      ),
      before = component.render(width);
    component.handleInput("\u001B[B");
    expect(component.render(width)).not.toStrictEqual(before);
    expect(component.render(width).length).toBeLessThanOrEqual(viewport);
    expect(() => {
      component.update({ invalid: true });
    }).toThrow(/invalid|unknown|missing/u);
    component.dispose();
  });
});
