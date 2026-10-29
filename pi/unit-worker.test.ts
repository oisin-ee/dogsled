import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { createEventBus, type ExtensionContext } from "@earendil-works/pi-coding-agent";
import { UnitWorker } from "./unit-worker.ts";
import { rig } from "./unit-fixture.test.ts";

test("only a correlated seal acknowledges the current final leaf; repeated acknowledgement is mechanical", async () => {
	const r = rig(), dir = await mkdtemp(join(tmpdir(), "unit-worker-")), path = join(dir, "seed.json"), calls: string[][] = [];
	await writeFile(path, JSON.stringify({ version: 1, ...r.prepared }));
	const worker = new UnitWorker({ ...r.api, getFlag: () => path }, async args => { calls.push(args); return {}; });
	r.session("request"); worker.start();
	const receipt = { version: 1, execution: r.execution, unitRef: r.ref, digest: r.prepared.exported.digest, localSessionId: "request", seal: null };
	try {
		r.imported(receipt); assert.equal(worker.guard(r.ctx), true); await worker.settled(r.ctx); assert.equal(calls.length, 0);
		r.imported({ ...receipt, seal: { entryId: "seal-entry", disposition: "completed" } });
		assert.equal(worker.guard(r.ctx), false); assert.equal(r.state().aborted, true);
		await worker.settled(r.ctx); await worker.settled(r.ctx);
		assert.equal(calls.length, 2);
		assert.deepEqual(JSON.parse(calls[0]![3]!), { execution: r.execution, unitRef: r.ref, digest: r.prepared.exported.digest, sealEntryId: "seal-entry", sessionLeafId: "leaf" });
	} finally { await rm(dir, { recursive: true, force: true }); }
});

for (const flagged of [true, false]) test(`recorded import survives reload without launch artifact (flag=${flagged})`, async () => {
	const r = rig(), calls: string[][] = [];
	const worker = new UnitWorker({ ...r.api, getFlag: () => flagged ? "/nonexistent/seed" : undefined }, async args => { calls.push(args); return {}; });
	r.session("request");
	r.imported({ version: 1, execution: r.execution, unitRef: r.ref, digest: r.prepared.exported.digest, localSessionId: "request", seal: { entryId: "seal", disposition: "abandoned" } });
	worker.start("reload");
	assert.equal(worker.guard(r.ctx), false, "persisted seal blocks another task even without launch flags");
	await worker.settled(r.ctx); assert.equal(calls.length, 1);
	r.session("replacement"); worker.start("new");
	await assert.rejects(worker.settled(r.ctx), /mismatched import/);
	assert.equal(calls.length, 1, "another session cannot acknowledge the imported execution");
});

test("managed startup without a task provider aborts before work", () => {
	let aborted = 0;
	const notices: unknown[][] = [];
	const worker = new UnitWorker({ events: createEventBus(), getFlag: () => "/nonexistent/seed.json", registerFlag() {} }, async () => ({}));
	const ctx = { abort() { aborted++; }, ui: { notify(...args: unknown[]) { notices.push(args); } } } as unknown as ExtensionContext;
	worker.start("startup");
	assert.equal(worker.guard(ctx), false);
	assert.equal(aborted, 1);
	assert.equal(notices.length, 1);
	assert.match(String(notices[0]![0]), /IMPORT_FAILED:.*task provider unavailable/);
	assert.equal(notices[0]![1], "error");
});

test("guard aborts unreadable or changed correlation before work", async () => {
	const r = rig(); const worker = new UnitWorker({ ...r.api, getFlag: () => "/nonexistent/seed" }, async () => ({}));
	worker.start(); assert.equal(worker.guard(r.ctx), false); assert.equal(r.state().aborted, true);
});
