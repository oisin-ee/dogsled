import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import wrangle from "./index.ts";

type Handler = (event: any, ctx: any) => Promise<any> | void;
function rig() {
	const handlers = new Map<string, Handler>();
	const calls: string[][] = [];
	const widgets: unknown[] = [];
	const notices: unknown[] = [];
	const servers: unknown[] = [];
	let code = 0;
	let context = "local 3/8\n";
	let missing = false;
	let items: unknown = { items: [], queue: [] };
	const api = {
		on(event: string, fn: Handler) { handlers.set(event, fn); },
		registerMcpServer(name: string, config: unknown) { servers.push([name, config]); },
		async exec(binary: string, args: string[], options?: { timeout: number }) {
			calls.push([binary, ...args]);
			if (missing) throw new Error("ENOENT");
			if (args[0] === "hook") assert.equal(options?.timeout, 6_000);
			return { code, stdout: args[0] === "hook" ? context : JSON.stringify(items), stderr: "failure", killed: false };
		},
	};
	wrangle(api as unknown as ExtensionAPI);
	const ctx = { mode: "tui", ui: { setWidget: (...args: unknown[]) => widgets.push(args), notify: (...args: unknown[]) => notices.push(args) } };
	return { calls, widgets, notices, servers, ctx, context: (text: string) => { context = text; }, missing: () => { missing = true; }, fail: () => { code = 2; }, recover: () => { code = 0; }, items: (value: unknown) => { items = value; },
		async invoke(name: string, event: unknown = {}) {
			const handler = handlers.get(name); assert.ok(handler); return handler(event, ctx);
		} };
}

test("registers the wrangle tools as a direct MCP server", () => {
	const r = rig();
	assert.deepEqual(r.servers.map(s => (s as [string, Record<string, unknown>])[0]), ["wrangle"]);
	const config = (r.servers[0] as [string, Record<string, unknown>])[1];
	assert.deepEqual([config["args"], config["exposure"]], [["mcp"], "direct"]);
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

test("tracks structured and text spawn results, filters widget, and cleans up timer", async (t) => {
	const previous = process.env["HERDR_PANE_ID"];
	process.env["HERDR_PANE_ID"] = "w1:p1";
	t.after(() => { if (previous === undefined) delete process.env["HERDR_PANE_ID"]; else process.env["HERDR_PANE_ID"] = previous; });
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	await r.invoke("session_start");
	assert.deepEqual(r.widgets.at(-1), ["wrangle", undefined, { placement: "aboveEditor" }]);
	r.items({ items: [
		{ id: "j1", kind: "job", host: "peer", pane: "w2:p1", closed_ms: null },
		{ id: "j2", kind: "job", closed_ms: 100 },
		{ id: "a1", kind: "agent", name: "worker", host: "peer", pane: "w3:p1", closed_ms: null },
		{ id: "a2", kind: "agent", name: "closed", closed_ms: 100 },
	], queue: [{ id: "t1", state: "pending" }, { id: "t2", state: "admitted" }] });
	const spawn = { spawned: true, machine: "peer", target: "w2:p1", name: "worker" };
	await r.invoke("tool_result", { toolName: "agents", isError: false, details: spawn, content: [] });
	assert.ok(r.calls.some(call => call.slice(1).join(" ") === "track peer w2:p1 --name worker"));
	assert.deepEqual(r.widgets.at(-1), ["wrangle", ["job j1 · peer · w2:p1", "child worker · peer · w3:p1 · wrangle close when done", "queued t1"], { placement: "aboveEditor" }]);
	await r.invoke("tool_result", { toolName: "agents", isError: false, content: [{ type: "text", text: JSON.stringify(spawn) }] });
	assert.equal(r.calls.filter(call => call[1] === "track").length, 2);
	await r.invoke("tool_result", { toolName: "agents", isError: false, content: [{ type: "text", text: "not a spawn" }] });
	const before = r.calls.length;
	t.mock.timers.tick(10_000);
	await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
	assert.equal(r.calls.length, before + 1);
	await r.invoke("session_shutdown");
	t.mock.timers.tick(10_000);
	assert.equal(r.calls.length, before + 1);
	assert.deepEqual(r.widgets.at(-1), ["wrangle", undefined]);
});

test("tool_call places agents spawns and ignores other tools", async () => {
	const r = rig();
	assert.equal(await r.invoke("tool_call", { toolName: "bash", input: { command: "ls" } }), undefined);
	const input = { action: "spawn", pane: "w8M:p1", cwd: "/x" };
	assert.equal(await r.invoke("tool_call", { toolName: "agents", input }), undefined);
	assert.deepEqual(input, { action: "spawn", pane: "w8M:p1", placement: "pane" });
	assert.equal(r.calls.length, 0);
});

test("startup refresh failure notifies and the timer recovers", async (t) => {
	const previous = process.env["HERDR_PANE_ID"];
	process.env["HERDR_PANE_ID"] = "w1:p1";
	t.after(() => { if (previous === undefined) delete process.env["HERDR_PANE_ID"]; else process.env["HERDR_PANE_ID"] = previous; });
	t.mock.timers.enable({ apis: ["setInterval"] });
	const r = rig();
	r.fail();
	await r.invoke("session_start");
	assert.equal(r.notices.length, 1);
	assert.equal(r.widgets.length, 0);
	r.recover();
	r.items({ items: [{ id: "j1", kind: "job", host: "local", pane: "w2:p1", closed_ms: null }], queue: [] });
	t.mock.timers.tick(10_000);
	await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
	assert.deepEqual(r.widgets.at(-1), ["wrangle", ["job j1 · local · w2:p1"], { placement: "aboveEditor" }]);
	await r.invoke("session_shutdown");
});
