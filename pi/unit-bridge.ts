import type { ExtensionAPI, ExtensionContext, ToolCallEvent, ToolCallEventResult } from "@earendil-works/pi-coding-agent";
import { acquire, execution, prepared, record, reference, repositoryIdentity, sameExecution, sameRef, unitInput, unwrap, type Execution, type Prepared } from "./unit-protocol.ts";
import { UnitWorker, type UnitExec } from "./unit-worker.ts";

interface Invocation { execution: Execution; cwd: string; prepared?: Prepared; ended: boolean }
const SPAWN = "mcp__dogsled__spawn";

/** Execution bookkeeping stays in Dogsled; the task provider owns all task state. */
export class UnitBridge {
	private readonly calls = new Map<string, Invocation>();
	private epoch = 0;
	private draining: Promise<void> | undefined;
	private readonly worker: UnitWorker;
	private readonly exec: UnitExec;
	private readonly pi: Pick<ExtensionAPI, "events" | "exec" | "getFlag" | "registerFlag">;
	constructor(pi: Pick<ExtensionAPI, "events" | "exec" | "getFlag" | "registerFlag">, binary: string) {
		this.pi = pi;
		this.exec = async (args, cwd) => {
			const result = await pi.exec(binary, args, cwd === undefined ? undefined : { cwd });
			if (result.code !== 0) throw new Error(`dogsled ${args.slice(0, 2).join(" ")} failed: ${result.stderr.trim() || result.stdout.trim() || result.code}`);
			return JSON.parse(result.stdout);
		};
		this.worker = new UnitWorker(pi, this.exec);
	}
	start(): void { this.epoch++; this.worker.start(); }
	stop(): void { this.epoch++; }
	guard(ctx: ExtensionContext): boolean { return this.worker.guard(ctx); }

	async call(event: ToolCallEvent, ctx: ExtensionContext): Promise<ToolCallEventResult | undefined> {
		if (event.toolName !== SPAWN) return;
		const input: unknown = event.input;
		if (!record(input)) return { block: true, reason: "REQUEST_CONFLICT: spawn input must be an object" };
		try {
			if ("preparedUnit" in input) {
				const owned = this.calls.get(event.toolCallId)?.prepared;
				if (!owned || Object.keys(input).length !== 1 || JSON.stringify(owned) !== JSON.stringify(input.preparedUnit)) {
					throw new Error("REQUEST_CONFLICT: preparedUnit is reserved for this invocation's mechanical handoff");
				}
				return;
			}
			if (!("unit" in input)) return;
			const request = unitInput(input), provider = acquire(this.pi.events);
			if (!provider) throw new Error("PROVIDER_UNAVAILABLE: pi-todo capability is unavailable or ambiguous");
			const contract = unwrap(provider.inspect(request.id));
			if (!record(contract) || !reference(contract.ref) || !record(contract.scope) || typeof contract.scope.repositoryId !== "string") {
				throw new Error("PROVIDER_UNAVAILABLE: invalid inspected contract");
			}
			const epoch = this.epoch, session = ctx.sessionManager.getSessionId();
			if (contract.ref.sessionId !== session) throw new Error("STALE_ORIGIN: task belongs to another session");
			const origin = await this.pi.exec("git", ["remote", "get-url", "origin"], { cwd: ctx.cwd });
			if (origin.code !== 0 || repositoryIdentity(origin.stdout) !== contract.scope.repositoryId) throw new Error("REQUEST_CONFLICT: checkout origin differs from repositoryId");
			this.current(epoch, session, ctx);
			const result = await this.exec(["unit", "prepare", "--input", JSON.stringify({ origin: contract.ref, placement: request.placement }), "--tool-call-id", event.toolCallId], ctx.cwd);
			if (!execution(result)) throw new Error("LAUNCH_UNRESOLVED: prepare returned invalid execution");
			const invocation: Invocation = { execution: result, cwd: ctx.cwd, ended: false };
			this.calls.set(event.toolCallId, invocation);
			this.current(epoch, session, ctx);
			// Reacquire after I/O so shutdown and ownership changes cannot revive old authority.
			const current = acquire(this.pi.events);
			if (!current) throw new Error("PROVIDER_UNAVAILABLE: task provider disappeared before claim");
			const claim = unwrap(current.claim({ expected: contract.ref, execution: result }));
			const enriched = record(claim) ? { execution: result, binding: claim.binding, exported: claim.exported } : null;
			if (!prepared(enriched) || !sameRef(enriched.binding.contract, contract.ref) || !sameExecution(enriched.execution, result)) throw new Error("PROVIDER_UNAVAILABLE: claim returned mismatched export");
			invocation.prepared = enriched;
			for (const key of Object.keys(input)) delete input[key];
			input.preparedUnit = enriched;
			return;
		} catch (error) {
			// This hook never committed. Its own failure proves this invocation did not launch.
			const invocation = this.calls.get(event.toolCallId);
			if (invocation) { invocation.ended = true; await this.recover(event.toolCallId, invocation, ctx); }
			return { block: true, reason: String(error) };
		}
	}

