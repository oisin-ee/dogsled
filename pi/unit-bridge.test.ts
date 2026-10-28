import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { rig } from "./unit-fixture.test.ts";
import { repositoryIdentity, type Result } from "./unit-protocol.ts";

/** The rig provider, with a captured settlement and a replaceable inspected reference. */
function capture(r: ReturnType<typeof rig>) {
	const target = r.provider as unknown as { inspect: (id: string) => Result; settle: (input: unknown) => Result };
	const settled: unknown[] = [], original = target.settle;
	target.settle = (input: unknown) => { settled.push(input); return original(input); };
	return {
		settled,
		inspect: (ref: unknown) => { target.inspect = () => ({ ok: true, value: { ...r.contract, ref } }); },
		settle: (implementation: (input: unknown) => Result) => { target.settle = implementation; },
	};
}
/** The last acknowledgement input the bridge sent to the server. */
function acknowledged(r: ReturnType<typeof rig>): unknown {
	const call = r.calls.filter(args => args[2] === "ack-settlement").at(-1);
	return call ? JSON.parse(call[call.indexOf("--input") + 1]!) : undefined;
}
/** One delivery envelope with its custody reference and its unchanged settlement. */
function delivery(settlement: unknown, custody: Record<string, unknown> = { itemId: "u-1", claimId: "claim-1", revision: 3, recipient: "lead" }) {
	return { version: 1, custody, settlement };
}

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

test("a delivery envelope settles the unchanged event and acknowledges its custody", async () => {
	const r = rig(), provider = capture(r);
	const custody = { itemId: "u-1", claimId: "claim-1", revision: 3, recipient: "lead" };
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push(delivery(settlement, custody));
	await r.bridge.drain(r.ctx);
	assert.equal(r.state().acks, 1);
	assert.deepEqual(provider.settled, [settlement], "the provider receives only the unchanged settlement");
	assert.deepEqual(acknowledged(r), { execution: r.execution, eventId: "returned", custody });
});

test("an unknown delivery version keeps the delivery pending", async () => {
	const r = rig();
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push({ version: 2, custody: { itemId: "u-1", claimId: "claim-1", revision: 3, recipient: "lead" }, settlement });
	await assert.rejects(r.bridge.drain(r.ctx), /LAUNCH_UNRESOLVED/);
	assert.equal(r.state().acks, 0);
	assert.equal(r.settlements.length, 1, "the delivery stays pending");
});

test("a malformed custody reference keeps the delivery pending", async () => {
	const r = rig();
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push(delivery(settlement, { itemId: "u-1", revision: 3, recipient: "lead" }));
	await assert.rejects(r.bridge.drain(r.ctx), /LAUNCH_UNRESOLVED/);
	assert.equal(r.state().acks, 0);
	assert.equal(r.settlements.length, 1, "the delivery stays pending");
});

test("a delivery for another session stays pending", async () => {
	const r = rig(), provider = capture(r);
	const settlement = { version: 1, eventId: "returned", binding: { ...r.prepared.binding, contract: { ...r.ref, sessionId: "other-session" } }, result: { kind: "returned" } };
	r.settlements.push(delivery(settlement));
	await r.bridge.drain(r.ctx);
	assert.equal(r.state().acks, 0);
	assert.deepEqual(provider.settled, [], "another session never reaches the provider");
	assert.equal(r.settlements.length, 1, "the delivery stays pending");
});

test("a changed origin keeps the delivery pending", async () => {
	const r = rig(), provider = capture(r);
	provider.inspect({ ...r.ref, contractEntryId: "another-branch" });
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push(delivery(settlement));
	await r.bridge.drain(r.ctx);
	assert.equal(r.state().acks, 0);
	assert.deepEqual(provider.settled, [], "another branch never reaches the provider");
	assert.equal(r.settlements.length, 1, "the delivery stays pending");
});

