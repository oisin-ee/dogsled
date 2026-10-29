import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { WorkTodo, type TodoSnapshot } from "./work-todo.ts";
import { workBoard } from "./work.ts";
import { visibleWidth } from "@earendil-works/pi-tui";
import { todoDisplay } from "./todo-fixture.test.ts";
import type { TodoPresenter } from "./todo/src/work.ts";

const plain = { fg: (_: string, text: string) => text, bold: (text: string) => text };
const task = { content: "Verify changes", status: "in_progress" as const };
function rig() {
	let presenter: TodoPresenter | undefined;
	let started = false;
	let leaf: string | null = "branch";
	const ctx = { mode: "tui", sessionManager: { getSessionId: () => "session", getLeafId: () => leaf }, ui: { notify() {}, theme: plain } } as unknown as ExtensionContext;
	const claims: TodoSnapshot[] = [], releases: boolean[] = [];
	let draws = 0, opens = 0;
	const data = (extra: Partial<TodoSnapshot> = {}): TodoSnapshot => ({ session: "session", branch: leaf, revision: 1, display: todoDisplay(), ...extra });
	const send = (next = data()) => {
		if (!presenter || !consumer.show(next)) return false;
		assert.ok(draws > 0, "draw completes before claiming");
		claims.push(next);
		return true;
	};
	const consumer = new WorkTodo({ attach(next) {
		presenter = next;
		if (started) send();
		return () => { presenter = undefined; releases.push(true); };
	} }, () => { draws++; }, () => { opens++; });
	return { consumer, ctx, claims, releases, data, send,
		start: () => { started = true; send(); }, open: () => presenter?.open(), stop: () => presenter?.stop(),
		branch: (next: string) => { leaf = next; }, draws: () => draws, opens: () => opens };
}

test("consumer attaches an already started provider and acknowledges only after rendering", () => {
	const r = rig();
	r.start();
	r.consumer.start(r.ctx);
	assert.equal(r.consumer.current?.display.current, task.content);
	assert.equal(r.claims.length, 1);
	r.consumer.stop(); assert.equal(r.releases.length, 1);
});

test("provider starting later publishes and can open the todo view", () => {
	const r = rig(); r.consumer.start(r.ctx);
	r.start();
	r.open();
	assert.equal(r.opens(), 1);
	r.consumer.stop();
	r.open();
	assert.equal(r.opens(), 1);
});

test("invalid revisions release presentation", () => {
	for (const revision of [Number.NaN, -1, 0, 1.5]) {
		const r = rig(); r.consumer.start(r.ctx);
		assert.equal(r.send(r.data({ revision })), false);
		assert.equal(r.consumer.current, undefined);
		assert.match(r.consumer.error ?? "", /unavailable/);
		assert.equal(r.releases.length, 1);
		r.consumer.stop();
	}
});

test("delayed contexts and duplicate revisions cannot replace current data", () => {
	const r = rig(); r.consumer.start(r.ctx);
	r.send(r.data());
	r.send(r.data({ display: todoDisplay({ empty: true }) }));
	assert.equal(r.consumer.current?.display.empty, false);
	r.branch("other");
	r.send(r.data({ branch: "branch", revision: 2 }));
	assert.equal(r.consumer.current?.revision, 1);
	r.send(r.data({ revision: 3, display: todoDisplay({ empty: true }) }));
	assert.equal(r.consumer.current?.branch, "other");
	assert.equal(r.send(r.data({ session: "another", revision: 99 })), false);
	assert.equal(r.consumer.current?.revision, 3);
	r.stop();
	assert.equal(r.consumer.current, undefined);
	r.consumer.stop();
});

test("render failure releases presentation; shutdown detaches and non-TUI does not claim", () => {
	const r = rig(); r.consumer.start(r.ctx);
	r.send();
	r.send(r.data({ revision: 2, display: todoDisplay({ createView() { throw new Error("render failed"); } }) }));
	assert.equal(r.consumer.current, undefined);
	assert.match(r.consumer.error ?? "", /unavailable/);
	assert.equal(r.releases.length, 1);
	r.consumer.stop();
	const before = r.draws();
	r.send(r.data());
	assert.equal(r.draws(), before);
	r.consumer.start({ ...r.ctx, mode: "rpc" });
	assert.equal(r.claims.length, 1);
});

