import assert from "node:assert/strict";
import { test } from "node:test";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { Observation, parseFrame, type Observed, type Watcher } from "./observation.ts";

const board = { hosts: [], rows: [] };
const history = { jobs: [], hosts: [{ id: "local", label: "local" }] };
const line = (revision: number, extra: Record<string, unknown> = { board, history }) => `${JSON.stringify({ revision, ...extra })}\n`;

class FakeWatcher extends EventEmitter implements Watcher {
	stdout = new PassThrough();
	stderr = new PassThrough();
	killed = false;
	kill() { this.killed = true; return true; }
	/** End the process after its output, as a child process does. */
	exit(code: number, stderr = "") {
		if (stderr) this.stderr.write(stderr);
		this.stdout.end();
		this.stdout.once("close", () => setImmediate(() => this.emit("close", code)));
	}
}

function rig() {
	const watchers: FakeWatcher[] = [];
	const spawned: string[][] = [];
	const observed: Observed[] = [];
	let notify: () => void = () => {};
	const observation = new Observation("dogsled", "w1:p1", value => { observed.push(value); notify(); }, (binary, args) => {
		spawned.push([binary, ...args]);
		const watcher = new FakeWatcher();
		watchers.push(watcher);
		return watcher;
	});
	const count = (n: number) => new Promise<void>(resolve => {
		const check = () => { if (observed.length >= n) resolve(); else notify = check; };
		check();
	});
	return { observation, watchers, spawned, observed, count };
}

test("frames parse into a board and history; unpublished domains are explicit errors", () => {
	assert.deepEqual(parseFrame(line(3).trim()), { frame: { revision: 3, board, history } });
	assert.deepEqual(parseFrame(line(4, { error: "observation unavailable: board\u001b" }).trim()), { error: "observation unavailable: board " });
	for (const invalid of ["not json", "null", "{}", JSON.stringify({ revision: 1, board: {} , history })]) {
		assert.ok(parseFrame(invalid).error, invalid);
	}
	assert.match(parseFrame(JSON.stringify({ revision: 1, board, history: {} })).error ?? "", /job history/);
});

test("one subscription per owner, forward revisions only, and no resubscription while live", async () => {
	const r = rig();
	r.observation.start();
	r.observation.start();
	assert.deepEqual(r.spawned, [["dogsled", "board", "--watch", "--json", "--owner", "w1:p1"]]);
	const watcher = r.watchers[0]!;
	watcher.stdout.write(line(2));
	watcher.stdout.write(line(1));
	watcher.stdout.write(line(2));
	watcher.stdout.write(line(3, { error: "observation unavailable: boards" }));
	await r.count(2);
	assert.deepEqual(r.observed.map(value => value.frame?.revision ?? value.error), [2, "observation unavailable: boards"]);
	r.observation.stop();
	assert.equal(watcher.killed, true);
});

test("a stopped subscription ignores its late frames and exit", async () => {
	const r = rig();
	r.observation.start();
	const old = r.watchers[0]!;
	r.observation.stop();
	old.stdout.write(line(9));
	old.exit(1, "late");
	await new Promise<void>(resolve => old.once("close", () => resolve()));
	assert.deepEqual(r.observed, []);
	assert.equal(r.observation.running, false);
});

test("an ended subscription reports why, delivers its last frames, and can subscribe again", async () => {
	const r = rig();
	r.observation.start();
	const first = r.watchers[0]!;
	first.stdout.write(line(5));
	first.exit(1, "observation disconnected: publisher stopped\n");
	await r.count(2);
	assert.equal(r.observed[0]?.frame?.revision, 5);
	assert.equal(r.observed[1]?.error, "Dogsled observation unavailable: observation disconnected: publisher stopped");
	assert.equal(r.observation.running, false);
	r.observation.start();
	assert.equal(r.spawned.length, 2);
	// A new subscription may start from a restarted publisher's lower revision.
	r.watchers[1]!.stdout.write(line(1));
	await r.count(3);
	assert.equal(r.observed[2]?.frame?.revision, 1);
	r.observation.stop();
});

test("a watcher that cannot start is reported, not thrown", () => {
	const observed: Observed[] = [];
	const observation = new Observation("missing", "w1:p1", value => observed.push(value), () => { throw new Error("ENOENT"); });
	observation.start();
	assert.deepEqual(observed, [{ error: "Dogsled observation unavailable: Error: ENOENT" }]);
	assert.equal(observation.running, false);
});
