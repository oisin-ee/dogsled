import assert from "node:assert/strict";
import { test } from "node:test";
import { EventEmitter } from "node:events";
import { createServer } from "node:net";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SessionManager, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { realpathSync } from "node:fs";
import dogsled, { VERSION, versionMismatch } from "./index.ts";

type Handler = (event: any, ctx: any) => Promise<any> | void;
const plain = { fg: (_c: string, t: string) => t, bold: (t: string) => t, bg: (_c: string, t: string) => t };

function rig() {
	const handlers = new Map<string, Handler[]>();
	const manager = SessionManager.inMemory(process.cwd(), { id: "session" });
	const calls: string[][] = [];
	const widgets: any[] = [];
	const notices: unknown[] = [];
	const servers: unknown[] = [];
	const commands = new Map<string, any>();
	const shortcuts = new Map<string, any>();
	const renderers = new Map<string, any>();
	const tools = new Map<string, any>();
	const sent: any[] = [];
	const receipts: { customType: string; data: unknown }[] = [];
	const userMessages: string[] = [];
	let active: string[] = ["read", "write", "edit", "bash"];
	let code = 0;
	let context = "local 3/8\n";
	let gate = "";
	let missing = false;
	let board: unknown = { hosts: [], rows: [] };
	let opened: unknown = { pane_id: "w1:p2", terminal_id: "term-board" };
	let version = `dogsled ${VERSION}\n`;
	const events = new EventEmitter();
	const api = {
		events: { emit: (name: string, data: unknown) => { events.emit(name, data); }, on: (name: string, fn: (data: unknown) => void) => { events.on(name, fn); return () => { events.off(name, fn); }; } },
		on(event: string, fn: Handler) {
			const registered = handlers.get(event) ?? [];
			registered.push(fn);
			handlers.set(event, registered);
		},
		registerFlag() {},
		getFlag() { return undefined; },
		registerMcpServer(name: string, config: unknown) { servers.push([name, config]); },
		registerCommand(name: string, options: unknown) { commands.set(name, options); },
		registerShortcut(key: string, options: unknown) { shortcuts.set(key, options); },
		registerMessageRenderer(type: string, fn: unknown) { renderers.set(type, fn); },
		appendEntry(customType: string, data: unknown) { receipts.push({ customType, data }); manager.appendCustomEntry(customType, data); },
		getAllTools() { return [...tools.keys()].map(name => ({ name, sourceInfo: { path: realpathSync(new URL("./index.ts", import.meta.url)) } })); },
		registerTool(tool: unknown) { tools.set((tool as { name: string }).name, tool); },
		getActiveTools() { return [...active]; },
		setActiveTools(names: string[]) { active = [...names]; },
		sendUserMessage(content: string) { userMessages.push(content); },
		sendMessage(message: unknown, options: unknown) { sent.push([message, options]); },
		async exec(binary: string, args: string[], options?: { timeout: number }) {
			calls.push([binary, ...args]);
			if (missing) throw new Error("ENOENT");
			if (args[0] === "hook") assert.equal(options?.timeout, 6_000);
			const stdout = args[0] === "hook" ? (args[1] === "gate" ? gate : context)
				: args[0] === "board" && args.includes("--open") ? JSON.stringify(opened)
				: args[0] === "board" && args.includes("--todo-path") ? JSON.stringify({ path: "/nonexistent-dogsled-todo/pane.json", width: 48, height: 10_000 })
				: args[0] === "board" ? JSON.stringify(board)
				: args[1] === "claim" ? "[]"
				: args[1] === "path" ? JSON.stringify({ dir: "/nonexistent-dogsled-inbox", file: "t.json" })
				: args[0] === "unit" && ["prepared", "settlements"].includes(args[1] ?? "") ? "[]"
				: args[0] === "items" ? '{"items":[]}'
				: args[0] === "ps" || args[0] === "machine" ? '[]'
				: args[0] === "--version" ? version
				: "{}";
			return { code, stdout, stderr: "failure", killed: false };
		},
	};
	dogsled(api as unknown as ExtensionAPI);
	let drawn: () => void = () => {};
	const components = new Map<string, any>();
	const ctx = { mode: "tui", hasUI: true, cwd: process.cwd(), sessionManager: manager, isIdle: () => true, ui: { theme: plain,
		setWidget: (...args: any[]) => {
			components.get(args[0])?.dispose?.();
			widgets.push(args);
			const component = typeof args[1] === "function" ? args[1]({ terminal: { rows: 24 }, requestRender() { drawn(); } }, plain) : undefined;
			components.set(args[0], component);
			component?.render(100);
			drawn();
		}, notify: (...args: unknown[]) => { notices.push(args); drawn(); } } };
	/** Resolves at the next widget or notice: the end of a timer refresh. */
	const next = () => new Promise<void>(resolve => { drawn = resolve; });
	return { calls, widgets, notices, servers, commands, shortcuts, renderers, tools, sent, receipts, userMessages, active: () => [...active], ctx, next,
		widget() { return components.get("dogsled"); },
		gate: (text: string) => { gate = text; },
		context: (text: string) => { context = text; }, missing: () => { missing = true; }, fail: () => { code = 2; }, recover: () => { code = 0; }, board: (value: unknown) => { board = value; }, opened: (value: unknown) => { opened = value; }, version: (text: string) => { version = text; },
		async invoke(name: string, event: any = {}) {
			const registered = handlers.get(name); assert.ok(registered, name);
			let current = name === "agent_before_settle" ? { entries: [], continue: false, ...event } : event;
			let result: any;
			for (const handler of registered) {
				const next = await handler(current, ctx);
				if (next && Object.keys(next).length) {
					result = next;
					if (name === "tool_call" && next.block) return next;
					if (name === "agent_before_settle") current = { ...current, ...next };
				}
			}
			return result;
		} };
}