	private current(epoch: number, session: string, ctx: ExtensionContext): void {
		if (epoch !== this.epoch || session !== ctx.sessionManager.getSessionId() || ctx.signal?.aborted) throw new Error("STALE_ORIGIN: session changed or invocation aborted during handoff");
	}
	async ended(id: string, ctx: ExtensionContext): Promise<void> {
		const invocation = this.calls.get(id);
		if (!invocation) return;
		invocation.ended = true;
		await this.recover(id, invocation, ctx);
	}
	private async recover(id: string, invocation: Invocation, ctx: ExtensionContext): Promise<void> {
		try {
			const state = await this.exec(["unit", "observe", "--input", JSON.stringify({ execution: invocation.execution })], invocation.cwd);
			if (!record(state) || !execution(state.execution) || !sameExecution(state.execution, invocation.execution)) throw new Error("LAUNCH_UNRESOLVED: invalid observation");
			if (state.phase === "prepared" && [...this.calls.values()].some(other => other !== invocation && !other.ended && sameExecution(other.execution, invocation.execution))) return;
			if (state.phase === "prepared" && invocation.ended) {
				await this.exec(["unit", "abandon-prepared", "--input", JSON.stringify({ execution: invocation.execution })], invocation.cwd);
			} else if (state.phase === "prepared") return;
			this.calls.delete(id);
			await this.drain(ctx);
		} catch (error) {
			// Unknown outcome keeps the durable claim and in-memory recovery intent.
			ctx.ui.notify(`Dogsled unit recovery pending: ${String(error)}`, "warning");
		}
	}

	/** Provider settlement acknowledgements are independent of inbox text acknowledgements. */
	drain(ctx: ExtensionContext): Promise<void> {
		if (this.draining) return this.draining;
		const epoch = this.epoch;
		this.draining = (async () => {
			if (!acquire(this.pi.events)) return;
			const pending = await this.exec(["unit", "settlements"], ctx.cwd);
			if (!Array.isArray(pending)) throw new Error("LAUNCH_UNRESOLVED: settlements returned no list");
			for (const event of pending) {
				if (epoch !== this.epoch) return;
				if (!record(event) || typeof event.eventId !== "string") throw new Error("LAUNCH_UNRESOLVED: invalid settlement identity");
				const owner = record(event.binding) ? event.binding.execution : event.execution;
				if (!execution(owner)) throw new Error("LAUNCH_UNRESOLVED: missing settlement execution");
				const provider = acquire(this.pi.events);
				if (!provider) return;
				const result = provider.settle(event);
				if (!result.ok) continue; // Wrong branch or revision stays pending in Dogsled.
				if (!record(result.value) || typeof result.value.applied !== "boolean") throw new Error("PROVIDER_UNAVAILABLE: invalid settlement acknowledgement");
				await this.exec(["unit", "ack-settlement", "--input", JSON.stringify({ execution: owner, eventId: event.eventId })], ctx.cwd);
			}
		})().finally(() => { this.draining = undefined; });
		return this.draining;
	}
	async settled(ctx: ExtensionContext): Promise<void> {
		for (const [id, invocation] of this.calls) {
			if (invocation.ended) await this.recover(id, invocation, ctx);
		}
		await this.worker.settled(ctx);
		await this.drain(ctx);
	}
}
