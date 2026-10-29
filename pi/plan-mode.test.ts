import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionAPI, ExtensionContext, ExtensionToolContext, ToolCallEvent } from "@earendil-works/pi-coding-agent";
import { bashBlocked, PlanMode, planPathAllowed, registerPlanMode, type PlanHost } from "./plan-mode.ts";
import { PortalClient } from "./portal-client.ts";

interface Call { method: string; path: string; headers: Record<string, string | string[] | undefined>; body: string }

const DECISION = { review_id: "rev-1", path: "/repo/plans/plan.md", decision: "approved", message: "Looks good.", plan_content: "# Plan" };

/** A portal that answers the review, decision, and stream routes. */
function portal(decisions: string[] = [], reviews = [{ id: "rev-1", revision: 7 }]) {
	const calls: Call[] = [];
	let submitted = 0;
	const server = createServer((request, response) => {
		const chunks: Buffer[] = [];
		request.on("data", chunk => chunks.push(chunk as Buffer));
		request.on("end", () => {
			calls.push({ method: request.method ?? "", path: request.url ?? "", headers: request.headers, body: Buffer.concat(chunks).toString("utf8") });
			if (request.method === "POST" && request.url === "/api/reviews") {
				response.writeHead(200, { "content-type": "application/json" });
				response.end(JSON.stringify(reviews[Math.min(submitted++, reviews.length - 1)]));
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
	const plan: PlanHost = {
		sendUserMessage(content) { sent.push(content); },
		sendMessage(message) { messages.push(message); },
		getActiveTools() { return [...active]; },
		setActiveTools(names) { active.length = 0; active.push(...names); },
	};
	return { plan, sent, messages, active };
}

function call(toolName: string, input: unknown): ToolCallEvent {
	return { type: "tool_call", toolName, toolCallId: "c1", input } as unknown as ToolCallEvent;
}

test("planning lets a markdown file change and blocks every other write", async () => {
	const p = portal();
	const port = await listen(p.server);
	p.server.close();
	const mode = new PlanMode(host().plan, () => {}, new PortalClient(environment(port)), () => undefined);
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
	const mode = new PlanMode(host().plan, () => {}, new PortalClient(environment(port)), () => undefined);
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
	const mode = new PlanMode(host().plan, () => {}, new PortalClient(environment(port)), () => undefined);
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

test("submit_plan posts the plan and answers at once without reopening a later revision", async t => {
	const p = portal();
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const opened: string[] = [];
	const mode = new PlanMode(h.plan, () => {}, new PortalClient(environment(port)), url => { opened.push(url); });
	const ctx = context();
	const dir = mkdtempSync(join(tmpdir(), "dogsled-plan-"));
	const file = join(dir, "plan.md");
	writeFileSync(file, "# Plan\n\nStep one.");
	const review = await mode.submit(file, ctx);
	assert.deepEqual(review, { id: "rev-1", revision: 7, url: `http://127.0.0.1:${port}/?review=rev-1` });
	assert.deepEqual(opened, []);
	const post = p.calls.find(call => call.method === "POST" && call.path === "/api/reviews");
	assert.ok(post);
	assert.equal(post.headers["authorization"], "Bearer secret-token");
	assert.equal(post.headers["origin"], undefined);
	assert.deepEqual(JSON.parse(post.body), { path: file, content: "# Plan\n\nStep one.", pane: process.env["HERDR_PANE_ID"] ?? "", session: "session-1", cwd: "/repo" });
	mode.stopWatching();
});

test("plan reviews open only the first revision of each review id", async t => {
	const p = portal([], [{ id: "rev-1", revision: 1 }, { id: "rev-1", revision: 2 }, { id: "rev &2", revision: 1 }]);
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const directory = mkdtempSync(join(tmpdir(), "dogsled-plan-open-"));
	t.after(() => rmSync(directory, { recursive: true, force: true }));
	const file = join(directory, "plan.md");
	writeFileSync(file, "# Plan");
	const opened: string[] = [];
	const mode = new PlanMode(host().plan, () => {}, new PortalClient(environment(port)), url => { opened.push(url); });
	t.after(() => mode.stopWatching());
	const firstUrl = `http://127.0.0.1:${port}/?review=rev-1`;
	assert.deepEqual(await mode.submit(file, context()), { id: "rev-1", revision: 1, url: firstUrl });
	assert.deepEqual(opened, [firstUrl]);
	writeFileSync(file, "# Revised plan");
	assert.deepEqual(await mode.submit(file, context()), { id: "rev-1", revision: 2, url: firstUrl });
	assert.deepEqual(opened, [firstUrl]);
	const nextUrl = `http://127.0.0.1:${port}/?review=rev%20%262`;
	assert.deepEqual(await mode.submit(file, context()), { id: "rev &2", revision: 1, url: nextUrl });
	assert.deepEqual(opened, [firstUrl, nextUrl]);
});

test("the submit_plan tool text includes the review URL", async t => {
	const p = portal([], [{ id: "rev-1", revision: 1 }]);
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const directory = mkdtempSync(join(tmpdir(), "dogsled-plan-tool-"));
	t.after(() => rmSync(directory, { recursive: true, force: true }));
	const file = join(directory, "plan.md");
	writeFileSync(file, "# Plan");
	let tool: Parameters<ExtensionAPI["registerTool"]>[0] | undefined;
	const pi = {
		...host().plan,
		registerTool(value: Parameters<ExtensionAPI["registerTool"]>[0]) { tool = value; },
		registerCommand() {},
		registerShortcut() {},
	} as unknown as ExtensionAPI;
	const opened: string[] = [];
	const mode = registerPlanMode(pi, () => {}, new PortalClient(environment(port)), url => { opened.push(url); });
	t.after(() => mode.stopWatching());
	assert.ok(tool);
	const result = await tool.execute("submit", { filePath: file }, undefined, undefined, context() as ExtensionToolContext);
	const url = `http://127.0.0.1:${port}/?review=rev-1`;
	assert.deepEqual(opened, [url]);
	assert.ok(result.content.some(item => item.type === "text" && item.text.includes(url)));
});

test("a decision is delivered, acknowledged, and handed on through the approval callback", async t => {
	const p = portal();
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const mode = new PlanMode(h.plan, plan => { approved.push(plan); }, new PortalClient(environment(port)), () => undefined);
	const ctx = context();
	const approved: unknown[] = [];
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
	const mode = new PlanMode(h.plan, () => {}, new PortalClient(environment(port)), () => undefined);
	const ctx = { ...context(), isIdle: () => false } as unknown as ExtensionContext;
	await mode.decision(JSON.stringify(DECISION), ctx);
	assert.deepEqual(steered, [["Looks good.", "steer"]]);
});

test("an acknowledged decision is not delivered twice", async t => {
	const p = portal();
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const h = host();
	const mode = new PlanMode(h.plan, () => {}, new PortalClient(environment(port)), () => undefined);
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
	const mode = new PlanMode(h.plan, () => {}, new PortalClient(environment(port)), () => undefined);
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

/** A real dogsled executable that records argv and answers later. */
function fallbackEnvironment(t: { after: (fn: () => void) => void }, script: string) {
	const directory = mkdtempSync(join(tmpdir(), "dogsled-review-fallback-"));
	const calls = join(directory, "calls.json");
	const plan = join(directory, "plan.md");
	writeFileSync(plan, "# Plan");
	writeFileSync(join(directory, "dogsled"), `#!${process.execPath}\nconst fs = require("node:fs");\n${script}\nfs.writeFileSync(${JSON.stringify(calls)}, JSON.stringify(process.argv.slice(2)));`, { mode: 0o755 });
	t.after(() => rmSync(directory, { recursive: true, force: true }));
	return { env: { PATH: directory, DOGSLED_STATE: join(directory, "state") }, calls, directory, plan };
}

async function until(condition: () => boolean): Promise<void> {
	const deadline = Date.now() + 5_000;
	while (!condition() && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 10));
	assert.ok(condition(), "the background review completed");
}

test("an unreachable portal starts a review in the background and delivers the approval", async t => {
	const fake = fallbackEnvironment(t, `setTimeout(() => process.stdout.write(${JSON.stringify(JSON.stringify(DECISION))}), 200);`);
	const h = host();
	const approved: unknown[] = [];
	const mode = new PlanMode(h.plan, plan => { approved.push(plan); }, new PortalClient(fake.env), () => undefined);
	t.after(() => mode.stopWatching());
	mode.start();
	const submitted = await mode.submit(fake.plan, context());
	assert.deepEqual(submitted, { fallback: true });
	assert.deepEqual(h.sent, [], "the tool returned before the decision");
	await until(() => approved.length === 1);
	assert.deepEqual(h.sent, ["Looks good."]);
	assert.deepEqual(approved, [{ cwd: "/repo", planFilePath: "/repo/plans/plan.md", planContent: "# Plan", feedback: "Looks good." }]);
	assert.equal(mode.active, false);
	const args = JSON.parse(readFileSync(fake.calls, "utf8"));
	assert.deepEqual(args, ["review", "--file", fake.plan, "--pane", process.env["HERDR_PANE_ID"] ?? "", "--session", "session-1", "--cwd", "/repo"]);
});

test("a fallback change request steers the running agent and leaves planning on", async t => {
	const decision = { ...DECISION, decision: "changes_requested", message: "Revise the tests." };
	const fake = fallbackEnvironment(t, `process.stdout.write(${JSON.stringify(JSON.stringify(decision))});`);
	const h = host();
	const steered: unknown[] = [];
	h.plan.sendUserMessage = (message, options) => steered.push([message, options?.deliverAs]);
	const mode = new PlanMode(h.plan, () => {}, new PortalClient(fake.env), () => undefined);
	t.after(() => mode.stopWatching());
	mode.start();
	await mode.submit(fake.plan, { ...context(), isIdle: () => false } as unknown as ExtensionContext);
	await until(() => steered.length === 1);
	assert.deepEqual(steered, [["Revise the tests.", "steer"]]);
	assert.equal(mode.active, true);
});

test("closing plan mode terminates a pending review child", async t => {
	const fake = fallbackEnvironment(t, 'process.on("SIGTERM", () => { fs.writeFileSync(process.env.DOGSLED_TEST_TERMINATED, "yes"); process.exit(0); }); setInterval(() => {}, 1000);');
	const terminated = join(fake.directory, "terminated");
	const h = host();
	const mode = new PlanMode(h.plan, () => {}, new PortalClient({ ...fake.env, DOGSLED_TEST_TERMINATED: terminated }), () => undefined);
	t.after(() => mode.stopWatching());
	await mode.submit(fake.plan, context());
	await until(() => existsSync(fake.calls));
	mode.stopWatching();
	await until(() => existsSync(terminated));
	assert.deepEqual(h.sent, []);
});

test("a failed review child reports its failure as a message", async t => {
	const fake = fallbackEnvironment(t, "process.exit(2);");
	const h = host();
	const mode = new PlanMode(h.plan, () => {}, new PortalClient(fake.env), () => undefined);
	t.after(() => mode.stopWatching());
	await mode.submit(fake.plan, context());
	await until(() => h.sent.length > 0);
	assert.match(h.sent[0] ?? "", /review exited with 2/);
});

test("a portal HTTP refusal does not launch a fallback", async t => {
	const server = createServer((_request, response) => { response.writeHead(403); response.end("refused"); });
	const port = await listen(server);
	t.after(() => server.close());
	const dir = mkdtempSync(join(tmpdir(), "dogsled-plan-refused-"));
	t.after(() => rmSync(dir, { recursive: true, force: true }));
	const file = join(dir, "plan.md");
	writeFileSync(file, "# Plan");
	const mode = new PlanMode(host().plan, () => {}, new PortalClient(environment(port)), () => undefined);
	await assert.rejects(() => mode.submit(file, context()), /403 refused/);
});

test("entering planning frames the task once and offers the submit tool", async () => {
	const p = portal();
	const port = await listen(p.server);
	p.server.close();
	const h = host();
	const mode = new PlanMode(h.plan, () => {}, new PortalClient(environment(port)), () => undefined);
	mode.start();
	assert.equal(h.messages.length, 1);
	assert.match(String((h.messages[0] as { content: string }).content), /markdown/);
	assert.ok(h.active.includes("submit_plan"));
	mode.start();
	assert.equal(h.messages.length, 1);
	mode.stop();
	assert.ok(!h.active.includes("submit_plan"));
});
