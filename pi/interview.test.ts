import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager, createAgentSession, createEventBus, type ExtensionFactory, type ExtensionToolContext, type ToolDefinition } from "@earendil-works/pi-coding-agent";
import { InMemoryCredentialStore, InMemoryModelsStore } from "@earendil-works/pi-ai";
import { fauxProvider, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { InterviewTool, loadQuestions, registerInterview } from "./interview.ts";
import { PortalClient } from "./portal-client.ts";

interface Call { method: string; path: string; headers: Record<string, string | string[] | undefined>; body: string }

const ANSWER = { id: "q-1", responses: [{ id: "q1", value: "B" }] };

/** A portal that answers the interview routes, so the tool can be tested over real HTTP. */
function portal(answer: (poll: number) => { status: number; body?: unknown }, attachment: { name?: string; status?: number } = {}, withdrawal = 204) {
	const calls: Call[] = [];
	let polls = 0;
	const server = createServer((request, response) => {
		const chunks: Buffer[] = [];
		request.on("data", chunk => chunks.push(chunk as Buffer));
		request.on("end", () => {
			calls.push({ method: request.method ?? "", path: request.url ?? "", headers: request.headers, body: Buffer.concat(chunks).toString("utf8") });
			if (request.method === "POST" && request.url === "/api/questions") {
				response.writeHead(200, { "content-type": "application/json" });
				response.end(JSON.stringify({ id: "q-1" }));
				return;
			}
			if (request.method === "POST" && request.url === "/api/attachments") {
				const status = attachment.status ?? 200;
				response.writeHead(status, { "content-type": "application/json" });
				response.end(JSON.stringify(status === 200 ? { path: `/state/web/attachments/${attachment.name ?? "image.png"}` } : { error: "not an image" }));
				return;
			}
			if (request.method === "DELETE") {
				response.writeHead(withdrawal);
				response.end();
				return;
			}
			if (request.method === "GET" && request.url?.split("?")[0] === "/api/questions/q-1/answer") {
				polls++;
				const result = answer(polls);
				if (result.status === 0) return; // A long poll still waiting on the owner.
				response.writeHead(result.status, { "content-type": "application/json" });
				response.end(result.body === undefined ? "" : JSON.stringify(result.body));
				return;
			}
			response.writeHead(404);
			response.end();
		});
	});
	return { calls, server, polls: () => polls };
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

const questions = JSON.stringify({ title: "Choice", questions: [{ id: "q1", type: "single", question: "Which one?", options: ["A", "B"] }] });
const questionsFile = { title: "Choice", questions: [{ id: "q1", type: "single", question: "Which one?", options: ["A", "B"] }] };

/** The pane the tool reports to the portal. */
function withPane(t: { after: (fn: () => void) => void }, pane = "w1:p1"): void {
	const previous = process.env["HERDR_PANE_ID"];
	process.env["HERDR_PANE_ID"] = pane;
	t.after(() => { if (previous === undefined) delete process.env["HERDR_PANE_ID"]; else process.env["HERDR_PANE_ID"] = previous; });
}

function context(): ExtensionToolContext {
	return { cwd: "/repo", sessionManager: { getSessionId: () => "session-1" } } as unknown as ExtensionToolContext;
}

test("the interview stays in the inbox and returns its deep link without opening a tab", async t => {
	withPane(t, "pane & name");
	const p = portal(poll => poll < 2 ? { status: 204 } : { status: 200, body: ANSWER });
	const port = await listen(p.server); t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	let updates = 0;
	const result = await tool.run({ questions }, context(), undefined, update => {
		assert.equal(p.polls(), 0); assert.equal(update.details.status, "started");
		assert.match(update.details.url, /question=q-1&pane=pane%20%26%20name/);
		updates++;
	});
	assert.equal(updates, 1); assert.equal(result.details.status, "completed");
	assert.deepEqual(result.details.responses, ANSWER.responses);
	assert.match(result.content[0]?.type === "text" ? result.content[0].text : "", /Which one\?: B/);
});

test("the interview posts the questions with the bearer token", async t => {
	withPane(t);
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await tool.run({ questions }, context(), undefined);
	const post = p.calls.find(call => call.method === "POST");
	assert.ok(post);
	assert.equal(post.path, "/api/questions");
	assert.equal(post.headers["authorization"], "Bearer secret-token");
	assert.equal(post.headers["origin"], undefined);
	assert.deepEqual(JSON.parse(post.body), { questions: questionsFile, pane: "w1:p1", session: "session-1", cwd: "/repo" });
});

test("the questions are read from a file and posted as the parsed object", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const dir = mkdtempSync(join(tmpdir(), "dogsled-questions-"));
	const file = join(dir, "questions.json");
	writeFileSync(file, questions);
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await tool.run({ questions: file }, context(), undefined);
	const post = p.calls.find(call => call.method === "POST");
	assert.ok(post);
	assert.deepEqual(JSON.parse(post.body).questions, questionsFile);
});

test("a questions path is read relative to the working directory", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const dir = mkdtempSync(join(tmpdir(), "dogsled-questions-"));
	writeFileSync(join(dir, "questions.json"), questions);
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const ctx = { ...context(), cwd: dir } as unknown as ExtensionToolContext;
	await tool.run({ questions: "questions.json" }, ctx, undefined);
	const post = p.calls.find(call => call.method === "POST");
	assert.ok(post);
	assert.deepEqual(JSON.parse(post.body).questions, questionsFile);
});