test("the board widget shows the current task above the board groups at bounded widths", () => {
	const r = rig(); r.consumer.start(r.ctx);
	const todo = r.data();
	const board = { hosts: [], rows: [{ id: "a", group: "attention" as const, kind: "agent", name: "界".repeat(100), what: "pi", host: "local", state: "blocked", activity: "", age_ms: 0 }] };
	for (const width of [1, 8, 20, 40, 80, 160]) {
		const lines = workBoard({ board, todo, error: undefined }, width, plain);
		assert.equal(lines.length, 5);
		assert.ok(lines.every(line => visibleWidth(line) <= width));
		if (width >= 20) assert.match(lines[4]!, /!/);
	}
	const wide = workBoard({ board, todo, error: undefined }, 160, plain);
	assert.match(wide[0]!, /^ dogsled {2}1 needs you/);
	assert.deepEqual(wide.slice(1, 4).map(line => line.trim().split(/ {2,}/)), [["TASK"], ["◆ Verify changes", "Todo list: 1/3 completed"], ["NEEDS YOU"]]);
	const closed = { ...todo, display: todoDisplay({ current: "", summary: "Todo list: 1/2 completed" }) };
	assert.match(workBoard({ board, todo: closed, error: undefined }, 160, plain).join("\n"), /1\/2 completed/);
	assert.deepEqual(workBoard({ board: { hosts: [], rows: [] }, todo: undefined, error: undefined }, 80, plain), []);
	assert.match(workBoard({ board: { hosts: [], rows: [] }, todo: undefined, error: "host offline" }, 80, plain).join("\n"), /unavailable/);
	r.consumer.stop();
});

test("hosted render failure restores standalone ownership after a successful claim", () => {
	const r = rig(); r.consumer.start(r.ctx);
	let created = 0, disposed = 0;
	const display = todoDisplay({ createView: () => {
		const preflight = created++ === 0;
		return { render: () => { if (!preflight) throw new Error("live render"); return []; },
			update() {}, invalidate() {}, handleInput() {}, dispose() { disposed++; } };
	} });
	r.send(r.data({ display }));
	const component = r.consumer.createView({ theme: plain, height: () => 10, requestRender() {} });
	assert.equal(r.claims.length, 1);
	assert.deepEqual(component?.render(80), []);
	assert.equal(r.consumer.current, undefined);
	assert.equal(r.releases.length, 1);
	assert.equal(disposed, 2);
	r.consumer.stop();
});

test("provider failures release ownership and dispose hosted components once", () => {
	for (const failure of ["create", "render", "update", "invalidate", "input"] as const) {
		const r = rig(); r.consumer.start(r.ctx);
		let disposed = 0;
		const display = todoDisplay({ createView: () => {
			if (failure === "create") throw new Error("create failed");
			return { render: () => { if (failure === "render") throw new Error("render failed"); return []; },
				update() {}, invalidate() {}, handleInput() {}, dispose() { disposed++; } };
		} });
		r.send(r.data({ display }));
		if (failure === "create" || failure === "render") {
			assert.equal(r.claims.length, 0);
			assert.equal(r.consumer.current, undefined);
		} else {
			const next = todoDisplay({ createView: () => ({ render: () => [],
				update() { if (failure === "update") throw new Error("update failed"); },
				invalidate() { if (failure === "invalidate") throw new Error("invalidate failed"); },
				handleInput() { if (failure === "input") throw new Error("input failed"); }, dispose() { disposed++; } }) });
			// The live factory can fail after the successful preflight.
			r.consumer.current!.display = next;
			const component = r.consumer.createView({ theme: plain, height: () => 10, requestRender() {} })!;
			if (failure === "update") component.update({});
			if (failure === "invalidate") component.invalidate();
			if (failure === "input") component.handleInput("x");
			assert.equal(r.consumer.current, undefined);
			assert.equal(r.releases.length, 1);
			const count = disposed;
			component.dispose();
			assert.equal(disposed, count);
		}
		assert.match(r.consumer.error ?? "", /unavailable/);
		r.consumer.stop();
	}
});

test("the board widget keeps agents and jobs in action groups with the task first", () => {
	const r = rig(); r.consumer.start(r.ctx);
	const row = { id: "a", group: "running" as const, kind: "agent", name: "builder", what: "pi", host: "local", state: "working", activity: "", age_ms: 120000 };
	const state = { board: { hosts: [], rows: [row, { ...row, id: "b", name: "reviewer", group: "attention" as const, state: "blocked" }, { ...row, id: "c", kind: "job", name: "tests", state: "running" }] }, todo: r.data(), error: undefined };
	const lines = workBoard(state, 48, plain);
	assert.deepEqual(lines.map(line => line.trim().split(/ {2,}/)[0]), ["dogsled", "TASK", "◆ Verify changes", "NEEDS YOU", "! reviewer", "WORKING", "◐ builder", "▸ tests"]);
	assert.ok(lines.every(line => visibleWidth(line) <= 48));
	r.consumer.stop();
});
