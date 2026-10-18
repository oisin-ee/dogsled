import type { TodoDisplay } from "./work-todo.ts";

/** Presentation-only fixture: consumers must not inspect todo task records. */
export function todoDisplay(extra: Partial<TodoDisplay> = {}): TodoDisplay {
	return {
		empty: false, summary: "Todo list: 1/3 completed", current: "Verify changes", text: "Provider-owned todo text", state: {},
		createView: () => ({ render: () => ["Provider-owned rows"], update() {}, invalidate() {}, handleInput() {}, dispose() {} }),
		...extra,
	};
}
