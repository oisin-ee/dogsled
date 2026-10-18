import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionContext, KeybindingsManager, Theme } from "@earendil-works/pi-coding-agent";
import { visibleWidth, matchesKey, Key } from "@earendil-works/pi-tui";
import { loadJobHistory } from "./work-history.ts";
import { WorkPanel, workEntries, inspectEntry, runWorkAction, showWorkDetail, type WorkAction, type WorkData, type WorkEntry } from "./work-panel.ts";

const plain = { fg: (_: string, text: string) => text, bold: (text: string) => text } as Theme;
const keys = { getKeys: () => ["esc"], matches: (input: string) => matchesKey(input, Key.escape) } as unknown as KeybindingsManager;
function data(): WorkData {
	return { board: { hosts: [], rows: [
		{ id: "agent", host: "netcup", pane: "w2:p1", name: "reviewer", kind: "agent", what: "pi", group: "attention", state: "blocked", age_ms: 0, activity: "needs input" },
		{ id: "job", host: "local", name: "tests", kind: "job", what: "job", group: "running", state: "running", age_ms: 0, activity: "tests" },
		{ id: "ticket", host: "any", name: "queued", kind: "job", what: "job", group: "waiting", state: "queued", age_ms: 0, activity: "waiting" },
	] }, todo: { version: 1, presenter: "p", request: "r", publisher: "t", revision: 1, session: "s", branch: null, tasks: [
		{ content: "Verify changes", status: "in_progress" }, { content: "Done", status: "completed" }, { content: "Dropped", status: "abandoned" },
	] }, error: undefined, history: { jobs: [{ id: "done", host: "local", owner: "w1:p1", cmd: ["test"], state: "exited", ended_ms: 1, exit: 0 }] } };
}

test("history joins both recorded host and job id and rejects unrelated owners", async () => {
	const calls: string[][] = [];
	const result = await loadJobHistory(async args => {
		calls.push(args);
		return JSON.stringify(args[0] === "items" ? { items: [{ id: "same", host: "netcup", owner_pane: "w1:p1", kind: "job" }, { id: "other", host: "local", owner_pane: "w2:p1", kind: "job" }] }
			: args[0] === "machine" ? [{ id: "netcup", label: "remote" }] : ["local", "netcup"].map(host => ({ id: "same", host, owner: "w1:p1", cmd: ["test"], state: "done", ended_ms: 1, exit: 0 })));
	}, "w1:p1");
	assert.deepEqual(result.jobs.map(job => job.host), ["netcup"]);
	assert.deepEqual(calls[0], ["items", "--owner", "w1:p1", "--json"]);
	assert.match((await loadJobHistory(async () => "{}", "owner")).error ?? "", /unavailable/);
});

test("tabs preserve todo status and collapse completed job history without duplicate active rows", () => {
	const d = data();
	assert.equal(workEntries(d, "all", false)[0]?.id, "agent");
	assert.deepEqual(workEntries(d, "todos", false).map(row => row.description), ["in_progress", "completed", "abandoned"]);
	assert.equal(workEntries(d, "agents", false).length, 1);
	assert.equal(workEntries(d, "jobs", false).some(row => row.id === "done"), false);
	assert.equal(workEntries(d, "jobs", true).filter(row => row.id === "done").length, 1);
	assert.ok(workEntries(d, "jobs", false).some(row => row.kind === "queued"));
});

test("history deduplicates by canonical host identity, not display label", () => {
	const d = data();
	d.history.hosts = [{ id: "remote-id", label: "netcup" }];
	d.board.rows.push({ ...d.board.rows[1]!, host: "netcup", id: "done", group: "attention" });
	d.history.jobs[0]!.host = "remote-id";
	const rows = workEntries(d, "jobs", true);
	assert.equal(rows.filter(row => row.id === "done").length, 1);
	assert.equal(rows.find(row => row.id === "done")?.host, "remote-id");
});

test("aborted interactions cannot stop, cancel, or send after a dialog resolves", async () => {
	for (const action of ["stop", "cancel", "send"] as const) {
		const controller = new AbortController();
		const ctx = { ui: {
			confirm: async () => { controller.abort(); return true; },
			input: async () => { controller.abort(); return "late message"; },
		} } as unknown as ExtensionContext;
		const entry: WorkEntry = { value: "j", label: "job", id: "j", running: true, kind: action === "stop" ? "job" : action === "cancel" ? "queued" : "agent" };
		await runWorkAction({ action, entry, cursor: { tab: "all", selected: undefined, history: false } }, ctx, async () => { assert.fail("late mutation"); }, controller.signal);
	}
});

