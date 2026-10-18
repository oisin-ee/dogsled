import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { pathToFileURL } from "node:url";
import { test } from "node:test";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { WorkTodo } from "./work-todo.ts";
import { WorkPanel } from "./work-panel.ts";

const source = process.env["PI_TODO_SOURCE"];
/** Release acceptance supplies the first-party provider source, never an undeclared package import. */
test("real pi-todo and Wrangle exchange snapshots in either load order", { skip: !source }, async () => {
	const { TodoWork } = await import(pathToFileURL(`${source}/work.ts`).href);
	const { TodoPresentation } = await import(pathToFileURL(`${source}/presentation.ts`).href);
	for (const providerFirst of [true, false]) {
		const emitter = new EventEmitter();
		const bus = { emit: (name: string, value: unknown) => { emitter.emit(name, value); }, on: (name: string, fn: (value: unknown) => void) => { emitter.on(name, fn); return () => { emitter.off(name, fn); }; } };
		let leaf = "a", tasks = [{ content: "one", status: "in_progress" }];
		const widgets: unknown[][] = [];
		const ctx = { mode: "tui", sessionManager: { getSessionId: () => "session", getLeafId: () => leaf,
			getBranch: () => [{ type: "custom", customType: "todo-state", data: { tasks } }] },
			ui: { theme: { fg: (_: string, text: string) => text, bold: (text: string) => text }, notify() {}, setWidget: (...args: unknown[]) => widgets.push(args) } } as unknown as ExtensionContext;
		const host = { events: bus, getAllTools: () => [{ name: "todo", sourceInfo: { path: `${source}/index.ts` } }] };
		const provider = new TodoWork(host, new TodoPresentation());
		let opens = 0;
		const consumer = new WorkTodo(bus, () => {}, () => { opens++; });
		if (providerFirst) { provider.start(ctx); consumer.start(ctx); }
		else { consumer.start(ctx); provider.start(ctx); }
		assert.equal(consumer.current?.display.current, "one");
		const hosted = consumer.createView({ theme: ctx.ui.theme, height: () => 10, requestRender() {} });
		assert.match(hosted?.render(80).join("\n") ?? "", /one/);
		const data = () => ({ board: { hosts: [], rows: [] }, todo: consumer.current, error: undefined, history: { jobs: [] } });
		const panel = new WorkPanel(data(), { tab: "all", selected: "todos", history: false }, ctx.ui.theme, () => {}, () => {}, () => 30, consumer);
		panel.handleInput("\r");
		assert.equal(panel.cursor.tab, "todos");
		assert.match(panel.render(80).join("\n"), /one/);
		assert.deepEqual(widgets.at(-1), ["pi-todo", undefined]);
		assert.equal(provider.open(), true); assert.equal(opens, 1);
		leaf = "b"; tasks = [{ content: "two", status: "completed" }];
		provider.publish(ctx);
		assert.equal(consumer.current?.branch, "b");
		assert.match(consumer.current?.display.text ?? "", /two/);
		assert.match(hosted?.render(80).join("\n") ?? "", /two/);
		hosted?.dispose();
		panel.update(data());
		assert.match(panel.render(80).join("\n"), /two/);
		panel.dispose();
		panel.dispose();
		consumer.stop();
		assert.equal(provider.open(), false);
		assert.equal(typeof widgets.at(-1)?.[1], "function");
		provider.stop();
	}
});
