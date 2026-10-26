import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI, ExtensionContext, ToolCallEvent } from "@earendil-works/pi-coding-agent";
import { bashBlocked, PlanMode, planPathAllowed, type PlanHost } from "./plan-mode.ts";
import { PortalClient } from "./portal-client.ts";

interface Call { method: string; path: string; headers: Record<string, string | string[] | undefined>; body: string }

const DECISION = { review_id: "rev-1", path: "/repo/plans/plan.md", decision: "approved", message: "Looks good.", plan_content: "# Plan" };

/** A portal that answers the review, decision, and stream routes. */
function portal(decisions: string[] = []) {
	const calls: Call[] = [];
	const server = createServer((request, response) => {
		const chunks: Buffer[] = [];
		request.on("data", chunk => chunks.push(chunk as Buffer));
		request.on("end", () => {
			calls.push({ method: request.method ?? "", path: request.url ?? "", headers: request.headers, body: Buffer.concat(chunks).toString("utf8") });
			if (request.method === "POST" && request.url === "/api/reviews") {
				response.writeHead(200, { "content-type": "application/json" });
				response.end(JSON.stringify({ id: "rev-1", revision: 7 }));
				return;
			}
			if (request.method === "POST" && request.url === "/api/decisions/rev-1/ack") {
				response.writeHead(204);
				response.end();
				return;
			}
			if (request.method === "GET" && request.url === "/api/sessions/session-1/decisions") {
				response.writeHead(200, { "content-type": "text/event-stream" });
				for (const decision of decisions) response.write(`event: decision\ndata: ${decision}\n\n`);
				response.end();
				return;
			}
			response.writeHead(404);
			response.end();
		});
	});
	return { calls, server };
}

async function listen(server: Server): Promise<number> {
	await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
	const address = server.address();
	assert.ok(address && typeof address === "object");
	return address.port;
}

/** A state directory with a portal token, and an environment that points at the fake server. */
function environment(port: number): NodeJS.ProcessEnv {
	const state = mkdtempSync(join(tmpdir(), "dogsled-portal-"));
	mkdirSync(join(state, "web"));
	writeFileSync(join(state, "web", "token"), "secret-token");
	return { DOGSLED_STATE: state, DOGSLED_WEB_PORT: String(port) };
}

function context(cwd = "/repo"): ExtensionContext {
	return { cwd, isIdle: () => true, sessionManager: { getSessionId: () => "session-1" } } as unknown as ExtensionContext;
}

/** A host that records what the plan mode sends and which tools it declares. */
function host() {
	const sent: string[] = [];
	const messages: unknown[] = [];
	const active: string[] = ["read", "write", "edit", "bash"];
	const listeners = new Map<string, ((data: unknown) => void)[]>();
	const events: ExtensionAPI["events"] = {
		emit(channel: string, data: unknown) { for (const handler of listeners.get(channel) ?? []) handler(data); },
		on(channel: string, handler: (data: unknown) => void) {
			const list = listeners.get(channel) ?? [];
			list.push(handler);
			listeners.set(channel, list);
			return () => { listeners.set(channel, (listeners.get(channel) ?? []).filter(item => item !== handler)); };
		},
	};
	const plan: PlanHost = {
		events,
		sendUserMessage(content) { sent.push(content); },
		sendMessage(message) { messages.push(message); },
		getActiveTools() { return [...active]; },
		setActiveTools(names) { active.length = 0; active.push(...names); },
	};
	return { plan, sent, messages, active, events };
}

function call(toolName: string, input: unknown): ToolCallEvent {
	return { type: "tool_call", toolName, toolCallId: "c1", input } as unknown as ToolCallEvent;
}

test("planning lets a markdown file change and blocks every other write", async () => {
	const p = portal();
	const port = await listen(p.server);
	p.server.close();
	const mode = new PlanMode(host().plan, new PortalClient(environment(port)));
	const ctx = context();
	assert.equal(await mode.gate(call("write", { path: "src/x.rs" }), ctx), undefined);
	mode.start();
	const blocked = await mode.gate(call("write", { path: "src/x.rs" }), ctx);
	assert.equal(blocked?.block, true);
	assert.match(String(blocked?.reason), /markdown file/);
	assert.equal(await mode.gate(call("write", { path: "plans/x.md" }), ctx), undefined);
	assert.equal(await mode.gate(call("edit", { path: "plans/x.mdx" }), ctx), undefined);
	assert.equal(await mode.gate(call("read", { path: "src/x.rs" }), ctx), undefined);
});

test("planning off lets every write through", async () => {
	const p = portal();
	const port = await listen(p.server);
	p.server.close();
	const mode = new PlanMode(host().plan, new PortalClient(environment(port)));
	const ctx = context();
	assert.equal(await mode.gate(call("write", { path: "src/x.rs" }), ctx), undefined);
	assert.equal(await mode.gate(call("write", { path: "plans/x.md" }), ctx), undefined);
});

test("a plan path must stay inside the working directory", () => {
	assert.equal(planPathAllowed("plans/x.md", "/repo"), true);
	assert.equal(planPathAllowed("/repo/plans/x.md", "/repo"), true);
	assert.equal(planPathAllowed("../outside.md", "/repo"), false);
	assert.equal(planPathAllowed("/etc/notes.md", "/repo"), false);
	assert.equal(planPathAllowed("plans/x.txt", "/repo"), false);
	assert.equal(planPathAllowed("plans/x.MD", "/repo"), true);
});