test("a provider rejection keeps the delivery pending", async () => {
	const r = rig(), provider = capture(r);
	r.settleOK(false);
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push(delivery(settlement));
	await r.bridge.drain(r.ctx);
	assert.equal(r.state().acks, 0);
	assert.equal(provider.settled.length, 1);
	assert.equal(r.settlements.length, 1, "the delivery stays pending");
});

test("a runtime replacement during the delivery keeps the delivery pending", async () => {
	const r = rig(), provider = capture(r);
	provider.settle(() => { r.bridge.stop(); return { ok: true, value: { applied: true } }; });
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push(delivery(settlement));
	await assert.rejects(r.bridge.drain(r.ctx), /STALE_ORIGIN/);
	assert.equal(r.state().acks, 0);
	assert.equal(r.settlements.length, 1, "the delivery stays pending");
});

test("a duplicate provider success is acknowledged without a second mutation", async () => {
	const r = rig(), provider = capture(r);
	provider.settle(() => ({ ok: true, value: { applied: false } }));
	const custody = { itemId: "u-1", claimId: "claim-1", revision: 3, recipient: "lead" };
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push(delivery(settlement, custody));
	await r.bridge.drain(r.ctx);
	assert.equal(r.state().acks, 1);
	assert.deepEqual(acknowledged(r), { execution: r.execution, eventId: "returned", custody });
	assert.equal(r.settlements.length, 0, "the acknowledged delivery leaves the pending list");
});

test("the un-enveloped settlement form still settles and acknowledges without custody", async () => {
	const r = rig(), provider = capture(r);
	const settlement = { version: 1, eventId: "returned", binding: r.prepared.binding, result: { kind: "returned" } };
	r.settlements.push(settlement);
	await r.bridge.drain(r.ctx);
	assert.equal(r.state().acks, 1);
	assert.deepEqual(provider.settled, [settlement], "the provider receives the event unchanged");
	assert.deepEqual(acknowledged(r), { execution: r.execution, eventId: "returned" });
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

for (const fault of ["reject", "malformed"] as const) test(`lost prepare response (${fault}) recovers only after actual invocation end`, async () => {
	const r = rig(); r.prepareFault(fault);
	assert.equal((await r.bridge.call(r.event(), r.ctx))?.block, true);
	assert.equal(r.pending.length, 1); assert.equal(r.state().claims, 0);
	await r.bridge.settled(r.ctx); await r.bridge.ready(r.ctx);
	assert.equal(r.state().abandons, 0, "unknown in-flight preparation remains held");
	r.observeFailure(true); await r.bridge.ended("call", r.ctx);
	assert.equal(r.state().abandons, 0, "failed observation retains recovery intent");
	r.observeFailure(false); await r.bridge.settled(r.ctx);
	assert.equal(r.state().abandons, 1); assert.equal(r.state().acks, 1);
	assert.equal(r.calls.filter(args => args[2] === "prepare").length, 1);
	assert.equal(r.calls.some(args => args[2] === "commit"), false);
});

test("SSH and HTTPS origins map to the user-selected repository identity", () => {
	for (const origin of ["git@github.com:owner/repo.git", "https://github.com/owner/repo.git", "ssh://git@github.com/owner/repo"]) assert.equal(repositoryIdentity(origin), "github.com/owner/repo");
	assert.throws(() => repositoryIdentity("/tmp/repo"));
});

test("repository identity matches the shared fixture", () => {
	const cases: { origin: string; identity: string | null }[] = JSON.parse(readFileSync(new URL("../tests/fixtures/repository-identity.json", import.meta.url), "utf8"));
	assert.ok(cases.length > 0);
	for (const { origin, identity } of cases) {
		if (identity === null) assert.throws(() => repositoryIdentity(origin), /REQUEST_CONFLICT|Invalid URL/, JSON.stringify(origin));
		else assert.equal(repositoryIdentity(origin), identity, JSON.stringify(origin));
	}
});
