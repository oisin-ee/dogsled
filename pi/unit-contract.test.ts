import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { after, before, test } from "node:test";
import { DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager, createAgentSession, createEventBus, type ExtensionAPI, type ExtensionContext, type ExtensionFactory, type ToolDefinition } from "@earendil-works/pi-coding-agent";
import { InMemoryCredentialStore, InMemoryModelsStore } from "@earendil-works/pi-ai";
import { fauxProvider, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";
import { UnitBridge } from "./unit-bridge.ts";
import { acquire, prepared, receipt, unwrap, type Prepared } from "./unit-protocol.ts";
import { worker as makeWorker } from "./todo/src/provider-fixture.ts";

test("native turn guidance agrees across launch, MCP, and Pi without changing todo", async () => {
	const guidance = "End every turn with exactly one of: done (whole task complete), ask (you need an answer from your owner), blocked (you cannot continue), or waiting for your own open children.";
	for (const path of ["../src/done.rs", "../src/mcp.rs", "./index.ts"]) {
		const text = await readFile(new URL(path, import.meta.url), "utf8");
		assert.ok(text.includes(guidance), path);
		assert.ok(!text.includes("or run `dogsled done`"), path);
	}
	const mcp = await readFile(new URL("../src/mcp.rs", import.meta.url), "utf8");
	for (const name of ["done", "ask", "blocked"]) assert.ok(mcp.includes(`async fn ${name}(`), name);
});

// Git hooks export repository selectors; fixture subprocesses must address only their own checkouts.
const gitEnvironment = Object.entries(process.env).filter(([key]) => key.startsWith("GIT_"));
before(() => { for (const [key] of gitEnvironment) delete process.env[key]; });
after(() => { for (const [key, value] of gitEnvironment) process.env[key] = value; });
type Session = Awaited<ReturnType<typeof createAgentSession>>["session"];
type Executor = ExtensionAPI["exec"];

/** Real extension, provider, WASM core, importer, guard, runtime and tool entry points.
 * Only executor transport is simulated here; Rust tests and the authorized live scenario own launch proof. */
async function runtime(cwd: string, providerFirst: boolean, execute: Executor, options: { id?: string; seed?: string; manager?: SessionManager; spawn?: (input: unknown) => Promise<void> } = {}) {
	const events = createEventBus(), manager = options.manager ?? SessionManager.inMemory(cwd, options.id ? { id: options.id } : undefined);
	const settings = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } });
	let bridge!: UnitBridge, context!: ExtensionContext;
	const adapter: ExtensionFactory = pi => {
		bridge = new UnitBridge({ events: pi.events, registerFlag: pi.registerFlag, getFlag: pi.getFlag,
			exec: (binary, args, execOptions) => binary === "git" ? pi.exec(binary, args, execOptions) : execute(binary, args, execOptions) }, "fixture-executor");
		pi.on("session_start", (event, ctx) => { context = ctx; bridge.start(event.reason); });
		pi.on("before_agent_start", async (_event, ctx) => { if (bridge.guard(ctx)) await bridge.ready(ctx); });
		pi.on("tool_call", (event, ctx) => bridge.call(event, ctx));
		pi.on("tool_execution_end", (event, ctx) => bridge.ended(event.toolCallId, ctx));
		pi.on("agent_settled", (_event, ctx) => bridge.settled(ctx));
		pi.on("session_shutdown", () => bridge.stop());
		if (options.spawn) pi.registerTool({ name: "mcp__dogsled__spawn", label: "Spawn", description: "Protocol boundary fixture", parameters: {
			type: "object", properties: { unit: { type: "object", properties: { id: { type: "string" } }, required: ["id"] }, preparedUnit: {} },
		} as ToolDefinition["parameters"], async execute(_id, input) {
			await options.spawn!(input);
			return { content: [{ type: "text", text: "Correlated fixture result" }], details: undefined };
		} });
	};
	const loader = new DefaultResourceLoader({ cwd, agentDir: "/nonexistent/unit-contract-test", eventBus: events, settingsManager: settings,
		additionalExtensionPaths: [fileURLToPath(new URL("./todo/src/index.ts", import.meta.url))], extensionFactories: [adapter],
		extensionsOverride: loaded => ({ ...loaded, extensions: loaded.extensions.toSorted((a, b) => {
			const left = a.tools.has("todo"), right = b.tools.has("todo");
			return left === right ? 0 : left === providerFirst ? -1 : 1;
		}) }),
		noContextFiles: true, noExtensions: true, noPromptTemplates: true, noSkills: true, noThemes: true });
	const models = await ModelRuntime.create({ credentials: new InMemoryCredentialStore(), modelsStore: new InMemoryModelsStore(), modelsPath: null, refreshOnCreate: false });
	const faux = fauxProvider({ tokensPerSecond: 0 }); models.registerNativeProvider(faux.provider);
	await loader.reload();
	assert.deepEqual(loader.getExtensions().errors, []);
	const { session } = await createAgentSession({ cwd, modelRuntime: models, model: faux.getModel(), sessionManager: manager, settingsManager: settings, resourceLoader: loader, thinkingLevel: "off" });
	if (options.seed) session.extensionRunner.setFlagValue("todo-unit-file", options.seed);
	await session.bindExtensions({});
	assert.ok(acquire(events), "real pi-todo must own its tool and expose the capability");
	return { session, manager, faux, provider: () => acquire(events)!, bridge, context };
}