function withPane(t: { after: (fn: () => void) => void }) {
	const herdr = process.env["HERDR_ENV"];
	delete process.env["HERDR_ENV"];
	t.after(() => { if (herdr === undefined) delete process.env["HERDR_ENV"]; else process.env["HERDR_ENV"] = herdr; });
	const previous = process.env["HERDR_PANE_ID"];
	process.env["HERDR_PANE_ID"] = "w1:p1";
	t.after(() => { if (previous === undefined) delete process.env["HERDR_PANE_ID"]; else process.env["HERDR_PANE_ID"] = previous; });
}

test("successful claim and accept tools append audit receipts without task operations", async () => {
	const r = rig();
	for (const [kind, receipt] of [
		["claim", { kind: "claim", item: "r-1", outcome: "claimed", claimId: "claim-1", revision: 1, custodianPane: "w1:p1" }],
		["accept", { kind: "accept", item: "r-1", claimId: "claim-1", revision: 1, resultId: "res-1", reviewId: "review-1", applied: true }],
	] as const) {
		const event = { toolName: `mcp__dogsled__${kind}`, toolCallId: `call-${kind}`, content: [{ type: "text", text: JSON.stringify(receipt) }], isError: false, input: {} };
		assert.equal(await r.invoke("tool_result", event), undefined);
		assert.deepEqual(r.receipts.at(-1), { customType: "dogsled.custody.v1", data: receipt });
	}
	assert.equal(r.receipts.length, 2);
	assert.deepEqual(r.calls, []);
	assert.deepEqual(r.sent, []);
	assert.deepEqual(r.userMessages, []);
});

test("custody audit ignores refusals, unrelated tools, and malformed receipts", async () => {
	const r = rig();
	for (const patch of [
		{ isError: true },
		{ toolName: "mcp__dogsled__status" },
		{ content: [{ type: "text", text: "not JSON" }] },
		{ content: [{ type: "text", text: JSON.stringify({ kind: "accept", item: "r-1" }) }] },
		{ content: [{ type: "text", text: JSON.stringify({ error: "refused" }) }] },
	]) {
		await r.invoke("tool_result", { toolName: "mcp__dogsled__claim", toolCallId: "c", isError: false, input: {}, content: [{ type: "text", text: JSON.stringify({ kind: "claim", item: "r-1", outcome: "held", revision: 1 }) }], ...patch });
	}
	assert.deepEqual(r.receipts, []);
	assert.deepEqual(r.calls, []);
});

