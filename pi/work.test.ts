import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { test } from "node:test";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { WorkTodo, snapshot, type TodoSnapshot } from "./work-todo.ts";
import { workStrip } from "./work.ts";
import { visibleWidth } from "@earendil-works/pi-tui";

const plain = { fg: (_: string, text: string) => text, bold: (text: string) => text };
const task = { content: "Verify changes", status: "in_progress" as const };
function rig() {
	const emitter = new EventEmitter();
	const bus = { emit(name: string, value: unknown) { emitter.emit(name, value); }, on(name: string, fn: (value: unknown) => void) { emitter.on(name, fn); return () => { emitter.off(name, fn); }; } };
	let leaf: string | null = "branch";
	const ctx = { mode: "tui", sessionManager: { getSessionId: () => "session", getLeafId: () => leaf }, ui: { notify() {} } } as unknown as ExtensionContext;
	const requests: any[] = [], claims: any[] = [], releases: any[] = [];
	let draws = 0, opens = 0;
	bus.on("oisin.work.request", value => requests.push(value));
	bus.on("oisin.work.claim", value => claims.push(value));
	bus.on("oisin.work.release", value => releases.push(value));
	const consumer = new WorkTodo(bus, () => { draws++; }, () => { opens++; });
	const data = (extra = {}): TodoSnapshot => ({ ...requests.at(-1), publisher: "provider", session: "session", branch: leaf, revision: 1, tasks: [task], ...extra });
	return { bus, consumer, ctx, requests, claims, releases, data, branch: (next: string) => { leaf = next; }, draws: () => draws, opens: () => opens };
}

test("todo data is validated at the integration boundary", () => {
	const r = rig(); r.consumer.start(r.ctx);
	assert.deepEqual(snapshot(r.data())?.tasks, [task]);
	for (const bad of [null, {}, r.data({ version: 2 }), r.data({ revision: -1 }), r.data({ tasks: [{ ...task, status: "done" }] })]) assert.equal(snapshot(bad), undefined);
	r.consumer.stop();
});

test("consumer requests an already started provider and acknowledges only after rendering", () => {
	const r = rig();
	r.bus.on("oisin.work.request", () => r.bus.emit("oisin.todo.snapshot", r.data()));
	r.bus.on("oisin.work.claim", () => assert.equal(r.draws(), 1));
	r.consumer.start(r.ctx);
	assert.equal(r.consumer.current?.tasks[0]?.content, task.content);
	assert.equal(r.claims.length, 1);
	r.consumer.stop(); assert.equal(r.releases.length, 1);
});

test("provider starting later requests a fresh handshake and can open the todo view", () => {
	const r = rig(); r.consumer.start(r.ctx);
	const first = r.requests.at(-1);
	r.bus.emit("oisin.todo.ready", { publisher: "provider", version: 1 });
	assert.notEqual(r.requests.at(-1).request, first.request);
	r.bus.emit("oisin.todo.snapshot", r.data());
	r.bus.emit("oisin.work.open", { ...r.requests.at(-1), view: "todos" });
	assert.equal(r.opens(), 1);
	r.bus.emit("oisin.work.open", { ...first, view: "todos" });
	assert.equal(r.opens(), 1);
	r.consumer.stop();
});

test("delayed contexts, duplicate revisions and retired publishers cannot replace current data", () => {
	const r = rig(); r.consumer.start(r.ctx);
	r.bus.emit("oisin.todo.snapshot", r.data());
	r.bus.emit("oisin.todo.snapshot", r.data({ tasks: [] }));
	assert.equal(r.consumer.current?.tasks.length, 1);
	r.branch("other");
	r.bus.emit("oisin.todo.snapshot", r.data({ branch: "branch", revision: 2 }));
	assert.equal(r.consumer.current?.revision, 1);
	r.bus.emit("oisin.todo.snapshot", r.data({ revision: 3, tasks: [] }));
	assert.equal(r.consumer.current?.branch, "other");
	r.bus.emit("oisin.todo.snapshot", r.data({ publisher: "new-provider" }));
	r.bus.emit("oisin.todo.snapshot", r.data({ revision: 99 }));
	assert.equal(r.consumer.current?.publisher, "new-provider");
	r.bus.emit("oisin.todo.stop", { version: 1, publisher: "new-provider" });
	assert.equal(r.consumer.current, undefined);
	r.consumer.stop();
});

test("malformed payload releases presentation; shutdown unsubscribes and non-TUI does not claim", () => {
	const r = rig(); r.consumer.start(r.ctx);
	r.bus.emit("oisin.todo.snapshot", r.data());
	r.bus.emit("oisin.todo.snapshot", r.data({ tasks: false }));
	assert.equal(r.consumer.current, undefined);
	assert.match(r.consumer.error ?? "", /unavailable/);
	assert.equal(r.releases.length, 1);
	r.consumer.stop();
	const before = r.draws();
	r.bus.emit("oisin.todo.snapshot", r.data());
	assert.equal(r.draws(), before);
	r.consumer.start({ ...r.ctx, mode: "rpc" });
	assert.equal(r.claims.length, 1);
});

test("strip stays one line, distinguishes completed from abandoned, and preserves attention", () => {
	const r = rig(); r.consumer.start(r.ctx);
	const todo = r.data({ tasks: [task, { content: "done", status: "completed" }, { content: "dropped", status: "abandoned" }] });
	const board = { hosts: [], rows: [{ id: "a", group: "attention" as const, kind: "agent", name: "界".repeat(100), what: "pi", host: "local", state: "blocked", activity: "", age_ms: 0 }] };
	for (const width of [1, 8, 20, 40, 80, 160]) {
		const lines = workStrip({ board, todo, error: undefined }, width, plain);
		assert.equal(lines.length, 1);
		assert.ok(visibleWidth(lines[0]!) <= width);
		if (width >= 8) assert.match(lines[0]!, /!1/);
	}
	assert.match(workStrip({ board, todo, error: undefined }, 160, plain)[0]!, /1\/3 todos/);
	assert.deepEqual(workStrip({ board: { hosts: [], rows: [] }, todo: undefined, error: undefined }, 80, plain), []);
	assert.match(workStrip({ board: { hosts: [], rows: [] }, todo: undefined, error: "host offline" }, 80, plain)[0]!, /unavailable/);
	r.consumer.stop();
});
