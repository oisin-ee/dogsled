import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager, createAgentSession, createCodemodeExtension, createEventBus, type ExtensionFactory, type ToolDefinition } from "@earendil-works/pi-coding-agent";
import { InMemoryCredentialStore, InMemoryModelsStore } from "@earendil-works/pi-ai";
import { fauxProvider, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { UnitBridge } from "./unit-bridge.ts";
import { UnitWorker } from "./unit-worker.ts";
import { fixture, rig } from "./unit-fixture.test.ts";

async function runtime(factories: ExtensionFactory[]) {
	const manager = SessionManager.inMemory("/checkout");
	const settings = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } });
	const loader = new DefaultResourceLoader({ cwd: "/checkout", agentDir: "/nonexistent/dogsled-test", eventBus: createEventBus(), extensionFactories: factories,
		noContextFiles: true, noExtensions: true, noPromptTemplates: true, noSkills: true, noThemes: true, settingsManager: settings });
	const models = await ModelRuntime.create({ credentials: new InMemoryCredentialStore(), modelsStore: new InMemoryModelsStore(), modelsPath: null, refreshOnCreate: false });
	const faux = fauxProvider({ tokensPerSecond: 0 }); models.registerNativeProvider(faux.provider);
	await loader.reload();
	const { session } = await createAgentSession({ cwd: "/checkout", modelRuntime: models, model: faux.getModel(), sessionManager: manager, settingsManager: settings, resourceLoader: loader, thinkingLevel: "off" });
	return { manager, session, faux };
}

for (const nested of [false, true]) for (const abort of [false, true]) test(`actual ${nested ? "nested codemode" : "direct"} ${abort ? "aborted" : "blocked"} invocation recovers without tool_result`, async () => {
	const r = rig(); let bridge: UnitBridge | undefined; let executed = 0; const ends: string[] = [], results: string[] = [];
	const extension: ExtensionFactory = pi => {
		pi.events.on("oisin.tasks.acquire.v1", (value: any) => value.reply(r.provider));
		bridge = new UnitBridge({ events: pi.events, exec: r.api.exec, getFlag: pi.getFlag, registerFlag: pi.registerFlag }, "dogsled");
		pi.on("session_start", () => bridge!.start());
		pi.on("tool_call", (event, ctx) => bridge!.call(event, ctx));
		pi.on("tool_execution_end", async (event, ctx) => { ends.push(event.toolCallId); await bridge!.ended(event.toolCallId, ctx); });
		pi.on("tool_result", event => { results.push(event.toolCallId); });
		pi.registerTool({ name: "mcp__dogsled__spawn", label: "Spawn", description: "Test spawn execution", parameters: { type: "object", properties: { unit: { type: "object", properties: { id: { type: "string" } }, required: ["id"] }, preparedUnit: {} } } as ToolDefinition["parameters"],
			async execute() { executed++; return { content: [{ type: "text", text: "launched" }], details: undefined }; } });
	};
	const later: ExtensionFactory = pi => { pi.on("tool_call", (event, ctx) => {
		if (event.toolName !== "mcp__dogsled__spawn") return;
		if (abort) ctx.abort();
		return { block: true, reason: "later guard" };
	}); };
	const testRuntime = await runtime([createCodemodeExtension(), extension, later]);
	r.ref.sessionId = testRuntime.manager.getSessionId();
	try {
		await testRuntime.session.bindExtensions({});
		testRuntime.session.setActiveToolsByName([...testRuntime.session.getActiveToolNames(), "codemode"]);
		testRuntime.faux.setResponses([fauxAssistantMessage(nested ? fauxToolCall("codemode", { code: 'return await tools.mcp__dogsled__spawn({unit:{id:"u-1"}})' }) : fauxToolCall("mcp__dogsled__spawn", { unit: { id: "u-1" } })), fauxAssistantMessage("Done")]);
		await testRuntime.session.prompt("Exercise spawn");
		assert.equal(executed, 0); assert.equal(r.state().claims, 1); assert.equal(r.state().abandons, 1);
		const prepare = r.calls.find(args => args[2] === "prepare"); assert.ok(prepare);
		const id = prepare[prepare.indexOf("--tool-call-id") + 1]; assert.ok(id); assert.ok(ends.includes(id));
		assert.equal(results.includes(id), false, "later block can omit tool_result");
		if (nested) assert.ok(id.includes("/"));
	} finally { testRuntime.session.dispose(); }
});

for (const mode of ["missing", "invalid", "valid", "sealed"] as const) test(`fresh loaded worker guard: ${mode} acknowledgement`, async () => {
	const data = fixture(), dir = await mkdtemp(join(tmpdir(), "dogsled-guard-")), path = join(dir, "seed.json");
	const acknowledgements: string[][] = [];
	const guard: ExtensionFactory = pi => {
		const worker = new UnitWorker(pi, async args => { acknowledgements.push(args); return {}; });
		pi.on("session_start", () => worker.start());
		pi.on("before_agent_start", (_event, ctx) => { worker.guard(ctx); });
	};
	// Provider registers after the guard. Initial-request validation must not depend on load order.
	const provider: ExtensionFactory = pi => {
		if (mode === "missing") return;
		pi.events.on("oisin.tasks.acquire.v1", (request: any) => request.reply({ inspect() {}, claim() {}, settle() {}, imported: () => ({ ok: true, value: {
			version: 1, execution: data.execution, unitRef: data.ref, digest: mode === "invalid" ? "b".repeat(64) : data.prepared.exported.digest,
			localSessionId: data.execution.requestId, seal: mode === "sealed" ? { entryId: "seal", disposition: "completed" } : null,
		} }) }));
	};
	const r = await runtime([guard, provider]);
	data.execution.requestId = r.manager.getSessionId();
	await writeFile(path, JSON.stringify({ version: 1, ...data.prepared }));
	try {
		r.session.extensionRunner.setFlagValue("todo-unit-file", path);
		await r.session.bindExtensions({});
		r.faux.setResponses([fauxAssistantMessage("Done")]);
		await r.session.prompt("Initial worker task");
		assert.equal(r.faux.state.callCount, mode === "valid" ? 1 : 0);
		assert.deepEqual(acknowledgements, []);
	} finally { r.session.dispose(); await rm(dir, { recursive: true, force: true }); }
});
