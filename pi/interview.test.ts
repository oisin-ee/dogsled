import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { InterviewTool, loadQuestions } from "./interview.ts";
import { PortalClient } from "./portal-client.ts";

interface Call { method: string; path: string; headers: Record<string, string | string[] | undefined>; body: string }

const ANSWER = { id: "q-1", responses: [{ id: "q1", value: "B" }] };

/** A portal that answers the interview routes, so the tool can be tested over real HTTP. */
function portal(answer: (poll: number) => { status: number; body?: unknown }) {
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
			if (request.method === "DELETE") {
				response.writeHead(204);
				response.end();
				return;
			}
			if (request.method === "GET" && request.url === "/api/questions/q-1/answer") {
				polls++;
				const result = answer(polls);
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
function withPane(t: { after: (fn: () => void) => void }): void {
	const previous = process.env["HERDR_PANE_ID"];
	process.env["HERDR_PANE_ID"] = "w1:p1";
	t.after(() => { if (previous === undefined) delete process.env["HERDR_PANE_ID"]; else process.env["HERDR_PANE_ID"] = previous; });
}

function context(): ExtensionToolContext {
	return { cwd: "/repo", sessionManager: { getSessionId: () => "session-1" } } as unknown as ExtensionToolContext;
}

test("the interview polls until the portal has an answer", async t => {
	const p = portal(poll => poll < 2 ? { status: 204 } : { status: 200, body: ANSWER });
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	const result = await tool.run({ questions }, context(), undefined);
	assert.equal(result.details.status, "completed");
	assert.equal(result.details.responses[0]?.value, "B");
	const text = result.content[0]?.type === "text" ? result.content[0].text : "";
	assert.match(text, /User completed the interview form/);
	assert.match(text, /Which one\?: B/);
	assert.ok(p.calls.some(call => call.method === "GET" && call.path === "/api/questions/q-1/answer"));
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

test("a withdrawn form is an error", async t => {
	const p = portal(() => ({ status: 410 }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await assert.rejects(() => tool.run({ questions }, context(), undefined), /withdrawn/);
});

test("an async interview returns at once and delivers the answers as a message", async t => {
	const p = portal(() => ({ status: 200, body: ANSWER }));
	const port = await listen(p.server);
	t.after(() => p.server.close());
	const delivered: string[] = [];
	const tool = new InterviewTool(new PortalClient(environment(port)), content => { delivered.push(content); });
	const result = await tool.run({ questions, async: true }, context(), undefined);
	assert.equal(result.details.status, "started");
	assert.match(String(result.content[0]?.type === "text" ? result.content[0].text : ""), /keep working and do not poll/);
	const deadline = Date.now() + 5_000;
	while (!delivered.length && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 20));
	assert.equal(delivered.length, 1);
	assert.match(delivered[0] ?? "", /Interview q-1 finished/);
	assert.match(delivered[0] ?? "", /Which one\?: B/);
});

test("an unreachable portal names the server that is missing", async () => {
	const p = portal(() => ({ status: 204 }));
	const port = await listen(p.server);
	await p.server.close();
	const tool = new InterviewTool(new PortalClient(environment(port)));
	await assert.rejects(() => tool.run({ questions }, context(), undefined), /dogsled web is not running/);
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
