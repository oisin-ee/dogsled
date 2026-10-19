import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionContext, Theme } from "@earendil-works/pi-coding-agent";
import { visibleWidth } from "@earendil-works/pi-tui";
import { loadJobHistory } from "./work-history.ts";
import { todoDisplay } from "./todo-fixture.test.ts";
import { WorkPanel, workEntries, inspectEntry, runWorkAction, WorkDetail, type WorkAction, type WorkData, type WorkEntry } from "./work-panel.ts";

const plain = { fg: (_: string, text: string) => text, bold: (text: string) => text } as Theme;
function data(): WorkData {
	return { board: { hosts: [], rows: [
		{ id: "agent", host: "netcup", pane: "w2:p1", name: "reviewer", kind: "agent", what: "pi", group: "attention", state: "blocked", age_ms: 0, activity: "needs input" },
		{ id: "job", host: "local", name: "tests", kind: "job", what: "job", group: "running", state: "running", age_ms: 0, activity: "tests" },
		{ id: "ticket", host: "any", name: "queued", kind: "job", what: "job", group: "waiting", state: "queued", age_ms: 0, activity: "waiting" },
	] }, todo: { version: 2, presenter: "p", request: "r", publisher: "t", revision: 1, session: "s", branch: null, display: todoDisplay() }, error: undefined, history: { jobs: [{ id: "done", host: "local", owner: "w1:p1", cmd: ["test"], state: "exited", ended_ms: 1, exit: 0 }] } };
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
	// Board order: the task first, then the groups in action order.
	assert.deepEqual(workEntries(d, "all", false).slice(0, 4).map(row => row.kind === "todo" ? "todo" : row.id), ["todo", "agent", "job", "ticket"]);
	assert.deepEqual(workEntries(d, "all", false).filter(row => row.kind === "todo").map(row => row.label), ["Todo list: 1/3 completed"]);
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

test("inline details fit the widget and sanitize terminal output", () => {
	const detail = new WorkDetail("Detail", "\u001b[31mtext\u001b[0m", plain, () => 24, () => {});
	assert.match(detail.render(80).join("\n"), /text/);
	assert.ok(detail.render(80).length <= 12);
});

test("refresh keeps stable selection and terminal geometry stays bounded", () => {
	const d = data(); let selected: WorkAction | undefined;
	const panel = new WorkPanel(d, { tab: "jobs", selected: "job:local:job", history: false }, plain, action => { selected = action; }, () => {}, () => 24);
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

test("expanded work keeps the board's header, groups, glyphs, and aligned columns", () => {
	const d = data();
	d.board.hosts = [{ label: "local", service: "up", reachable: true, busy: 3, cores: 12, eligible: true }];
	const panel = new WorkPanel(d, { tab: "all", selected: undefined, history: false }, plain, () => {}, () => {}, () => 40);
	const lines = panel.render(120).map(line => line.trimEnd());
	assert.match(lines[0]!, /^ dogsled {2}1 need attention · 1 running · 1 waiting +local ▰▱▱▱ 3\/12$/);
	assert.deepEqual(lines.slice(2, 10).map(line => line.trim().split(/ {2,}/)[0]), ["TASK", "→ ◆ Verify changes", "ATTENTION", "! reviewer", "RUNNING", "▸ tests", "WAITING", "⋯ queued"]);
	const rows = lines.filter(line => /reviewer|tests|queued/.test(line) && !/^ [A-Z]/.test(line));
	assert.equal(new Set(rows.map(line => line.indexOf(line.includes("reviewer") ? "pi" : "job"))).size, 1);
	panel.handleInput("\u001b[B");
	assert.match(panel.render(120).find(line => line.includes("reviewer"))!, /^→ ! reviewer/);
});

test("history toggle stays inside the panel and todo rows never offer mutation actions", () => {
	const d = data(); let closed = false;
	const panel = new WorkPanel(d, { tab: "jobs", selected: "history", history: false }, plain, () => { closed = true; }, () => {}, () => 30);
	panel.handleInput("\r"); assert.equal(panel.cursor.history, true); assert.equal(closed, false);
	const todo = new WorkPanel(d, { tab: "todos", selected: undefined, history: false }, plain, () => { closed = true; }, () => {}, () => 30);
	for (const input of ["s", "c", "m"]) todo.handleInput(input);
	assert.equal(closed, false);
});

test("inspection uses existing commands and sanitizes output without losing line breaks", async () => {
	const calls: string[][] = [];
	const exec = async (args: string[]) => { calls.push(args); return "\x1b[31mline one\x1b[0m\nline two"; };
	assert.equal(await inspectEntry({ kind: "todo", value: "t", label: "t", detail: "Full\ntodo" }, exec), "Full\ntodo");
	assert.equal(calls.length, 0);
	assert.match(await inspectEntry({ kind: "job", value: "j", label: "j", id: "job" }, exec), /Full log: dogsled logs job\n\nline one\nline two/);
	await inspectEntry({ kind: "agent", value: "a", label: "a", id: "agent", pane: "w2:p1", host: "netcup" }, exec);
	assert.deepEqual(calls, [["read", "job", "--host", "local"], ["read", "w2:p1", "--host", "netcup"]]);
});

test("inline detail viewport scrolls, resizes, and leaves room for the editor", () => {
	let rows = 24;
	const component = new WorkDetail("Logs", Array.from({ length: 60 }, (_, i) => `line ${i}`).join("\n"), plain, () => rows, () => {});
	const before = component.render(45);
	assert.ok(before.length <= 12);
	component.handleInput("\u001b[B");
	const after = component.render(45);
	assert.notDeepEqual(before, after);
	rows = 15;
	assert.ok(component.render(45).length <= 7);
	assert.ok(after.every(line => visibleWidth(line) <= 45));
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