test("Herdr owns the display: requests the side split and never sets a widget", async t => {
	withPane(t);
	const dir = await mkdtemp(join(tmpdir(), "dogsled-side-"));
	const socket = join(dir, "herdr.sock");
	const server = createServer(client => client.end());
	await new Promise<void>(resolve => server.listen(socket, resolve));
	const previous = process.env["HERDR_SOCKET_PATH"];
	process.env["HERDR_ENV"] = "1";
	process.env["HERDR_SOCKET_PATH"] = socket;
	t.after(async () => {
		if (previous === undefined) delete process.env["HERDR_SOCKET_PATH"]; else process.env["HERDR_SOCKET_PATH"] = previous;
		await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
		await rm(dir, { recursive: true, force: true });
	});
	const r = rig();
	await r.invoke("session_start");
	assert.ok(r.calls.some(call => call.slice(1).join(" ") === "board --open --owner w1:p1 --json"));
	assert.equal(r.widgets.filter(([key]) => key === "dogsled").length, 0);
	assert.ok(r.commands.has("dogsled"));
	await r.commands.get("work").handler("", r.ctx);
	assert.equal(r.widgets.filter(([key]) => key === "dogsled").length, 0);
	await r.invoke("session_shutdown");
	assert.equal(r.widgets.filter(([key]) => key === "dogsled").length, 0);
});

test("a Dogsled-launched child under Herdr keeps the widget: the board belongs to its parent", async t => {
	withPane(t);
	const dir = await mkdtemp(join(tmpdir(), "dogsled-child-"));
	const socket = join(dir, "herdr.sock");
	const server = createServer(client => client.end());
	await new Promise<void>(resolve => server.listen(socket, resolve));
	const previous = process.env["HERDR_SOCKET_PATH"];
	process.env["HERDR_ENV"] = "1";
	process.env["HERDR_SOCKET_PATH"] = socket;
	t.after(async () => {
		if (previous === undefined) delete process.env["HERDR_SOCKET_PATH"]; else process.env["HERDR_SOCKET_PATH"] = previous;
		await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
		await rm(dir, { recursive: true, force: true });
	});
	const r = rig();
	r.opened({ skipped: "child" });
	r.board({ hosts: [], rows: [{ group: "running", kind: "agent", id: "one", name: "worker", what: "pi", host: "local", state: "working", activity: "", age_ms: 0 }] });
	await r.invoke("session_start");
	assert.ok(r.calls.some(call => call.slice(1).join(" ") === "board --open --owner w1:p1 --json"));
	assert.ok(r.widgets.some(call => typeof call[1] === "function" && call[2]?.placement === "aboveEditor"));
	await r.invoke("session_shutdown");
});

test("an unreachable Herdr socket keeps the existing above-editor widget", async t => {
	withPane(t);
	const previous = process.env["HERDR_SOCKET_PATH"];
	process.env["HERDR_ENV"] = "1";
	process.env["HERDR_SOCKET_PATH"] = "/nonexistent-dogsled-herdr.sock";
	t.after(() => { if (previous === undefined) delete process.env["HERDR_SOCKET_PATH"]; else process.env["HERDR_SOCKET_PATH"] = previous; });
	const r = rig();
	r.board({ hosts: [], rows: [{ group: "running", kind: "agent", id: "one", name: "worker", what: "pi", host: "local", state: "working", activity: "", age_ms: 0 }] });
	await r.invoke("session_start");
	assert.ok(r.widgets.some(call => typeof call[1] === "function" && call[2]?.placement === "aboveEditor"));
	assert.ok(!r.calls.some(call => call.includes("--open")));
	await r.invoke("session_shutdown");
});

