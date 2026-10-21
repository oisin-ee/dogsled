import type { TodoDisplay, TodoSection } from "./work-todo.ts";

/** The four sections the contract guarantees, in order. */
export function todoSections(extra: Partial<Record<TodoSection["status"], TodoSection["items"]>> = {}): TodoSection[] {
	return (["in_progress", "pending", "completed", "abandoned"] as const).map(status => ({ status, count: (extra[status] ?? []).length, items: extra[status] ?? [] }));
}

/** Presentation-only fixture: consumers must not inspect todo task records. */
export function todoDisplay(extra: Partial<TodoDisplay> = {}): TodoDisplay {
	return {
		sections: todoSections(),
		empty: false, summary: "Todo list: 1/3 completed", current: "Verify changes", text: "Provider-owned todo text", state: {},
		createView: () => ({ render: () => ["Provider-owned rows"], update() {}, invalidate() {}, handleInput() {}, dispose() {} }),
		...extra,
	};
}