test("aborting an interaction closes its detail overlay", async () => {
	const controller = new AbortController();
	let closed = false;
	const ctx = { ui: { custom: async (factory: any) => {
		const component = factory({ terminal: { rows: 24 }, requestRender() {} }, plain, keys, () => { closed = true; });
		controller.abort();
		component.dispose();
	} } } as unknown as ExtensionContext;
	await showWorkDetail(ctx, "Detail", "text", controller.signal);
	assert.equal(closed, true);
});

test("refresh keeps stable selection and terminal geometry stays bounded", () => {
	const d = data(); let selected: WorkAction | undefined;
	const panel = new WorkPanel(d, { tab: "jobs", selected: "job:local:job", history: false }, plain, keys, action => { selected = action; }, () => {}, () => 24);
	panel.update({ ...d, board: { ...d.board, rows: [...d.board.rows].reverse() } });
	panel.handleInput("\r");
	assert.equal(selected?.entry.id, "job");
	for (const width of [20, 60, 120]) {
		const lines = panel.render(width);
		assert.ok(lines.length <= 20);
		assert.ok(lines.every(line => visibleWidth(line) <= width));
	}
	panel.handleInput("\t");
	assert.equal(panel.cursor.tab, "all");
});

test("history toggle stays inside the panel and todo rows never offer mutation actions", () => {
	const d = data(); let closed = false;
	const panel = new WorkPanel(d, { tab: "jobs", selected: "history", history: false }, plain, keys, () => { closed = true; }, () => {}, () => 30);
	panel.handleInput("\r"); assert.equal(panel.cursor.history, true); assert.equal(closed, false);
	const todo = new WorkPanel(d, { tab: "todos", selected: undefined, history: false }, plain, keys, () => { closed = true; }, () => {}, () => 30);
	for (const input of ["s", "c", "m"]) todo.handleInput(input);
	assert.equal(closed, false);
});

test("inspection uses existing commands and sanitizes output without losing line breaks", async () => {
	const calls: string[][] = [];
	const exec = async (args: string[]) => { calls.push(args); return "\x1b[31mline one\x1b[0m\nline two"; };
	assert.equal(await inspectEntry({ kind: "todo", value: "t", label: "t", detail: "Full\ntodo" }, exec), "Full\ntodo");
	assert.equal(calls.length, 0);
	assert.match(await inspectEntry({ kind: "job", value: "j", label: "j", id: "job" }, exec), /Full log: wrangle logs job\n\nline one\nline two/);
	await inspectEntry({ kind: "agent", value: "a", label: "a", id: "agent", pane: "w2:p1", host: "netcup" }, exec);
	assert.deepEqual(calls, [["read", "job", "--host", "local"], ["read", "w2:p1", "--host", "netcup"]]);
});

test("detail viewport scrolls and Escape returns through Pi's completion callback", async () => {
	let completed = false;
	const ctx = { ui: { custom: async (factory: any, options: any) => {
		assert.equal(options.overlay, true);
		const component = factory({ terminal: { rows: 15 }, requestRender() {} }, plain, keys, () => { completed = true; });
		const before = component.render(45);
		assert.ok(before.length <= 15);
		component.handleInput("\u001b[6~");
		const after = component.render(45);
		assert.notDeepEqual(before, after);
		assert.ok(after.every((line: string) => visibleWidth(line) <= 45));
		component.handleInput("\u001b");
	} } } as unknown as ExtensionContext;
	await showWorkDetail(ctx, "Logs", Array.from({ length: 60 }, (_, i) => `line ${i}`).join("\n"), new AbortController().signal);
	assert.equal(completed, true);
});

test("stop and cancel require confirmation; messaging uses inbox arguments, never terminal input", async () => {
	let accepted = false;
	const calls: string[][] = [];
	const ctx = { ui: { confirm: async () => accepted, input: async () => "message; not shell" } } as unknown as ExtensionContext;
	const exec = async (args: string[]) => { calls.push(args); return ""; };
	const action = (kind: WorkAction["action"], entry: WorkEntry): WorkAction => ({ action: kind, entry, cursor: { tab: "all", selected: undefined, history: false } });
	const job: WorkEntry = { value: "j", label: "tests", kind: "job", id: "job", running: true };
	const signal = new AbortController().signal;
	await runWorkAction(action("stop", job), ctx, exec, signal); assert.equal(calls.length, 0);
	accepted = true;
	await runWorkAction(action("stop", job), ctx, exec, signal);
	await runWorkAction(action("cancel", { ...job, kind: "queued", id: "ticket" }), ctx, exec, signal);
	await runWorkAction(action("send", { ...job, kind: "agent", pane: "w2:p1", host: "netcup" }), ctx, exec, signal);
	assert.deepEqual(calls, [["stop", "job"], ["cancel", "ticket"], ["send", "w2:p1", "--host", "netcup", "--message", "message; not shell"]]);
});
