import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import wrangle, { VERSION, versionMismatch } from "./index.ts";

type Handler = (event: any, ctx: any) => Promise<any> | void;
const plain = { fg: (_c: string, t: string) => t, bold: (t: string) => t, bg: (_c: string, t: string) => t };

function rig() {
	const handlers = new Map<string, Handler>();
	const calls: string[][] = [];
	const widgets: any[] = [];
	const notices: unknown[] = [];
	const servers: unknown[] = [];
	const commands = new Map<string, any>();
	const renderers = new Map<string, any>();
	const sent: any[] = [];
	let code = 0;
	let context = "local 3/8\n";
	let missing = false;
	let board: unknown = { hosts: [], rows: [] };
	let version = `wrangle ${VERSION}\n`;
	const api = {
		on(event: string, fn: Handler) { handlers.set(event, fn); },
		registerMcpServer(name: string, config: unknown) { servers.push([name, config]); },
		registerCommand(name: string, options: unknown) { commands.set(name, options); },
		registerMessageRenderer(type: string, fn: unknown) { renderers.set(type, fn); },
		sendMessage(message: unknown, options: unknown) { sent.push([message, options]); },
		async exec(binary: string, args: string[], options?: { timeout: number }) {
			calls.push([binary, ...args]);
			if (missing) throw new Error("ENOENT");
			if (args[0] === "hook") assert.equal(options?.timeout, 6_000);
			const stdout = args[0] === "hook" ? context
				: args[0] === "board" ? JSON.stringify(board)
				: args[1] === "claim" ? "[]"
				: args[1] === "path" ? JSON.stringify({ dir: "/nonexistent-wrangle-inbox", file: "t.json" })
				: args[0] === "--version" ? version
				: "{}";
			return { code, stdout, stderr: "failure", killed: false };
		},
	};
	wrangle(api as unknown as ExtensionAPI);
	let drawn: () => void = () => {};
	const ctx = { mode: "tui", hasUI: true, isIdle: () => true, ui: { setWidget: (...args: unknown[]) => { widgets.push(args); drawn(); }, notify: (...args: unknown[]) => { notices.push(args); drawn(); } } };
	/** Resolves at the next widget or notice: the end of a timer refresh. */
	const next = () => new Promise<void>(resolve => { drawn = resolve; });
	return { calls, widgets, notices, servers, commands, renderers, sent, ctx, next,
		context: (text: string) => { context = text; }, missing: () => { missing = true; }, fail: () => { code = 2; }, recover: () => { code = 0; }, board: (value: unknown) => { board = value; }, version: (text: string) => { version = text; },
		async invoke(name: string, event: unknown = {}) {
			const handler = handlers.get(name); assert.ok(handler, name); return handler(event, ctx);
		} };
}

function withPane(t: { after: (fn: () => void) => void }) {
	const previous = process.env["HERDR_PANE_ID"];
	process.env["HERDR_PANE_ID"] = "w1:p1";
	t.after(() => { if (previous === undefined) delete process.env["HERDR_PANE_ID"]; else process.env["HERDR_PANE_ID"] = previous; });
}

test("registers the wrangle tools, the panel command, and the message renderer", () => {
	const r = rig();
	assert.deepEqual(r.servers.map(s => (s as [string, Record<string, unknown>])[0]), ["wrangle"]);
	const config = (r.servers[0] as [string, Record<string, unknown>])[1];
	assert.deepEqual([config["args"], config["exposure"]], [["mcp"], "direct"]);
	assert.ok(r.commands.has("wrangle"));
	// The card names the sender and kind and shows the text, folded past 12 lines.
	const long = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");
	const message = { content: "x", details: { records: [{ id: "m", kind: "reply", sender: "web (w2:p1 on netcup)", text: long, created_ms: 0 }] } };
	const lines = r.renderers.get("wrangle")(message, { expanded: false, outputPad: 1 }, plain).render(60).map((l: string) => l.trim());
	assert.ok(lines.includes("wrangle reply from web (w2:p1 on netcup)"), lines.join("\n"));
	assert.ok(lines.includes("line 11") && !lines.includes("line 12") && lines.includes("… 8 more lines"));
	const open = r.renderers.get("wrangle")(message, { expanded: true, outputPad: 1 }, plain).render(60).map((l: string) => l.trim());
	assert.ok(open.includes("line 19"));
});