test("a broken questions file is an error before any call to the portal", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await assert.rejects(() => tool.run({ questions: "/repo/missing-questions.json" }, context(), undefined), /Questions file not found/);
	await assert.rejects(() => tool.run({ questions: "{not json" }, context(), undefined), /Invalid questions JSON/);
	assert.equal(p.calls.length, 0);
});

test("an answer in another shape is an error that names the shape it expected", async t => {
	const p = portal(() => ({ status: 200, body: { answers: [{ id: "q1", value: "A" }] } }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await assert.rejects(() => tool.run({ questions }, context(), undefined), /unexpected answer shape/);
});

test("the answers are named from the questions the model sent", async t => {
	const p = portal(() => ({ status: 200, body: { id: "q-1", responses: [{ id: "q1", value: "A" }] } }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const result = await tool.run({ questions }, context(), undefined);
	assert.equal(result.details.responses[0]?.value, "A");
	const text = result.content[0]?.type === "text" ? result.content[0].text : "";
	assert.match(text, /Which one\?: A/);
});

test("an aborted interview withdraws its form", async t => {
	const p = portal(() => ({ status: 204 }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const controller = new AbortController();
	const promise = tool.run({ questions }, context(), controller.signal);
	await new Promise(resolve => setTimeout(resolve, 50));
	controller.abort();
	const result = await promise;
	assert.equal(result.details.status, "aborted");
	assert.ok(p.calls.some(call => call.method === "DELETE" && call.path === "/api/questions/q-1"), JSON.stringify(p.calls));
});

test("a withdrawn form ends gracefully", async t => {
	const p = portal(() => ({ status: 410 }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	assert.equal((await tool.run({ questions }, context(), undefined)).details.status, "cancelled");
});

test("an async interview stays in the inbox and delivers once", async t => {
	withPane(t);
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server); t.after(() => p.server.close());
	const delivered: string[] = [];
	const tool = new InterviewTool(new PortalClient(environment(port)), text => { delivered.push(text); });
	t.after(() => tool.close());
	const result = await tool.run({ questions, async: true }, context(), undefined);
	assert.equal(result.details.status, "started");
	assert.match(result.content[0]?.type === "text" ? result.content[0].text : "", /keep working and do not poll/);
	await until(() => delivered.length === 1);
	assert.match(delivered[0] ?? "", /Interview q-1 completed/);
	assert.match(delivered[0] ?? "", /Which one\?: B/);
});

async function until(condition: () => boolean): Promise<void> {
	const deadline = Date.now() + 5_000;
	while (!condition() && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 10));
	assert.ok(condition(), "the background operation completed");
}

for (const async of [false, true]) test(`an unreachable portal reports the problem without a fallback (async=${async})`, async t => {
	const dir = mkdtempSync(join(tmpdir(), "dogsled-no-fallback-"));
	t.after(() => rmSync(dir, { recursive: true, force: true }));
	const calls = join(dir, "launched");
	for (const name of ["dogsled", "open", "xdg-open"]) {
		writeFileSync(join(dir, name), `#!${process.execPath}\nrequire('node:fs').writeFileSync(${JSON.stringify(calls)}, 'launched');`, { mode: 0o755 });
	}
	const tool = new InterviewTool(new PortalClient({ PATH: dir, DOGSLED_STATE: join(dir, "missing") }));
	await assert.rejects(() => tool.run({ questions, async }, context(), undefined), /dogsled web is not running/);
	assert.equal(existsSync(calls), false);
});

for (const async of [false, true]) test(`rejection preserves the owner's reason (async=${async})`, async t => {
	const p = portal(() => ({ status: 200, body: { id: "q-1", responses: [], rejection: { reason: "Neither. Use Dioxus." } } }));
	const port = await listen(p.server); t.after(() => p.server.close());
	const delivered: string[] = [];
	const tool = new InterviewTool(new PortalClient(environment(port)), text => { delivered.push(text); });
	const result = await tool.run({ questions, async }, context(), undefined);
	if (async) { await until(() => delivered.length === 1); assert.match(delivered[0] ?? "", /Neither. Use Dioxus./); }
	else { assert.equal(result.details.status, "rejected"); assert.equal(result.details.reason, "Neither. Use Dioxus."); assert.deepEqual(result.details.responses, []); }
});

test("an explicit timeout interrupts a long poll and withdraws the unanswered form", { timeout: 5_000 }, async t => {
	const p = portal(() => ({ status: 0 }));
	const port = await listen(p.server); t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const result = await tool.run({ questions, timeout: 1 }, context(), undefined);
	assert.equal(result.details.status, "timeout");
	assert.ok(p.calls.some(call => call.method === "DELETE"));
});

for (const rejected of [false, true]) test(`cancellation preserves a racing ${rejected ? "rejection" : "answer"}`, async t => {
	let ready = false;
	const saved = rejected ? { id: "q-1", responses: [], rejection: { reason: "Dioxus" } } : ANSWER;
	const p = portal(() => ready ? { status: 200, body: saved } : { status: 204 }, {}, 409);
	const port = await listen(p.server); t.after(() => p.server.close());
	const controller = new AbortController();
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const result = tool.run({ questions }, context(), controller.signal);
	await until(() => p.polls() > 0);
	ready = true;
	controller.abort();
	const outcome = await result;
	assert.equal(outcome.details.status, rejected ? "rejected" : "completed");
	assert.equal(rejected ? outcome.details.reason : outcome.details.responses[0]?.value, rejected ? "Dioxus" : "B");
});

test("the default interview remains answerable after ten minutes", async t => {
	t.mock.timers.enable({ apis: ["Date"], now: 0 });
	const p = portal(poll => {
		if (poll === 1) { t.mock.timers.tick(601_000); return { status: 204 }; }
		return { status: 200, body: ANSWER };
	});
	const port = await listen(p.server); t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	assert.equal((await tool.run({ questions }, context(), undefined)).details.status, "completed");
	assert.equal(p.calls.some(call => call.method === "DELETE"), false);
});

test("closing a session withdraws background questions without messaging its replacement", async t => {
	const p = portal(() => ({ status: 204 }));
	const port = await listen(p.server); t.after(() => p.server.close());
	const delivered: string[] = [];
	const tool = new InterviewTool(new PortalClient(environment(port)), text => { delivered.push(text); });
	await tool.run({ questions, async: true }, context(), undefined);
	await tool.close();
	assert.ok(p.calls.some(call => call.method === "DELETE"));
	assert.deepEqual(delivered, []);
});

for (const busy of [false, true]) for (const rejected of [false, true]) test(`the real Pi runtime receives ${rejected ? "rejection" : "answers"} while ${busy ? "busy" : "idle"}`, async t => {
	let ready = false;
	const outcome = rejected ? { id: "q-1", responses: [], rejection: { reason: "Use Dioxus instead." } } : ANSWER;
	const p = portal(() => ready ? { status: 200, body: outcome } : { status: 204 });
	const port = await listen(p.server);
	const manager = SessionManager.inMemory("/checkout");
	const settings = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } });
	let holdStarted!: () => void;
	const holding = new Promise<void>(resolve => { holdStarted = resolve; });
	let releaseHold!: () => void;
	const release = new Promise<void>(resolve => { releaseHold = resolve; });
	let tool: InterviewTool | undefined;
	const extension: ExtensionFactory = pi => {
		tool = registerInterview(pi, new PortalClient(environment(port)));
		pi.registerTool({ name: "hold", label: "Hold", description: "Hold the test run", parameters: { type: "object", properties: {} } as ToolDefinition["parameters"],
			async execute() { holdStarted(); await release; return { content: [{ type: "text", text: "released" }], details: undefined }; }
		});
	};
	const loader = new DefaultResourceLoader({ cwd: "/checkout", agentDir: "/nonexistent/dogsled-test", eventBus: createEventBus(), extensionFactories: [extension],
		noContextFiles: true, noExtensions: true, noPromptTemplates: true, noSkills: true, noThemes: true, settingsManager: settings });
	const models = await ModelRuntime.create({ credentials: new InMemoryCredentialStore(), modelsStore: new InMemoryModelsStore(), modelsPath: null, refreshOnCreate: false });
	const faux = fauxProvider({ tokensPerSecond: 0 }); models.registerNativeProvider(faux.provider);
	await loader.reload();
	const { session } = await createAgentSession({ cwd: "/checkout", modelRuntime: models, model: faux.getModel(), sessionManager: manager, settingsManager: settings, resourceLoader: loader, thinkingLevel: "off" });
	t.after(async () => { releaseHold(); await tool?.close(); session.dispose(); p.server.closeAllConnections(); p.server.close(); });
	await session.bindExtensions({});
	faux.setResponses([
		fauxAssistantMessage(fauxToolCall("interview", { questions, async: true })),
		busy ? fauxAssistantMessage(fauxToolCall("hold", {})) : fauxAssistantMessage("Waiting"),
		fauxAssistantMessage("Received"),
	]);
	const running = session.prompt("Ask the test interview");
	if (busy) await holding; else await running;
	ready = true;
	// The message must queue while the original tool is still running.
	if (busy) await until(() => session.pendingMessageCount > 0);
	releaseHold();
	await running;
	const delivered = () => manager.getBranch().filter(entry => entry.type === "message" && entry.message.role === "user")
		.some(entry => JSON.stringify(entry).includes(rejected ? "Use Dioxus instead." : "Interview q-1 completed"));
	await until(delivered);
	assert.ok(delivered());
});

test("custom answers and context reach the agent in the summary and structured result", async t => {
	const responses = [
		{ id: "single", value: { option: "Other", note: "Dioxus" } },
		{ id: "multi", value: ["Rust", { option: "Other", note: "Dioxus\nKeep the UI in Rust." }] },
		{ id: "context", value: { option: "Rust", note: "Use it for the UI too." } },
	];
	const p = portal(() => ({ status: 200, body: { id: "q-1", responses } }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const form = JSON.stringify({ questions: [
		{ id: "single", type: "single", question: "Which UI?", options: ["React", "Leptos"] },
		{ id: "multi", type: "multi", question: "Which tools?", options: ["Rust", "React"] },
		{ id: "context", type: "single", question: "Which language?", options: ["Rust"] },
	] });
	const result = await tool.run({ questions: form }, context(), undefined);
	assert.deepEqual(result.details.responses, responses);
	const summary = result.content.find(item => item.type === "text");
	assert.ok(summary && summary.type === "text");
	assert.match(summary.text, /Which UI\?: Other \(Dioxus\)/);
	assert.match(summary.text, /Which tools\?: Rust, Other \(Dioxus\nKeep the UI in Rust\.\)/);
	assert.match(summary.text, /Which language\?: Rust \(Use it for the UI too\.\)/);
	assert.doesNotMatch(summary.text, /\[object Object\]/);
});

test("the questions come from inline JSON or from a file", () => {
	const inline = loadQuestions(questions, "/repo");
	assert.deepEqual(inline, questionsFile);
	const dir = mkdtempSync(join(tmpdir(), "dogsled-questions-"));
	const file = join(dir, "questions.json");
	writeFileSync(file, questions);
	assert.deepEqual(loadQuestions(file, "/repo"), inline);
	assert.deepEqual(loadQuestions(join(dir, "questions.json"), "/repo"), inline);
});

/** A form with one image whose source names a file on the agent's host. */
function mediaQuestions(src: string): string {
	return JSON.stringify({
		title: "Media",
		questions: [{ id: "q1", type: "info", question: "Look", media: { type: "image", src, alt: "A" } }],
	});
}

test("a local image is uploaded to the portal and its source becomes the portal URL", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }), { name: "shot.png" });
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const dir = mkdtempSync(join(tmpdir(), "dogsled-media-"));
	const file = join(dir, "shot.png");
	writeFileSync(file, "PNG-BYTES");
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await tool.run({ questions: mediaQuestions(file) }, context(), undefined);
	const upload = p.calls.find(call => call.method === "POST" && call.path === "/api/attachments");
	assert.ok(upload, "the image was uploaded");
	assert.equal(upload.body, "PNG-BYTES");
	assert.equal(upload.headers["authorization"], "Bearer secret-token");
	const post = p.calls.find(call => call.method === "POST" && call.path === "/api/questions");
	assert.ok(post);
	const body = JSON.parse(post.body);
	assert.equal(body.questions.questions[0].media.src, "/api/attachments/shot.png");
	assert.equal(body.questions.questions[0].media.alt, "A");
});

test("an image the browser can already load is left alone", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await tool.run({ questions: mediaQuestions("https://example.test/shot.png") }, context(), undefined);
	assert.equal(p.calls.some(call => call.path === "/api/attachments"), false, "nothing was uploaded");
	const post = p.calls.find(call => call.method === "POST" && call.path === "/api/questions");
	assert.ok(post);
	assert.equal(JSON.parse(post.body).questions.questions[0].media.src, "https://example.test/shot.png");
});

test("a missing image is an error that names the path", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const missing = join(mkdtempSync(join(tmpdir(), "dogsled-media-")), "gone.png");
	await assert.rejects(() => tool.run({ questions: mediaQuestions(missing) }, context(), undefined), new RegExp(`Image not found: ${missing.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
	assert.equal(p.calls.some(call => call.method === "POST" && call.path === "/api/questions"), false, "the form was never posted");
});

test("a portal that refuses the image is an error that names the file", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }), { status: 415 });
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const dir = mkdtempSync(join(tmpdir(), "dogsled-media-"));
	const file = join(dir, "shot.png");
	writeFileSync(file, "NOT-AN-IMAGE");
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await assert.rejects(() => tool.run({ questions: mediaQuestions(file) }, context(), undefined), /Cannot use the image/);
});

test("a missing portal reports its status before reading a local image", async t => {
	const dir = mkdtempSync(join(tmpdir(), "dogsled-offline-image-"));
	t.after(() => rmSync(dir, { recursive: true, force: true }));
	const tool = new InterviewTool(new PortalClient({ DOGSLED_STATE: dir }));
	await assert.rejects(() => tool.run({ questions: mediaQuestions(join(dir, "missing.png")) }, context(), undefined), /dogsled web is not running/);
});
