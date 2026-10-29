import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { WorkTodo } from "./work-todo.ts";
import { WorkPanel } from "./work-panel.ts";
import { TodoWork } from "./todo/src/work.ts";
import { TodoPresentation } from "./todo/src/presentation.ts";

test("real pi-todo and Dogsled exchange snapshots in either load order", () => {
	for (const providerFirst of [true, false]) {
		const emitter = new EventEmitter();
		const bus = { emit: (name: string, value: unknown) => { emitter.emit(name, value); }, on: (name: string, fn: (value: unknown) => void) => { emitter.on(name, fn); return () => { emitter.off(name, fn); }; } };
		let leaf = "a";
		let tasks: { content: string; status: string; subtasks?: { content: string; status: string }[] }[] = [{ content: "one", status: "in_progress" }];
		const widgets: unknown[][] = [];
		const ctx = { mode: "tui", sessionManager: { getSessionId: () => "session", getLeafId: () => leaf,
			getBranch: () => [{ type: "custom", customType: "todo-state", data: { tasks } }] },
			ui: { theme: { fg: (_: string, text: string) => text, bold: (text: string) => text }, notify() {}, setWidget: (...args: unknown[]) => widgets.push(args) } } as unknown as ExtensionContext;
		const host = { events: bus, getAllTools: () => [{ name: "todo", sourceInfo: { path: fileURLToPath(new URL("./index.ts", import.meta.url)) } }] };
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
		leaf = "nested";
		tasks = [{ content: "Outcome", status: "in_progress", subtasks: [
			{ content: "First deliverable", status: "in_progress" },
			{ content: "Second deliverable", status: "in_progress" },
		] }];
		provider.publish(ctx);
		panel.update(data());
		assert.match(panel.render(80).join("\n"), /  ● First deliverable/);
		assert.match(panel.render(80).join("\n"), /  ● Second deliverable/);
		assert.match(consumer.current?.display.summary ?? "", /0\/2 subtasks/);
		tasks[0]!.subtasks!.forEach(child => { child.status = "completed"; });
		provider.publish(ctx);
		panel.update(data());
		assert.match(panel.render(80).join("\n"), /Ready for review/);
		assert.match(consumer.current?.display.summary ?? "", /0\/1 completed outcomes; 2\/2 subtasks/);
		panel.dispose();
		panel.dispose();
		consumer.stop();
		assert.equal(provider.open(), false);
		assert.equal(typeof widgets.at(-1)?.[1], "function");
		provider.stop();
	}
});