test("registers the dogsled tools, the panel command, and the message renderer", () => {
	const r = rig();
	assert.deepEqual(r.servers.map(s => (s as [string, Record<string, unknown>])[0]), ["dogsled"]);
	const config = (r.servers[0] as [string, Record<string, unknown>])[1];
	assert.deepEqual([config["args"], config["exposure"]], [["mcp"], "direct"]);
	assert.match(String(config["description"]), /native done, ask, and blocked tools/);
	assert.match(String(config["description"]), /End every turn with exactly one of: done \(whole task complete\), ask \(you need an answer from your owner\), blocked \(you cannot continue\), or waiting for your own open children/);
	assert.ok(!String(config["description"]).includes("dogsled done"));
	assert.ok(r.commands.has("dogsled"));
	// The card names the sender and kind and shows the text, folded past 12 lines.
	const long = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
	const message = { content: "x", details: { records: [{ id: "m", kind: "reply", sender: "web (w2:p1 on netcup)", text: long, created_ms: 0 }] } };
	const lines = r.renderers.get("dogsled")(message, { expanded: false, outputPad: 1 }, plain).render(60).map((l: string) => l.trim());
	assert.ok(lines.includes("dogsled reply from web (w2:p1 on netcup)"), lines.join("\n"));
	assert.ok(lines.includes("line 11") && !lines.includes("line 12") && lines.includes("… 8 more lines"));
	const open = r.renderers.get("dogsled")(message, { expanded: true, outputPad: 1 }, plain).render(60).map((l: string) => l.trim());
	assert.ok(open.includes("line 19"));
});

test("uses the shared context hook and leaves prompts unchanged when unavailable", async () => {
	const r = rig();
	await r.tools.get("todo").execute("setup", { op: "view" }, undefined, undefined, r.ctx);
	assert.equal(r.calls.length, 0);
	assert.deepEqual(await r.invoke("before_agent_start", { systemPrompt: "base" }), { systemPrompt: "base\n\n## Your work and fleet status\nlocal 3/8\n" });
	assert.deepEqual(r.calls.map(call => call.slice(1)), [["unit", "prepared"], ["unit", "settlements"], ["hook", "context", "--harness", "pi"]]);
	r.context("");
	assert.equal(await r.invoke("before_agent_start", { systemPrompt: "base" }), undefined);
	r.fail();
	assert.equal(await r.invoke("before_agent_start", { systemPrompt: "base" }), undefined);
	r.missing();
	assert.equal(await r.invoke("before_agent_start", { systemPrompt: "base" }), undefined);
});

test("stop gate continues with a visible reminder and preserves prior boundary entries", async t => {
	withPane(t);
	const r = rig();
	const event = { outcome: "completed", context: { canContinue: true }, continue: false, entries: [{ type: "custom", customType: "other", data: 1 }] };
	r.gate("Before you stop: child (uncommitted).\n");
	const result = await r.invoke("agent_before_settle", event);
	assert.deepEqual(result, { entries: [...event.entries, { type: "custom_message", customType: "dogsled", content: "Before you stop: child (uncommitted).", display: true }], continue: true });
	assert.deepEqual(r.calls[0]?.slice(1), ["hook", "gate"]);
	r.gate("");
	assert.equal(await r.invoke("agent_before_settle", event), undefined);
	const count = r.calls.length;
	for (const patch of [{ outcome: "aborted" }, { outcome: "error" }, { continue: true }]) {
		assert.equal(await r.invoke("agent_before_settle", { ...event, ...patch }), undefined);
	}
	assert.equal(r.calls.length, count);
	// A normal stop ends on the assistant, so Pi reports the pre-reminder context as not continuable.
	const assistantFinal = { ...event, context: { canContinue: false } };
	r.gate("");
	const before = r.calls.length;
	assert.equal(await r.invoke("agent_before_settle", assistantFinal), undefined);
	assert.deepEqual(r.calls.slice(before).map(c => c.slice(1)), [["hook", "gate"]]);
	r.gate("Before you stop: child (uncommitted).\n");
	assert.deepEqual(await r.invoke("agent_before_settle", assistantFinal), { entries: [...event.entries, { type: "custom_message", customType: "dogsled", content: "Before you stop: child (uncommitted).", display: true }], continue: true });
	r.fail();
	await assert.rejects(r.invoke("agent_before_settle", event), /stop gate failed/);
	r.missing();
	await assert.rejects(r.invoke("agent_before_settle", event), /ENOENT/);
});

