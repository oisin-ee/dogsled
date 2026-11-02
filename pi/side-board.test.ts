import assert from "node:assert/strict";
import { test } from "node:test";
import { TodoPublisher, type TodoPayload, type TodoSnapshot } from "./work-todo.ts";
import { todoDisplay, todoSections } from "./todo-fixture.test.ts";
import { herdrAvailable, parseBoard, renderBoard } from "./board.ts";

function snapshot(): TodoSnapshot {
	return { session: "session", branch: "branch", revision: 1, display: todoDisplay() };
}

test("todo publisher sends typed sections, orders revisions, and clears sessions", async () => {
	const calls: { pane: string; payload: TodoPayload | undefined }[] = [];
	const errors: unknown[] = [];
	const publisher = new TodoPublisher("w1:p1", error => errors.push(error), async (pane, payload) => { calls.push({ pane, payload }); });
	publisher.configure();
	const sections = todoSections({
		in_progress: [{ status: "in_progress", text: "Land PR #112", description: "Release it.", review: false, unit: null, children: [] }],
		pending: [{ status: "pending", text: "Verify", review: true, unit: "u1", children: [{ status: "pending", text: "child", unit: null }] }],
	});
	const next = snapshot();
	next.display = todoDisplay({ sections, state: new Proxy({}, { get() { throw new Error("Do not inspect task state"); } }) });
	publisher.publish(next);
	publisher.publish(next);
	await publisher.flushed();
	assert.equal(calls.length, 1);
	const payload = calls[0]?.payload;
	assert.ok(payload);
	assert.deepEqual(Object.keys(payload), ["session", "branch", "revision", "current", "pane", "sections"]);
	assert.equal(payload.pane, "w1:p1");
	assert.equal(payload.current, "Verify changes");
	assert.deepEqual(payload.sections, sections);
	assert.deepEqual(payload.sections.map(section => section.status), ["in_progress", "pending", "completed", "abandoned"]);
	publisher.publish({ ...next, revision: 2, branch: "other" });
	await publisher.flushed();
	assert.equal(calls[1]?.payload?.revision, 2);
	await publisher.clear();
	assert.equal(calls[2]?.pane, "w1:p1");
	assert.equal(calls[2]?.payload, undefined);
	assert.deepEqual(errors, []);
});

test("publisher remains disabled until configured and reports failed publications", async () => {
	const errors: unknown[] = [];
	const publisher = new TodoPublisher("w1:p1", error => errors.push(error), async () => { throw new Error("storage refused"); });
	publisher.publish(snapshot());
	await publisher.flushed();
	assert.deepEqual(errors, []);
	publisher.configure();
	publisher.publish(snapshot());
	await publisher.flushed();
	assert.equal(errors.length, 1);
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