test("planning blocks a bash command that changes files", async () => {
	const p = portal();
	const port = await listen(p.server);
	p.server.close();
	const mode = new PlanMode(host().plan, new PortalClient(environment(port)));
	const ctx = context();
	mode.start();
	const blocked = await mode.gate(call("bash", { command: "echo hi > out.txt" }), ctx);
	assert.equal(blocked?.block, true);
	assert.equal(await mode.gate(call("bash", { command: "ls -la" }), ctx), undefined);
	assert.equal(await mode.gate(call("bash", { command: "echo hi > /dev/null" }), ctx), undefined);
	assert.equal(await mode.gate(call("bash", { command: "git status" }), ctx), undefined);
	assert.equal(bashBlocked("rm -rf build"), "rm");
	assert.equal(bashBlocked("git commit -m x"), "git commit");
	assert.equal(bashBlocked("npm install lodash"), "npm install");
	assert.equal(bashBlocked("cat file > /dev/null"), undefined);
	assert.equal(bashBlocked("cmd 2>&1"), undefined);
	assert.equal(bashBlocked("cmd 2> err.txt"), "a write redirect");
	assert.equal(bashBlocked("echo 'rm -rf /'"), undefined);
});

test("submit_plan posts the plan and answers at once", async t => {
	const p = portal();
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const mode = new PlanMode(h.plan, new PortalClient(environment(port)));
	const ctx = context();
	const dir = mkdtempSync(join(tmpdir(), "dogsled-plan-"));
	const file = join(dir, "plan.md");
	writeFileSync(file, "# Plan\n\nStep one.");
	const review = await mode.submit(file, ctx);
	assert.deepEqual(review, { id: "rev-1", revision: 7 });
	const post = p.calls.find(call => call.method === "POST" && call.path === "/api/reviews");
	assert.ok(post);
	assert.equal(post.headers["authorization"], "Bearer secret-token");
	assert.deepEqual(JSON.parse(post.body), { path: file, content: "# Plan\n\nStep one.", pane: process.env["HERDR_PANE_ID"] ?? "", session: "session-1", cwd: "/repo" });
	mode.stopWatching();
});

test("a decision is delivered, acknowledged, and handed on as the plan-approved event", async t => {
	const p = portal();
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const mode = new PlanMode(h.plan, new PortalClient(environment(port)));
	const ctx = context();
	const approved: unknown[] = [];
	h.events.on("dogsled:plan-approved", value => { approved.push(value); });
	await mode.decision(JSON.stringify(DECISION), ctx);
	assert.deepEqual(h.sent, ["Looks good."]);
	assert.ok(p.calls.some(call => call.method === "POST" && call.path === "/api/decisions/rev-1/ack"));
	assert.deepEqual(approved, [{ cwd: "/repo", planFilePath: "/repo/plans/plan.md", planContent: "# Plan", feedback: "Looks good." }]);
});

test("a decision that arrives while the agent is busy is steered into the running turn", async t => {
	const p = portal();
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const steered: unknown[] = [];
	h.plan.sendUserMessage = (content: string, options?: { deliverAs?: string }) => { steered.push([content, options?.deliverAs]); };
	const mode = new PlanMode(h.plan, new PortalClient(environment(port)));
	const ctx = { ...context(), isIdle: () => false } as unknown as ExtensionContext;
	await mode.decision(JSON.stringify(DECISION), ctx);
	assert.deepEqual(steered, [["Looks good.", "steer"]]);
});

test("an acknowledged decision is not delivered twice", async t => {
	const p = portal();
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const mode = new PlanMode(h.plan, new PortalClient(environment(port)));
	const ctx = context();
	await mode.decision(JSON.stringify(DECISION), ctx);
	await mode.decision(JSON.stringify(DECISION), ctx);
	assert.deepEqual(h.sent, ["Looks good."]);
});

test("a reconnected decisions stream does not deliver an acknowledged decision twice", async t => {
	const p = portal([JSON.stringify(DECISION)]);
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const mode = new PlanMode(h.plan, new PortalClient(environment(port)));
	const ctx = context();
	const dir = mkdtempSync(join(tmpdir(), "dogsled-plan-"));
	const file = join(dir, "plan.md");
	writeFileSync(file, "# Plan");
	// The stream ends after the decision, so the mode reconnects and reads it again.
	try {
		await mode.submit(file, ctx);
		const deadline = Date.now() + 5_000;
		while (p.calls.filter(call => call.path === "/api/sessions/session-1/decisions").length < 2 && Date.now() < deadline) {
			await new Promise(resolve => setTimeout(resolve, 50));
		}
		assert.deepEqual(h.sent, ["Looks good."]);
		assert.equal(p.calls.filter(call => call.path === "/api/decisions/rev-1/ack").length, 1);
		assert.ok(p.calls.filter(call => call.path === "/api/sessions/session-1/decisions").length >= 2, "the stream reconnected");
	} finally {
		mode.stopWatching();
	}
});

test("entering planning frames the task once and offers the submit tool", async () => {
	const p = portal();
	const port = await listen(p.server);
	p.server.close();
	const h = host();
	const mode = new PlanMode(h.plan, new PortalClient(environment(port)));
	mode.start();
	assert.equal(h.messages.length, 1);
	assert.match(String((h.messages[0] as { content: string }).content), /markdown/);
	assert.ok(h.active.includes("submit_plan"));
	mode.start();
	assert.equal(h.messages.length, 1);
	mode.stop();
	assert.ok(!h.active.includes("submit_plan"));
});