test("stop gate shows cleanup warnings, dirty worktrees, and failures without interpreting them", async t => {
	withPane(t);
	const r = rig();
	for (const content of [
		"Cleanup repository warnings:\n/missing: repository entry /missing in repositories.json. Hand the fix to a dogsled agent.",
		"Landed worktrees still have dirty files:\n/repo/.worktrees/x (branch feat/x): its commits are already in the default branch. Dirty paths: notes.txt.",
		"Cleanup pass at 123 ms failed:\nworkspace list: survey unavailable",
	]) {
		r.gate(`${content}\n`);
		const result = await r.invoke("agent_before_settle", { outcome: "completed", context: { canContinue: true }, entries: [] });
		assert.deepEqual(result, { entries: [{ type: "custom_message", customType: "dogsled", content, display: true }], continue: true });
		r.gate("");
		assert.equal(await r.invoke("agent_before_settle", { outcome: "completed", context: { canContinue: true } }), undefined);
	}
});

test("the stop gate does not query fleet-wide work without an owner", async t => {
	const previous = process.env["HERDR_PANE_ID"];
	delete process.env["HERDR_PANE_ID"];
	t.after(() => { if (previous !== undefined) process.env["HERDR_PANE_ID"] = previous; });
	const r = rig();
	assert.equal(await r.invoke("agent_before_settle", { outcome: "completed", context: { canContinue: true } }), undefined);
	assert.equal(r.calls.length, 0);
});

test("the board widget follows dogsled board, refreshes every 10 s, and stops at shutdown", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	await r.invoke("session_start");
	// Claims first, so this pane is a registered reader before anyone sends to it.
	assert.deepEqual(r.calls[0]?.slice(1, 3), ["inbox", "claim"]);
	assert.deepEqual(r.widgets.at(-1), ["dogsled", undefined]);
	r.board({ hosts: [{ label: "local", service: "up", reachable: true, busy: 1, cores: 8, eligible: true }],
		rows: [{ group: "running", kind: "job", id: "j1", name: "cargo test", what: "job", host: "local", state: "running", activity: "cargo test", age_ms: 60_000 }] });
	const drawn = r.next();
	t.mock.timers.tick(10_000);
	await drawn;
	const [key, , options] = r.widgets.at(-1);
	assert.deepEqual([key, options], ["dogsled", { placement: "aboveEditor" }]);
	const lines: string[] = r.widget().render(80);
	assert.deepEqual(lines.map(line => line.trim().split(/ {2,}/)), [["dogsled", "1 working", "local ▰▱▱▱ 1/8"], ["WORKING"], ["▸ cargo test", "job", "local", "cargo test", "1m"]]);
	await r.invoke("session_shutdown");
	const after = r.calls.length;
	t.mock.timers.tick(10_000);
	assert.equal(r.calls.length, after);
	assert.deepEqual(r.widgets.at(-1), ["dogsled", undefined]);
});

test("a failing board refresh is reported once and the timer recovers", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	r.fail();
	await r.invoke("session_start");
	const boardFailures = () => r.notices.filter(n => String((n as unknown[])[0]).startsWith("Error: dogsled board failed")).length;
	assert.equal(boardFailures(), 1);
	r.recover();
	r.board({ hosts: [], rows: [{ group: "waiting", kind: "agent", id: "t1", name: "lints", what: "pi · sol", host: "any", state: "queued", activity: "for a host with room", age_ms: 0 }] });
	const drawn = r.next();
	t.mock.timers.tick(10_000);
	await drawn;
	assert.equal(typeof r.widgets.at(-1)[1], "function");
	assert.equal(boardFailures(), 1);
	await r.invoke("session_shutdown");
});