async function todo(session: Session, input: any): Promise<any> {
	const tool = session.extensionRunner.getToolDefinition("todo"); assert.ok(tool);
	return tool.execute("fixture-setup", input, undefined, undefined, session.extensionRunner.createToolContext("fixture-setup", new AbortController().signal));
}
function snapshot(manager: SessionManager): any {
	const entry = manager.getBranch().findLast(entry => entry.type === "custom" && entry.customType === "todo-state");
	assert.ok(entry && entry.type === "custom"); return entry.data;
}

test("actual extension loader and startup guard: merged extension", async () => {
	const cwd = await mkdtemp(join(tmpdir(), "unit-loader-")), seed = join(cwd, "seed.json"), executor = join(cwd, "executor");
	const previousBinary = process.env["DOGSLED_BIN"], previousOwner = process.env["HERDR_PANE_ID"];
	const origin = await makeWorker("github.com/fixture/work");
	const execution = origin.claimed.binding.execution;
	await writeFile(seed, JSON.stringify({ version: 1, execution, ...origin.claimed }));
	await writeFile(executor, "#!/bin/sh\nexit 0\n", { mode: 0o700 });
	execFileSync("git", ["init", "--quiet", cwd]); execFileSync("git", ["-C", cwd, "remote", "add", "origin", "git@github.com:fixture/work.git"]);
	process.env["DOGSLED_BIN"] = executor; delete process.env["HERDR_PANE_ID"];
	let session: Session | undefined;
	try {
		const paths = [fileURLToPath(new URL("./index.ts", import.meta.url))];
		const manager = SessionManager.inMemory(cwd, { id: execution.requestId }), settings = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } }), events = createEventBus();
		const loader = new DefaultResourceLoader({ cwd, agentDir: "/nonexistent/unit-loader-test", eventBus: events, settingsManager: settings, additionalExtensionPaths: paths,
			noContextFiles: true, noExtensions: true, noPromptTemplates: true, noSkills: true, noThemes: true });
		await loader.reload();
		assert.deepEqual(loader.getExtensions().errors, []);
		const extensions = loader.getExtensions().extensions;
		assert.equal(extensions.length, 1);
		assert.ok(extensions[0]!.tools.has("todo"));
		assert.ok(extensions[0]!.flags.has("todo-unit-file"));
		assert.deepEqual(loader.getExtensions().extensions.map(extension => extension.path), paths);
		const models = await ModelRuntime.create({ credentials: new InMemoryCredentialStore(), modelsStore: new InMemoryModelsStore(), modelsPath: null, refreshOnCreate: false });
		const faux = fauxProvider({ tokensPerSecond: 0 }); models.registerNativeProvider(faux.provider);
		({ session } = await createAgentSession({ cwd, modelRuntime: models, model: faux.getModel(), sessionManager: manager, settingsManager: settings, resourceLoader: loader, thinkingLevel: "off" }));
		session.extensionRunner.setFlagValue("todo-unit-file", seed);
		await session.bindExtensions({});
		const provider = acquire(events); assert.ok(provider);
		const imported = unwrap(provider.imported()); assert.ok(receipt(imported)); assert.equal(imported.digest, origin.claimed.exported.digest);
		faux.setResponses([fauxAssistantMessage("Imported task ready."), fauxAssistantMessage("Awaiting implementation.")]);
		await session.prompt("Check the loaded import.");
		assert.equal(faux.state.callCount, 2);
	} finally {
		session?.dispose(); origin.test.session.dispose();
		if (previousBinary === undefined) delete process.env["DOGSLED_BIN"]; else process.env["DOGSLED_BIN"] = previousBinary;
		if (previousOwner === undefined) delete process.env["HERDR_PANE_ID"]; else process.env["HERDR_PANE_ID"] = previousOwner;
		await rm(cwd, { recursive: true, force: true });
	}
});

