import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { deliveredIds, InboxReader, MESSAGE_TYPE } from "./inbox.ts";

function record(id: string, text = `text ${id}`) {
	return { id, kind: "reply", text, created_ms: 1, message: `wrangle reply from w (w1:p2 on m):\n${text}`, sender: "w (w1:p2 on m)", state: "claimed" };
}

function rig(dir = mkdtempSync(join(tmpdir(), "wrangle-inbox-"))) {
	const sent: Array<{ message: any; options: any }> = [];
	const calls: string[][] = [];
	const notices: string[] = [];
	let pending: unknown[] = [];
	let idle = true;
	let claimFails = false;
	const exec = async (args: string[]) => {
		calls.push(args);
		if (args[1] === "claim") {
			if (claimFails) throw new Error("wrangle inbox failed: offline");
			const out = pending; pending = []; return JSON.stringify(out);
		}
		if (args[1] === "path") return JSON.stringify({ dir, file: "term_a.json" });
		return JSON.stringify({ acked: args.length - 2 });
	};
	const reader = new InboxReader({ sendMessage: (message: any, options?: any) => { sent.push({ message, options }); } } as any, exec);
	const ctx = { hasUI: true, isIdle: () => idle, ui: { notify: (text: string) => notices.push(text) } } as unknown as ExtensionContext;
	return { reader, ctx, sent, calls, notices, dir,
		queue: (...records: unknown[]) => { pending = records; },
		busy: () => { idle = false; }, failClaims: (v: boolean) => { claimFails = v; } };
}

const acks = (calls: string[][]) => calls.filter(c => c[1] === "ack");

test("an idle session gets one message that starts one turn; nothing is acked before Pi ends it", async () => {
	const r = rig();
	r.queue(record("m1"), record("m2"));
	await r.reader.start(r.ctx);
	assert.equal(r.sent.length, 1);
	const { message, options } = r.sent[0]!;
	assert.equal(message.customType, MESSAGE_TYPE);
	assert.equal(message.content, `${record("m1").message}\n\n${record("m2").message}`);
	assert.deepEqual(options, { triggerTurn: true });
	assert.deepEqual(acks(r.calls), []);
	await r.reader.ended({ role: "custom", ...message });
	assert.deepEqual(acks(r.calls), [["inbox", "ack", "m1", "m2"]]);
	r.reader.stop();
});

test("a running session is steered, and a record re-offered while Pi holds it is not sent twice", async () => {
	const r = rig();
	r.busy();
	r.queue(record("m1"));
	await r.reader.start(r.ctx);
	assert.deepEqual(r.sent[0]!.options, { deliverAs: "steer" });
	// The lease ran out while the steering queue held the message.
	r.queue(record("m1"), record("m3"));
	await r.reader.drain(r.ctx);
	assert.equal(r.sent.length, 2);
	assert.deepEqual(r.sent[1]!.message.details.records.map((x: any) => x.id), ["m3"]);
	// After the run settles without ending it (an abort), the next offer is sent again.
	r.reader.settled();
	r.queue(record("m1"));
	await r.reader.drain(r.ctx);
	assert.deepEqual(r.sent[2]!.message.details.records.map((x: any) => x.id), ["m1"]);
	r.reader.stop();
});

test("a change to the inbox file drains it at once; other files and other messages are ignored", async () => {
	const r = rig();
	await r.reader.start(r.ctx);
	r.queue(record("w1"));
	writeFileSync(join(r.dir, "other.json"), "[]");
	writeFileSync(join(r.dir, "term_a.json"), "[]");
	for (let i = 0; i < 50 && r.sent.length === 0; i++) await new Promise(resolve => setTimeout(resolve, 20));
	assert.deepEqual(r.sent.map(s => s.message.details.records[0].id), ["w1"]);
	assert.deepEqual(deliveredIds({ role: "user", content: "x" }), []);
	assert.deepEqual(deliveredIds({ role: "custom", customType: "other", details: { records: [{ id: "x" }] } }), []);
	await r.reader.ended({ role: "assistant" });
	assert.deepEqual(acks(r.calls), []);
	r.reader.stop();
});

test("a failing claim is reported once and delivery resumes when it recovers", async () => {
	const r = rig();
	r.failClaims(true);
	await r.reader.start(r.ctx);
	await r.reader.drain(r.ctx);
	assert.deepEqual(r.notices, ["wrangle inbox: wrangle inbox failed: offline"]);
	r.failClaims(false);
	r.queue(record("m1"));
	await r.reader.drain(r.ctx);
	assert.equal(r.sent.length, 1);
	r.reader.stop();
	r.queue(record("m2"));
	await r.reader.drain(r.ctx);
	assert.equal(r.sent.length, 1, "a stopped reader claims nothing");
});
