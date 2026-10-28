import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { TodoPublisher, snapshot as parseSnapshot, type TodoSnapshot } from "./work-todo.ts";
import { todoDisplay, todoSections } from "./todo-fixture.test.ts";
import { herdrAvailable, parseBoard, renderBoard } from "./board.ts";

function snapshot(): TodoSnapshot {
	return { version: 3, presenter: "presenter", publisher: "publisher", request: "request", session: "session", branch: "branch", revision: 1, display: todoDisplay() };
}

test("todo publisher writes the typed sections, orders revisions, and clears sessions", async () => {
	const dir = await mkdtemp(join(tmpdir(), "dogsled-todo-"));
	const path = join(dir, "pane.json");
	const errors: unknown[] = [];
	const publisher = new TodoPublisher("w1:p1", error => errors.push(error));
	try {
		publisher.configure({ path });
		const sections = todoSections({
			in_progress: [{ status: "in_progress", text: "Land PR #112", description: "Release it.", review: false, unit: null, children: [] }],
			pending: [{ status: "pending", text: "Verify", review: true, unit: "u1", children: [{ status: "pending", text: "child", unit: null }] }],
		});
		const next = snapshot();
		next.display = todoDisplay({ sections, state: new Proxy({}, { get() { throw new Error("Do not inspect task state"); } }) });
		publisher.publish(next);
		publisher.publish(next);
		await publisher.flushed();
		const payload = JSON.parse(await readFile(path, "utf8"));
		assert.deepEqual(Object.keys(payload), ["session", "branch", "revision", "current", "pane", "sections"]);
		assert.equal(payload.pane, "w1:p1");
		assert.equal(payload.current, "Verify changes");
		assert.deepEqual(payload.sections, sections);
		assert.deepEqual(payload.sections.map((section: { status: string }) => section.status), ["in_progress", "pending", "completed", "abandoned"]);
		publisher.publish({ ...next, revision: 2, branch: "other" });
		await publisher.flushed();
		assert.equal(JSON.parse(await readFile(path, "utf8")).revision, 2);
		await publisher.clear();
		await assert.rejects(readFile(path), { code: "ENOENT" });
		assert.deepEqual(errors, []);
	} finally { await rm(dir, { recursive: true, force: true }); }
});

test("publisher rejects a path without a string and the integration boundary rejects other versions and malformed sections", () => {
	const publisher = new TodoPublisher("w1:p1", () => {});
	assert.throws(() => publisher.configure({ path: 1 }));
	assert.throws(() => publisher.configure(null));
	assert.ok(parseSnapshot(snapshot()));
	assert.equal(parseSnapshot({ ...snapshot(), version: 2 }), undefined);
	const display = todoDisplay();
	assert.equal(parseSnapshot({ ...snapshot(), display: { ...display, sections: display.sections.slice(1) } }), undefined);
	assert.equal(parseSnapshot({ ...snapshot(), display: { ...display, sections: [...display.sections].reverse() } }), undefined);
	assert.equal(parseSnapshot({ ...snapshot(), display: { ...display, sections: undefined } }), undefined);
});

test("the board decoder keeps lifecycle strings and unknown groups visible", () => {
	const board = parseBoard({ hosts: [], rows: [
		{ group: "running", kind: "agent", id: "a", name: "finishing-child", what: "pi", host: "local", state: "finishing", activity: "", age_ms: 0 },
		{ group: "new-group", kind: "agent", id: "b", name: "unknown-child", what: "pi", host: "local", state: "new-state", activity: "", age_ms: 0 },
	] });
	const plain = { fg: (_: string, text: string) => text, bold: (text: string) => text };
	const lines = renderBoard(board, 120, plain);
	assert.deepEqual(lines.filter(line => /^ [A-Z]/.test(line)), [" NEEDS YOU", " WORKING"]);
	assert.ok(lines.some(line => line.includes("unknown-child") && line.includes("new-state")));
	assert.ok(lines.some(line => line.includes("finishing-child") && line.includes("finishing")));
});

test("Herdr detection requires the managed environment and a reachable socket", async () => {
	assert.equal(await herdrAvailable({}), false);
	assert.equal(await herdrAvailable({ HERDR_ENV: "0", HERDR_SOCKET_PATH: "/bad" }), false);
	assert.equal(await herdrAvailable({ HERDR_ENV: "1", HERDR_SOCKET_PATH: "/nonexistent-dogsled.sock" }), false);
});