test("uses the shared context hook and leaves prompts unchanged when unavailable", async () => {
	const r = rig();
	assert.equal(r.calls.length, 0);
	assert.deepEqual(await r.invoke("before_agent_start", { systemPrompt: "base" }), { systemPrompt: "base\n\n## Host status\nlocal 3/8\n" });
	assert.deepEqual(r.calls[0]?.slice(1), ["hook", "context", "--harness", "pi"]);
	r.context("");
	assert.equal(await r.invoke("before_agent_start", { systemPrompt: "base" }), undefined);
	r.fail();
	assert.equal(await r.invoke("before_agent_start", { systemPrompt: "base" }), undefined);
	r.missing();
	assert.equal(await r.invoke("before_agent_start", { systemPrompt: "base" }), undefined);
});

test("the board widget follows wrangle board, refreshes every 10 s, and stops at shutdown", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	await r.invoke("session_start");
	// Claims first, so this pane is a registered reader before anyone sends to it.
	assert.deepEqual(r.calls[0]?.slice(1, 3), ["inbox", "claim"]);
	assert.deepEqual(r.widgets.at(-1), ["wrangle", undefined, { placement: "aboveEditor" }]);
	r.board({ hosts: [{ label: "local", service: "up", reachable: true, busy: 1, cores: 8, eligible: true }],
		rows: [{ group: "running", kind: "job", id: "j1", name: "cargo test", what: "job", host: "local", state: "running", activity: "cargo test", age_ms: 60_000 }] });
	const drawn = r.next();
	t.mock.timers.tick(10_000);
	await drawn;
	const [key, factory, options] = r.widgets.at(-1);
	assert.deepEqual([key, options], ["wrangle", { placement: "aboveEditor" }]);
	const lines: string[] = factory(undefined, plain).render(80);
	assert.ok(lines.some(l => l.includes("▸ cargo test")), lines.join("\n"));
	await r.invoke("session_shutdown");
	const after = r.calls.length;
	t.mock.timers.tick(10_000);
	assert.equal(r.calls.length, after);
	assert.deepEqual(r.widgets.at(-1), ["wrangle", undefined]);
});

test("a failing board refresh is reported once and the timer recovers", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	r.fail();
	await r.invoke("session_start");
	const boardFailures = () => r.notices.filter(n => String((n as unknown[])[0]).includes("wrangle board failed")).length;
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

test("the board widget steps aside while /wrangle is open, so no row shows twice", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	r.board({ hosts: [], rows: [{ group: "running", kind: "job", id: "j1", name: "make", what: "job", host: "local", state: "running", activity: "make", age_ms: 0 }] });
	await r.invoke("session_start");
	assert.equal(typeof r.widgets.at(-1)[1], "function");
	let during: unknown;
	const ctx = { ...r.ctx, ui: { ...r.ctx.ui, custom: async () => { during = r.widgets.at(-1)[1]; return undefined; } } };
	await r.commands.get("wrangle").handler("", ctx);
	assert.equal(during, undefined);
	assert.equal(typeof r.widgets.at(-1)[1], "function");
	await r.invoke("session_shutdown");
});

test("without a Herdr pane the extension starts nothing", async () => {
	const previous = process.env["HERDR_PANE_ID"];
	delete process.env["HERDR_PANE_ID"];
	try {
		const r = rig();
		await r.invoke("session_start");
		assert.deepEqual(r.calls, []);
	} finally {
		if (previous !== undefined) process.env["HERDR_PANE_ID"] = previous;
	}
});

test("the extension's version is the package's, and a different wrangle binary is named once at start", async (t) => {
	withPane(t);
	t.mock.timers.enable({ apis: ["setInterval"] });
	const pkg = JSON.parse(await import("node:fs").then(f => f.readFileSync(new URL("../package.json", import.meta.url), "utf8")));
	assert.equal(VERSION, pkg.version);
	assert.equal(versionMismatch(`wrangle ${VERSION}\n`), undefined);
	assert.match(versionMismatch("wrangle 0.9.1\n") ?? "", new RegExp(`^wrangle 0\\.9\\.1 is on PATH, but this extension is ${VERSION.replaceAll(".", "\\.")}\\.`));
	const r = rig();
	await r.invoke("session_start");
	assert.equal(r.notices.length, 0);
	await r.invoke("session_shutdown");
	const old = rig();
	old.version("wrangle 0.9.1\n");
	await old.invoke("session_start");
	assert.deepEqual(old.notices.map(n => (n as unknown[])[1]), ["warning"]);
	await old.invoke("session_shutdown");
});
