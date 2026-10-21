import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { DefaultResourceLoader, SettingsManager, createEventBus } from "@earendil-works/pi-coding-agent";
import { renderTodoLines, TodoPublisher, type TodoSnapshot } from "./work-todo.ts";
import { todoDisplay } from "./todo-fixture.test.ts";
import { herdrAvailable } from "./board.ts";

const theme = { fg: (_: string, text: string) => `\x1b[32m${text}\x1b[0m`, bold: (text: string) => text };
const host = { theme, height: () => 10_000, requestRender() {} };
function snapshot(): TodoSnapshot {
	return { version: 2, presenter: "presenter", publisher: "publisher", request: "request", session: "session", branch: "branch", revision: 1, display: todoDisplay() };
}

test("todo publisher writes only native lines, orders revisions, and clears sessions", async () => {
	const dir = await mkdtemp(join(tmpdir(), "dogsled-todo-"));
	const path = join(dir, "pane.json");
	const errors: unknown[] = [];
	const publisher = new TodoPublisher("w1:p1", error => errors.push(error));
	try {
		publisher.configure({ path, width: 48, height: 10_000 });
		let rendered = 0, disposed = 0;
		const next = snapshot();
		next.display = todoDisplay({ state: new Proxy({}, { get() { throw new Error("Do not inspect task state"); } }),
			createView: viewHost => ({ render(width) { rendered++; assert.equal(viewHost.height(), 10_000); return [`\x1b[32mwidth ${width}\x1b[0m  `, "  native indentation"]; }, dispose() { disposed++; }, update() { throw new Error("No copied state"); }, invalidate() {}, handleInput() {} }) });
		publisher.publish(next, theme);
		publisher.publish(next, theme);
		await publisher.flushed();
		assert.equal(rendered, 1); assert.equal(disposed, 1);
		const payload = JSON.parse(await readFile(path, "utf8"));
		assert.deepEqual(Object.keys(payload), ["session", "branch", "revision", "current", "pane", "width", "lines"]);
		assert.deepEqual(payload.lines, ["\x1b[32mwidth 48\x1b[0m  ", "  native indentation"]);
		publisher.configure({ path, width: 36, height: 10_000 });
		publisher.publish(next, theme);
		publisher.publish({ ...next, revision: 2, branch: "other" }, theme);
		await publisher.flushed();
		assert.equal(JSON.parse(await readFile(path, "utf8")).revision, 2);
		assert.equal(JSON.parse(await readFile(path, "utf8")).width, 36);
		await publisher.clear();
		await assert.rejects(readFile(path), { code: "ENOENT" });
		assert.deepEqual(errors, []);
	} finally { await rm(dir, { recursive: true, force: true }); }
});

test("side bridge disposes the provider view when rendering fails", () => {
	const next = snapshot();
	let disposed = false;
	next.display = todoDisplay({ createView: () => ({ render() { throw new Error("provider render failed"); }, dispose() { disposed = true; }, update() {}, invalidate() {}, handleInput() {} }) });
	assert.throws(() => renderTodoLines(next, "w1:p1", host), /provider render failed/);
	assert.equal(disposed, true);
});

test("Herdr detection requires the managed environment and a reachable socket", async () => {
	assert.equal(await herdrAvailable({}), false);
	assert.equal(await herdrAvailable({ HERDR_ENV: "0", HERDR_SOCKET_PATH: "/bad" }), false);
	assert.equal(await herdrAvailable({ HERDR_ENV: "1", HERDR_SOCKET_PATH: "/nonexistent-dogsled.sock" }), false);
});

const source = join(homedir(), ".pi/agent/npm/node_modules/@oisin-ee/oisinpi/node_modules/@oisin-ee/pi-todo/src");
test("side bridge bytes equal real pi-todo render(width), including ANSI and indentation", { skip: !existsSync(join(source, "display.ts")) }, async () => {
	// Pi's native extension loader handles installed TypeScript and its own SDK aliases.
	// Node's built-in type stripping deliberately refuses .ts under node_modules.
	const dir = await mkdtemp(join(tmpdir(), "dogsled-real-todo-"));
	const fixture = join(dir, "fixture.ts");
	const tasks = { tasks: [{ content: "Owner outcome", status: "in_progress", subtasks: [
		{ content: "Validate the real provider renderer at the side-pane width", status: "in_progress" },
		{ content: "Keep indentation and ANSI bytes", status: "completed" },
	] }] };
	const next = snapshot();
	const events = createEventBus();
	let received = false;
	const off = events.on("side-fixture", value => { next.display = value as TodoSnapshot["display"]; received = true; });
	try {
		await writeFile(fixture, `import { display } from ${JSON.stringify(join(source, "display.ts"))};\nimport { parse } from ${JSON.stringify(join(source, "wasm.ts"))};\nexport default function(pi) { pi.events.emit("side-fixture", display(parse(${JSON.stringify(tasks)}))); }\n`);
		const loader = new DefaultResourceLoader({ cwd: dir, agentDir: dir, eventBus: events, settingsManager: SettingsManager.inMemory(),
			additionalExtensionPaths: [fixture], noContextFiles: true, noExtensions: true, noPromptTemplates: true, noSkills: true, noThemes: true });
		await loader.reload();
		assert.deepEqual(loader.getExtensions().errors, []);
		assert.ok(received);
		for (const width of [24, 48, 72]) {
			const native = next.display.createView(host);
			try {
				const expected = native.render(width);
				assert.ok(expected.length > 0);
				assert.deepEqual(renderTodoLines(next, "w1:p1", host, width).lines, expected);
			} finally { native.dispose(); }
		}
	} finally { off(); await rm(dir, { recursive: true, force: true }); }
});