test("a resumed original session settles the unchanged delivery and completes only with native done", async () => {
	const cwd = await mkdtemp(join(tmpdir(), "unit-recovery-"));
	execFileSync("git", ["init", "--quiet", cwd]);
	execFileSync("git", ["-C", cwd, "remote", "add", "origin", "git@github.com:owner/repo.git"]);
	const execution = { executor: "dogsled", ownerId: "recovery-owner", requestId: "recovery-worker" };
	const pending: unknown[] = [], acknowledged: any[] = [];
	const execute: Executor = async (_binary, args) => {
		let value: unknown = {};
		switch (args[1]) {
			case "settlements": value = pending; break;
			case "ack-settlement": acknowledged.push(JSON.parse(args[3]!)); pending.length = 0; break;
			default: throw new Error(`Unexpected fixture executor operation: ${args.join(" ")}`);
		}
		return { code: 0, stdout: JSON.stringify(value), stderr: "", killed: false };
	};
	const custody = { itemId: "", claimId: "recovery-claim", revision: 1, recipient: "recovery-session" };
	let settlement: any;
	const original = await runtime(cwd, true, execute, { manager: SessionManager.create(cwd, join(cwd, "sessions"), { id: "recovery-session" }) });
	try {
		await todo(original.session, { op: "init", items: [{ content: "Recovery task", contract: {
			deliverable: "Settle the delivery in the original session", scope: { repositoryId: "github.com/owner/repo", owns: ["src/"], reads: [], generates: [] },
			acceptance: { kind: "review", artifact: "fixture result", criterion: "reviewed" }, dependsOn: [], assignment: { kind: "worker", profile: "general" },
		} }] });
		const id = snapshot(original.manager).tasks[0].unit.reference.unitId;
		const inspected = unwrap(original.provider().inspect(id)) as any;
		const claimed = unwrap(original.provider().claim({ expected: inspected.ref, execution })) as any;
		settlement = { version: 1, eventId: "recovery-returned", binding: claimed.binding, result: { kind: "returned", evidence: { execution, resultId: "recovery-result", summary: "Fixture deliverable complete." } } };
		custody.itemId = id;
		original.faux.setResponses([fauxAssistantMessage("Original history recorded.")]);
		await original.session.prompt("Record the original history.");
		const file = original.manager.getSessionFile()!;
		original.session.dispose();
		pending.push({ version: 1, custody, settlement });
		const resumed = await runtime(cwd, true, execute, { manager: SessionManager.open(file, undefined, cwd) });
		try {
			const reopened = unwrap(resumed.provider().inspect(id)) as any;
			assert.deepEqual(reopened.ref, inspected.ref, "the resumed provider returns the complete original reference");
			await resumed.bridge.drain(resumed.context);
			assert.equal(acknowledged.length, 1, JSON.stringify(acknowledged));
			assert.deepEqual(acknowledged[0], { execution, eventId: "recovery-returned", custody }, "the acknowledgement carries the delivered custody");
			const state = snapshot(resumed.manager);
			assert.equal(state.tasks[0].status, "in_progress", "settlement alone never completes the task");
			assert.equal(state.tasks[0].unit.evidence.length, 1, "the unchanged evidence is recorded");
			assert.deepEqual(state.tasks[0].unit.evidence[0], settlement.result.evidence);
			await todo(resumed.session, { op: "done", task: id, evidence: "Original session reviewed the delivery" });
			assert.equal(snapshot(resumed.manager).tasks[0].status, "completed");
		} finally { resumed.session.dispose(); }
	} finally { await rm(cwd, { recursive: true, force: true }); }
});

