import assert from "node:assert/strict";
import { test } from "node:test";
import { rig } from "./unit-fixture.test.ts";
import { repositoryIdentity } from "./unit-protocol.ts";

test("one spawn prepares, claims and enriches exact bytes but never commits in the hook", async () => {
	const r = rig(), event = r.event();
	assert.equal(await r.bridge.call(event, r.ctx), undefined);
	assert.deepEqual(event.input, { preparedUnit: r.prepared });
	assert.equal(r.state().claims, 1);
	assert.equal(r.calls.some(args => args[1] === "unit" && args[2] === "commit"), false);
	assert.ok(r.calls.some(args => args.includes("--tool-call-id") && args.includes("call")));
	r.phase("running"); await r.bridge.ended("call", r.ctx);
	assert.equal(r.state().abandons, 0);
});

test("later-hook block without tool_result abandons only on actual execution end", async () => {
	const r = rig(), event = r.event(undefined, "outer/1");
	await r.bridge.call(event, r.ctx);
	await r.bridge.settled(r.ctx);
	assert.equal(r.state().abandons, 0, "settled alone does not prove invocation termination");
	await r.bridge.ended("outer", r.ctx);
	assert.equal(r.state().abandons, 0, "wrapper id cannot release nested call");
	await r.bridge.ended("outer/1", r.ctx);
	assert.equal(r.state().abandons, 1);
	assert.equal(r.state().acks, 1);
});

test("claim failure rolls back durable preparation without launch", async () => {
	const r = rig(); r.claimFailure();
	assert.equal((await r.bridge.call(r.event(), r.ctx))?.block, true);
	assert.equal(r.state().abandons, 1);
});

test("shutdown during prepare never claims against replacement runtime", async () => {
	const r = rig(); r.onPrepare(() => { r.bridge.stop(); r.session("replacement"); });
	assert.equal((await r.bridge.call(r.event(), r.ctx))?.block, true);
	assert.equal(r.state().claims, 0);
	assert.equal(r.state().abandons, 1);
});

test("uncertain outcome retains recovery intent and retries at settlement", async () => {
	const r = rig(); await r.bridge.call(r.event(), r.ctx); r.observeFailure(true);
	await r.bridge.ended("call", r.ctx);
	assert.equal(r.state().abandons, 0);
	r.observeFailure(false); await r.bridge.settled(r.ctx);
	assert.equal(r.state().abandons, 1);
});

test("an ended duplicate invocation cannot abandon another live invocation", async () => {
	const r = rig(); await r.bridge.call(r.event(undefined, "one"), r.ctx); await r.bridge.call(r.event(undefined, "two"), r.ctx);
	await r.bridge.ended("one", r.ctx); assert.equal(r.state().abandons, 0);
	r.phase("running"); await r.bridge.ended("two", r.ctx); await r.bridge.settled(r.ctx);
	assert.equal(r.state().abandons, 0);
});

test("prepared envelopes cannot be pasted into a new model call", async () => {
	const r = rig(); assert.equal((await r.bridge.call(r.event({ preparedUnit: r.prepared }), r.ctx))?.block, true);
	assert.equal(r.calls.length, 0);
	assert.equal((await r.bridge.call(r.event({ unit: { id: "u-1" }, task: "override" }), r.ctx))?.block, true);
	assert.equal(r.calls.length, 0);
});

test("legacy spawn stays unchanged and missing capability does not prepare", async () => {
	const r = rig(), legacy = r.event({ name: "old", task: "old brief" });
	assert.equal(await r.bridge.call(legacy, r.ctx), undefined); assert.equal(r.calls.length, 0);
	r.missing(); assert.equal((await r.bridge.call(r.event(), r.ctx))?.block, true); assert.equal(r.calls.length, 0);
});

test("settlement stays pending on the wrong branch and duplicate success is acknowledged without a model turn", async () => {
	const r = rig(); r.settlements.push({ version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } });
	r.settleOK(false); await r.bridge.drain(r.ctx); assert.equal(r.state().acks, 0);
	r.settleOK(true); await r.bridge.drain(r.ctx); assert.equal(r.state().acks, 1);
});

test("fresh runtime recovers current-session prepared requests without redispatch or model setup", async () => {
	const r = rig();
	r.pending.push({ execution: r.execution, origin: r.ref, toolCallId: "lost-call" });
	r.pending.push({ execution: { ...r.execution, requestId: "other" }, origin: { ...r.ref, sessionId: "other-session" }, toolCallId: "other-call" });
	await r.bridge.resume(r.ctx);
	assert.equal(r.state().abandons, 1); assert.equal(r.state().claims, 0); assert.equal(r.state().acks, 1);
	assert.equal(r.calls.some(args => args[2] === "prepare" || args[2] === "commit"), false);
});

test("runtime-boundary recovery excludes an invocation still known to execute", async () => {
	const r = rig(); await r.bridge.call(r.event(), r.ctx);
	r.pending.push({ execution: r.execution, origin: r.ref, toolCallId: "call" });
	await r.bridge.resume(r.ctx); assert.equal(r.state().abandons, 0);
	await r.bridge.ended("call", r.ctx); assert.equal(r.state().abandons, 1);
});

test("SSH and HTTPS origins map to the user-selected repository identity", () => {
	for (const origin of ["git@github.com:owner/repo.git", "https://github.com/owner/repo.git", "ssh://git@github.com/owner/repo"]) assert.equal(repositoryIdentity(origin), "github.com/owner/repo");
	assert.throws(() => repositoryIdentity("/tmp/repo"));
});
