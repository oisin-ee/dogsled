import type { ExtensionContext, ToolCallEvent } from "@earendil-works/pi-coding-agent";
import { UnitBridge } from "./unit-bridge.ts";
import type { Receipt, TaskProvider } from "./unit-protocol.ts";
import type { Seed, UnitContract } from "./todo/src/wasm.ts";

export function fixture() {
	const ref = { sessionId: "lead", unitId: "u-1", revision: 1, contractEntryId: "contract" };
	const execution = { executor: "dogsled", ownerId: "owner", requestId: "request" };
	const digest = "a".repeat(64);
	const contract: UnitContract = { ref, title: "Build", deliverable: "Deliver", scope: { repositoryId: "github.com/owner/repo", owns: ["src/"], reads: [], generates: [] }, acceptance: { kind: "review", artifact: "diff", criterion: "review" }, dependsOn: [], assignment: { kind: "worker", profile: "general" } };
	const prepared: Omit<Seed, "version"> = { execution, binding: { contract: ref, execution, exportDigest: digest }, exported: { version: 1, provider: "pi-todo", contract, prerequisites: [], brief: "provider brief", digest } };
	return { ref, execution, contract, prepared };
}
export function rig() {
	const data = fixture(), calls: string[][] = [], notices: string[] = [], settlements: unknown[] = [];
	const pending: unknown[] = [];
	let phase = "prepared", session = "lead", imported: Receipt | null = null, settleOK = true, claims = 0, abandons = 0, acks = 0, aborted = false;
	let prepareFault: "reject" | "malformed" | undefined;
	let missing = false, claimFailure = false, observeFailure = false, onPrepare: (() => void) | undefined;
	const provider: TaskProvider = {
		active: () => ({ ok: true, value: null }),
		inspect() { return { ok: true, value: data.contract }; },
		claim() { claims++; return claimFailure ? { ok: false, error: { code: "STALE_ORIGIN", message: "changed" } } : { ok: true, value: { binding: data.prepared.binding, exported: data.prepared.exported } }; },
		settle() { return settleOK ? { ok: true, value: { applied: false } } : { ok: false, error: { code: "STALE_ORIGIN", message: "other branch" } }; },
		imported() { return { ok: true, value: imported }; },
	};
	const tasks = () => missing ? undefined : provider;
	const api = { registerFlag() {}, getFlag() { return undefined; },
		async exec(binary: string, args: string[], options?: { cwd?: string }) {
			calls.push([binary, ...args]);
			if (binary === "git") return { code: 0, stdout: "git@github.com:owner/repo.git", stderr: "", killed: false };
			let value: unknown = {};
			switch (args[1]) {
				case "prepared": value = pending; break;
				case "prepare":
					if (options?.cwd !== "/checkout") throw new Error("wrong cwd");
					onPrepare?.();
					if (prepareFault) {
						pending.push({ execution: data.execution, origin: data.ref, toolCallId: args[args.indexOf("--tool-call-id") + 1] });
						if (prepareFault === "reject") throw new Error("response lost after persistence");
						return { code: 0, stdout: "truncated {", stderr: "", killed: false };
					}
					value = data.execution; break;
				case "observe": if (observeFailure) throw new Error("offline"); value = { execution: data.execution, phase }; break;
				case "abandon-prepared": if (phase !== "prepared" && phase !== "not_started") throw new Error("already committed"); phase = "not_started"; abandons++; settlements.push({ version: 1, eventId: "abandoned", origin: data.ref, execution: data.execution, result: { kind: "not_started", reason: "abandoned" } }); break;
				case "settlements": value = settlements; break;
				case "ack-settlement": acks++; settlements.length = 0; break;
			}
			return { code: 0, stdout: JSON.stringify(value), stderr: "", killed: false };
		},
	};
	const ctx = { cwd: "/checkout", signal: undefined, sessionManager: { getSessionId: () => session, getLeafId: () => "leaf", getEntries: () => [] }, abort() { aborted = true; }, ui: { notify(text: string) { notices.push(text); } } } as unknown as ExtensionContext;
	const bridge = new UnitBridge(api, "dogsled", tasks); bridge.start();
	return { ...data, api, tasks, bridge, ctx, calls, notices, provider, settlements, pending,
		state: () => ({ phase, claims, abandons, acks, aborted }),
		phase: (value: string) => { phase = value; },
		prepareFault: (value: "reject" | "malformed") => { prepareFault = value; },
		claimFailure: () => { claimFailure = true; },
		observeFailure: (value: boolean) => { observeFailure = value; },
		onPrepare: (fn: () => void) => { onPrepare = fn; },
		session: (value: string) => { session = value; },
		settleOK: (value: boolean) => { settleOK = value; },
		imported: (value: Receipt | null) => { imported = value; },
		missing: () => { missing = true; },
		event: (input: Record<string, unknown> = { unit: { id: "u-1" }, host: "local" }, id = "call"): ToolCallEvent => ({ type: "tool_call", toolName: "mcp__dogsled__spawn", toolCallId: id, input }),
	};
}