test("the board widget steps aside while /dogsled is open, so no row shows twice", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	r.board({ hosts: [], rows: [{ group: "running", kind: "job", id: "j1", name: "make", what: "job", host: "local", state: "running", activity: "make", age_ms: 0 }] });
	await r.invoke("session_start");
	assert.equal(typeof r.widgets.at(-1)[1], "function");
	let during: unknown;
	const ctx = { ...r.ctx, ui: { ...r.ctx.ui, custom: async () => { during = r.widgets.at(-1)[1]; return undefined; } } };
	await r.commands.get("dogsled").handler("", ctx);
	assert.equal(during, undefined);
	assert.equal(typeof r.widgets.at(-1)[1], "function");
	await r.invoke("session_shutdown");
});

test("Alt+W and /work expand the same above-editor widget without an overlay or editor changes", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	r.board({ hosts: [], rows: [{ group: "running", kind: "job", id: "j1", name: "tests", what: "job", host: "local", state: "running", activity: "tests", age_ms: 0 }] });
	await r.invoke("session_start");
	const compact = r.widget().render(100);
	assert.equal(r.calls.some(args => args.includes("ps")), false);
	const ctx = { ...r.ctx, ui: { ...r.ctx.ui,
		custom: async () => assert.fail("work must never open a custom screen"),
		setEditorText: () => assert.fail("draft must remain untouched"),
	} };
	await r.shortcuts.get("alt+w").handler(ctx);
	assert.deepEqual(r.widgets.at(-1)[2], { placement: "aboveEditor" });
	const panel = r.widget();
	assert.ok(panel.render(100).length > compact.length);
	assert.match(panel.render(100).join("\n"), /tests/);
	assert.equal(r.calls.filter(args => args.includes("ps")).length, 1);
	await r.commands.get("work").handler("", ctx);
	assert.deepEqual(r.widget().render(100), compact);
	const drawn = r.next();
	t.mock.timers.tick(10_000);
	await drawn;
	assert.equal(r.calls.filter(args => args.includes("ps")).length, 1);
	await r.invoke("session_shutdown");
});

test("work navigation uses modified keys and resets on session replacement", async t => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	await r.invoke("session_start");
	await r.shortcuts.get("alt+w").handler(r.ctx);
	const panel = r.widget();
	await r.shortcuts.get("alt+]").handler(r.ctx);
	assert.match(panel.render(100).join("\n"), /\[TODOS\]/);
	// The work panel's own keys are all alt-modified; the plan mode keeps its own shortcut.
	const work = [...r.shortcuts.keys()].filter(key => key !== "ctrl+alt+p");
	assert.ok(work.length > 0 && work.every(key => key.startsWith("alt+")), work.join(" "));
	await r.invoke("session_start");
	assert.equal(r.widgets.at(-1)[1], undefined);
	await r.invoke("session_shutdown");
});

test("shutdown aborts a pending work confirmation before it can mutate a job", async t => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	await r.invoke("session_start");
	let signal: AbortSignal | undefined;
	r.board({ hosts: [], rows: [{ group: "running", kind: "job", id: "j", name: "tests", what: "job", host: "local", state: "running", activity: "tests", age_ms: 0 }] });
	let confirmed!: () => void;
	const finished = new Promise<void>(resolve => { confirmed = resolve; });
	const ctx = { ...r.ctx, ui: { ...r.ctx.ui,
		confirm: async (_title: string, _body: string, options: { signal: AbortSignal }) => {
			signal = options.signal;
			await r.invoke("session_shutdown");
			confirmed();
			return true;
		},
	} };
	await r.commands.get("work").handler("", ctx);
	r.widget().render(100);
	await r.shortcuts.get("alt+s").handler(ctx);
	await finished;
	assert.equal(signal?.aborted, true);
	assert.equal(r.calls.some(args => args[1] === "stop"), false);
});

