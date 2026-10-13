import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import wrangle from "./index.ts";

type Handler = (event: { systemPrompt: string }) => Promise<{ systemPrompt: string }>;

function rig(code: number, stdout: string, stderr = "") {
	let handler: Handler | undefined;
	const calls: unknown[] = [];
	const api = {
		on(event: string, fn: Handler) {
			assert.equal(event, "before_agent_start");
			handler = fn;
		},
		async exec(binary: string, args: string[]) {
			calls.push([binary, args]);
			return { code, stdout, stderr, killed: false };
		},
	};
	// The harness exposes only the APIs this context-only extension can use.
	wrangle(api as unknown as ExtensionAPI);
	return {
		calls,
		invoke: async (systemPrompt: string) => {
			assert.ok(handler);
			return handler({ systemPrompt });
		},
	};
}

test("adds fresh brief status to each prompt without tools or widgets", async () => {
	const r = rig(0, "local 3/8 mem 1/8GiB disk 50% free agents 2\nRun `wrangle status` for details.\n");
	assert.equal(r.calls.length, 0);
	const first = await r.invoke("base prompt");
	assert.match(first.systemPrompt, /^base prompt\n\n## Host status\nlocal 3\/8/);
	const second = await r.invoke("next prompt");
	assert.ok(second.systemPrompt.startsWith("next prompt\n"));
	assert.deepEqual(r.calls, Array(2).fill([process.env["WRANGLE_BIN"] ?? "wrangle", ["status", "--brief"]]));
});

test("surfaces status errors rather than injecting an empty or invented status", async () => {
	await assert.rejects(rig(2, "", "ssh configuration error").invoke("base"), /ssh configuration error/);
});
