import { EventEmitter } from "node:events";
import type { ExtensionContext, ToolCallEvent } from "@earendil-works/pi-coding-agent";
import { UnitBridge } from "./unit-bridge.ts";
import type { Prepared, Receipt, Result } from "./unit-protocol.ts";

export function fixture() {
	const ref = { sessionId: "lead", unitId: "u-1", revision: 1, contractEntryId: "contract" };
	const execution = { executor: "dogsled", ownerId: "owner", requestId: "request" };
	const digest = "a".repeat(64);
	const contract = { ref, title: "Build", deliverable: "Deliver", scope: { repositoryId: "github.com/owner/repo", owns: ["src/"], reads: [], generates: [] }, acceptance: { kind: "review", artifact: "diff", criterion: "review" }, dependsOn: [], assignment: { kind: "worker", profile: "general" } };
	const prepared: Prepared = { execution, binding: { contract: ref, execution, exportDigest: digest }, exported: { version: 1, provider: "pi-todo", contract, prerequisites: [], brief: "provider brief", digest } };
	return { ref, execution, contract, prepared };
}
export function rig() {
	const data = fixture(), emitter = new EventEmitter(), calls: string[][] = [], notices: string[] = [], settlements: unknown[] = [];
	let phase = "prepared", session = "lead", imported: Receipt | null = null, settleOK = true, claims = 0, abandons = 0, acks = 0, aborted = false;
	let claimFailure = false, observeFailure = false, onPrepare: (() => void) | undefined;
	const bus = { emit(name: string, value: unknown) { emitter.emit(name, value); }, on(name: string, handler: (value: unknown) => void) { emitter.on(name, handler); return () => { emitter.off(name, handler); }; } };
	const provider = {
		inspect(): Result { return { ok: true, value: data.contract }; },
		claim(): Result { claims++; return claimFailure ? { ok: false, error: { code: "STALE_ORIGIN", message: "changed" } } : { ok: true, value: { binding: data.prepared.binding, exported: data.prepared.exported } }; },
		settle(): Result { return settleOK ? { ok: true, value: { applied: false } } : { ok: false, error: { code: "STALE_ORIGIN", message: "other branch" } }; },
		imported(): Result { return { ok: true, value: imported }; },
	};
	const listener = (input: any) => input.reply(provider);
	emitter.on("oisin.tasks.acquire.v1", listener);
	const api = { events: bus, registerFlag() {}, getFlag() { return undefined; },
		async exec(binary: string, args: string[], options?: { cwd?: string }) {
			calls.push([binary, ...args]);
			if (binary === "git") return { code: 0, stdout: "git@github.com:owner/repo.git", stderr: "", killed: false };
			let value: unknown = {};
			switch (args[1]) {
				case "prepare": if (options?.cwd !== "/checkout") throw new Error("wrong cwd"); onPrepare?.(); value = data.execution; break;
				case "observe": if (observeFailure) throw new Error("offline"); value = { execution: data.execution, phase }; break;
				case "abandon-prepared": if (phase !== "prepared" && phase !== "not_started") throw new Error("already committed"); phase = "not_started"; abandons++; settlements.push({ version: 1, eventId: "abandoned", origin: data.ref, execution: data.execution, result: { kind: "not_started", reason: "abandoned" } }); break;
				case "settlements": value = settlements; break;
				case "ack-settlement": acks++; settlements.length = 0; break;
			}
			return { code: 0, stdout: JSON.stringify(value), stderr: "", killed: false };
		},
	};
	const ctx = { cwd: "/checkout", signal: undefined, sessionManager: { getSessionId: () => session, getLeafId: () => "leaf" }, abort() { aborted = true; }, ui: { notify(text: string) { notices.push(text); } } } as unknown as ExtensionContext;
	const bridge = new UnitBridge(api, "dogsled"); bridge.start();
	return { ...data, api, bus, bridge, ctx, calls, notices, provider, settlements,
		state: () => ({ phase, claims, abandons, acks, aborted }),
		phase: (value: string) => { phase = value; },
		claimFailure: () => { claimFailure = true; },
		observeFailure: (value: boolean) => { observeFailure = value; },
		onPrepare: (fn: () => void) => { onPrepare = fn; },
		session: (value: string) => { session = value; },
		settleOK: (value: boolean) => { settleOK = value; },
		imported: (value: Receipt | null) => { imported = value; },
		missing: () => { emitter.off("oisin.tasks.acquire.v1", listener); },
		event: (input: Record<string, unknown> = { unit: { id: "u-1" }, host: "local" }, id = "call"): ToolCallEvent => ({ type: "tool_call", toolName: "mcp__dogsled__spawn", toolCallId: id, input }),
	};
}
