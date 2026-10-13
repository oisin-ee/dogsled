import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import wrangle from "./index.ts";

type Handler = (event: any, ctx: any) => Promise<any> | void;
function rig() {
	const handlers = new Map<string, Handler>();
	const calls: string[][] = [];
	const widgets: unknown[] = [];
	let code = 0;
	let items: unknown = { items: [], queue: [] };
	const api = {
		on(event: string, fn: Handler) { handlers.set(event, fn); },
		async exec(binary: string, args: string[]) {
			calls.push([binary, ...args]);
			return { code, stdout: args[0] === "status" ? "local 3/8\n" : JSON.stringify(items), stderr: "failure", killed: false };
		},
	};
	wrangle(api as unknown as ExtensionAPI);
	const ctx = { mode: "tui", ui: { setWidget: (...args: unknown[]) => widgets.push(args), notify: () => {} } };
	return { calls, widgets, ctx, fail: () => { code = 2; }, items: (value: unknown) => { items = value; },
		async invoke(name: string, event: unknown = {}) {
			const handler = handlers.get(name); assert.ok(handler); return handler(event, ctx);
		} };
}

test("keeps the fresh brief and surfaces status errors", async () => {
	const r = rig();
	assert.equal(r.calls.length, 0);
	assert.deepEqual(await r.invoke("before_agent_start", { systemPrompt: "base" }), { systemPrompt: "base\n\n## Host status\nlocal 3/8\n" });
	r.fail();
	await assert.rejects(r.invoke("before_agent_start", { systemPrompt: "base" }), /failure/);
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
		{ id: "a1", kind: "agent", closed_ms: null },
	], queue: [{ id: "t1", state: "pending" }, { id: "t2", state: "admitted" }] });
	const spawn = { spawned: true, machine: "peer", target: "w2:p1", name: "worker" };
	await r.invoke("tool_result", { toolName: "agents", isError: false, details: spawn, content: [] });
	assert.ok(r.calls.some(call => call.slice(1).join(" ") === "track peer w2:p1 --name worker"));
	assert.deepEqual(r.widgets.at(-1), ["wrangle", ["job j1 · peer · w2:p1", "queued t1"], { placement: "aboveEditor" }]);
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