for (const providerFirst of [true, false]) test(`real unit export/import/result/acceptance (providerFirst=${providerFirst})`, async () => {
	const cwd = await mkdtemp(join(tmpdir(), "unit-contract-"));
	execFileSync("git", ["init", "--quiet", cwd]);
	execFileSync("git", ["-C", cwd, "remote", "add", "origin", "git@github.com:fixture/work.git"]);
	const execution = { executor: "dogsled", ownerId: "fixture-owner", requestId: "fixture-worker-session" };
	let envelope: Prepared | undefined, origin: unknown, phase = "prepared", acknowledgements = 0, commits = 0;
	let worker: Awaited<ReturnType<typeof runtime>> | undefined;
	const pending: unknown[] = [], commands: string[][] = [], terminal: any[] = [], acknowledged: any[] = [];
	const execute: Executor = async (_binary, args) => {
		commands.push(args); let value: unknown = {};
		switch (args[1]) {
			case "prepared": value = []; break;
			case "prepare": origin = JSON.parse(args[3]!).origin; value = execution; break;
			case "observe": value = { execution, phase }; break;
			case "settlements": value = pending; break;
			case "ack-settlement": acknowledgements++; acknowledged.push(JSON.parse(args[3]!)); pending.length = 0; break;
			case "terminal-ack": terminal.push(JSON.parse(args[3]!)); break;
			default: throw new Error(`Unexpected fixture executor operation: ${args.join(" ")}`);
		}
		return { code: 0, stdout: JSON.stringify(value), stderr: "", killed: false };
	};
	const lead = await runtime(cwd, providerFirst, execute, { spawn: async input => {
		assert.ok(input && typeof input === "object" && "preparedUnit" in input && prepared(input.preparedUnit));
		envelope = input.preparedUnit; commits++; phase = "running";
		assert.deepEqual(envelope.binding.contract, origin);
		assert.deepEqual(envelope.exported.contract, unwrap(lead.provider().inspect(envelope.binding.contract.unitId)));
		assert.equal(snapshot(lead.manager).tasks[0].status, "in_progress");
		// The lead may close a held leaf with a note (pi-todo ac2e931); without one, done is refused and the unit stays open.
		await assert.rejects(todo(lead.session, { op: "done", task: envelope.binding.contract.unitId }), /ACCEPTANCE_REQUIRED/);
		const seed = join(cwd, "seed.json"); await writeFile(seed, JSON.stringify({ version: 1, ...envelope }));
		worker = await runtime(cwd, !providerFirst, execute, { id: execution.requestId, seed });
		assert.equal(worker.faux.state.callCount, 0, "automatic import needs no model setup call");
		assert.deepEqual(unwrap(worker.provider().inspect(envelope.binding.contract.unitId)), envelope.exported.contract);
		const imported = unwrap(worker.provider().imported()); assert.ok(receipt(imported)); assert.equal(imported.seal, null);
		worker.faux.setResponses([fauxAssistantMessage(fauxToolCall("todo", { op: "done", task: envelope.binding.contract.unitId, evidence: "Fixture deliverable reviewed" })), fauxAssistantMessage("Fixture deliverable complete.")]);
		await worker.session.prompt("Execute the imported contract.");
		assert.equal(worker.faux.state.callCount, 2);
		assert.equal(terminal.length, 1); assert.equal(terminal[0].execution.requestId, execution.requestId);
		const sealed = unwrap(worker.provider().imported()); assert.ok(receipt(sealed)); assert.ok(sealed.seal);
		assert.equal(terminal[0].sealEntryId, sealed.seal.entryId);
		assert.equal(terminal[0].sessionLeafId, worker.manager.getLeafId());
		await rm(seed); await worker.session.extensionRunner.emit({ type: "session_start", reason: "reload" });
		assert.deepEqual(unwrap(worker.provider().imported()), sealed);
		pending.push({ version: 1, eventId: "fixture-returned", binding: envelope.binding, result: { kind: "returned", evidence: { execution, resultId: "fixture-result", summary: "Fixture deliverable complete." } } });
		phase = "returned";
	} });
	try {
		await todo(lead.session, { op: "init", items: [{ content: "Fixture task", contract: {
			deliverable: "Prove cross-repository handoff", scope: { repositoryId: "github.com/fixture/work", owns: ["src/"], reads: [], generates: [] },
			acceptance: { kind: "review", artifact: "fixture result", criterion: "reviewed" }, dependsOn: [], assignment: { kind: "worker", profile: "general" },
		} }] });
		const id = snapshot(lead.manager).tasks[0].unit.reference.unitId;
		lead.faux.setResponses([fauxAssistantMessage(fauxToolCall("mcp__dogsled__spawn", { unit: { id } })), fauxAssistantMessage("Awaiting explicit acceptance."), fauxAssistantMessage("Ready for review.")]);
		await lead.session.prompt("Dispatch the unit once.");
		assert.equal(commits, 1, JSON.stringify(lead.session.messages.filter(message => message.role === "toolResult"))); assert.equal(commands.filter(args => args[1] === "prepare").length, 1);
		assert.equal(acknowledgements, 1, JSON.stringify(lead.session.messages.filter(message => message.role === "toolResult"))); assert.equal(snapshot(lead.manager).tasks[0].status, "in_progress");
		assert.equal(snapshot(lead.manager).tasks[0].unit.evidence.length, 1);
		const leaf = lead.manager.getLeafId();
		const custody = { itemId: envelope!.binding.contract.unitId, claimId: "fixture-claim", revision: 1, recipient: lead.manager.getSessionId() };
		pending.push({ version: 1, custody, settlement: { version: 1, eventId: "stale-returned", binding: { ...envelope!.binding, contract: { ...envelope!.binding.contract, contractEntryId: "another-branch" } }, result: { kind: "returned", evidence: { execution, resultId: "stale-result", summary: "Stale branch" } } } });
		await lead.bridge.drain(lead.context);
		assert.equal(acknowledgements, 1); assert.equal(pending.length, 1, "stale evidence remains pending");
		assert.equal(lead.manager.getLeafId(), leaf);
		pending.length = 0;
		pending.push({ version: 1, custody, settlement: { version: 1, eventId: "fixture-returned", binding: envelope!.binding, result: { kind: "returned", evidence: { execution, resultId: "fixture-result", summary: "Fixture deliverable complete." } } } });
		await lead.bridge.drain(lead.context);
		assert.equal(lead.manager.getLeafId(), leaf, "duplicate delivery appends no task mutation"); assert.equal(acknowledgements, 2);
		assert.deepEqual(acknowledged[1], { execution, eventId: "fixture-returned", custody }, "the acknowledgement carries the delivered custody");
		assert.equal(snapshot(lead.manager).tasks[0].status, "in_progress", "settlement alone never completes the task");
		assert.equal(snapshot(lead.manager).tasks[0].unit.evidence.length, 1, "a duplicate success causes no second mutation");
		await todo(lead.session, { op: "done", task: id, evidence: "Lead reviewed fixture evidence" });
		assert.equal(snapshot(lead.manager).tasks[0].status, "completed");
	} finally { worker?.session.dispose(); lead.session.dispose(); await rm(cwd, { recursive: true, force: true }); }
});