test("without a Herdr pane the extension starts no commands and explains unavailable jobs", async () => {
	const previous = process.env["HERDR_PANE_ID"];
	delete process.env["HERDR_PANE_ID"];
	try {
		const r = rig();
		await r.invoke("session_start");
		assert.deepEqual(r.calls, []);
		const lines: string[] = r.widget().render(100);
		assert.match(lines.join(""), /Herdr owner pane/);
		await r.invoke("session_shutdown");
	} finally {
		if (previous !== undefined) process.env["HERDR_PANE_ID"] = previous;
	}
});

test("the extension's version is the package's, and a different dogsled binary is named once at start", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const pkg = JSON.parse(await import("node:fs").then(f => f.readFileSync(new URL("../package.json", import.meta.url), "utf8")));
	assert.equal(VERSION, pkg.version);
	assert.equal(versionMismatch(`dogsled ${VERSION}\n`), undefined);
	assert.match(versionMismatch("dogsled 0.9.1\n") ?? "", new RegExp(`^dogsled 0\\.9\\.1 is on PATH, but this extension is ${VERSION.replaceAll(".", "\\.")}\\.`));
	const r = rig();
	await r.invoke("session_start");
	assert.equal(r.notices.length, 0);
	await r.invoke("session_shutdown");
	const old = rig();
	old.version("dogsled 0.9.1\n");
	await old.invoke("session_start");
	assert.deepEqual(old.notices.map(n => (n as unknown[])[1]), ["warning"]);
	await old.invoke("session_shutdown");
});

test("the extension registers the interview and submit_plan tools, the plan command, and its shortcut", () => {
	const r = rig();
	assert.ok(r.tools.has("interview"));
	assert.ok(r.tools.has("submit_plan"));
	assert.ok(r.commands.has("plan"));
	assert.ok(r.shortcuts.has("ctrl+alt+p"));
	// The submit tool belongs to planning, so it is not declared to the model at first.
	assert.equal((r.tools.get("submit_plan") as { defaultActive: boolean }).defaultActive, false);
	assert.equal((r.tools.get("submit_plan") as { executionMode: string }).executionMode, "sequential");
});

test("plan mode blocks a write until the plan command turns it on", async () => {
	const r = rig();
	await r.tools.get("todo").execute("setup", { op: "init", start: "fixture", items: [{ key: "fixture", content: "Test plan-mode writes", contract: {
		deliverable: "Exercise plan-mode gating with an active unit", scope: { repositoryId: "github.com/oisin-ee/dogsled", owns: ["plans/", "src/"], reads: [], generates: [] },
		acceptance: { kind: "review", artifact: "fixture", criterion: "write gating asserted" }, dependsOn: [], assignment: { kind: "self" },
	} }] }, undefined, undefined, r.ctx);
	const write = { type: "tool_call", toolName: "write", toolCallId: "c1", input: { path: "src/x.rs" } };
	assert.deepEqual(await r.invoke("tool_call", write), { block: false });
	await r.commands.get("plan").handler("", r.ctx);
	const blocked = await r.invoke("tool_call", write);
	assert.equal(blocked?.block, true);
	assert.match(String(blocked?.reason), /markdown file/);
	const allowed = await r.invoke("tool_call", { type: "tool_call", toolName: "write", toolCallId: "c2", input: { path: "plans/x.md" } });
	assert.deepEqual(allowed, { block: false });
	assert.ok(r.active().includes("submit_plan"));
	await r.commands.get("plan").handler("", r.ctx);
	assert.deepEqual(await r.invoke("tool_call", write), { block: false });
	assert.ok(!r.active().includes("submit_plan"));
});

test("plan mode frames the task as a conversation message, not a system prompt change", async () => {
	const r = rig();
	await r.commands.get("plan").handler("", r.ctx);
	assert.equal(r.sent.length, 1);
	const [message] = r.sent[0] as [{ customType: string; content: string; display: boolean }];
	assert.equal(message.customType, "dogsled:plan-mode");
	assert.match(message.content, /submit_plan/);
	assert.equal(message.display, true);
});
