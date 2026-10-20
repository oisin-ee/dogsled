import { closeSync, constants, fstatSync, openSync, readFileSync } from "node:fs";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { acquire, prepared, receipt, record, sameExecution, sameRef, unwrap, type Prepared, type Receipt } from "./unit-protocol.ts";

/** Read transport correlation, not task semantics; pi-todo validates the actual contract. */
function seedFile(path: string): Prepared {
	const fd = openSync(path, constants.O_RDONLY | constants.O_NONBLOCK);
	try {
		const stat = fstatSync(fd);
		if (!stat.isFile() || stat.size > 1_048_576) throw new Error("invalid seed artifact");
		const bytes = readFileSync(fd);
		if (bytes.length > 1_048_576) throw new Error("seed artifact too large");
		const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
		if (!record(value) || value.version !== 1 || !prepared(value)) throw new Error("invalid seed correlation");
		return value;
	} finally { closeSync(fd); }
}

export type UnitExec = (args: string[], cwd?: string) => Promise<unknown>;

/** The loaded guard enforces fresh-session startup, not resume-time compaction. */
export class UnitWorker {
	private seed: Prepared | undefined;
	private failure: string | undefined;
	private managed = false;
	private readonly pi: Pick<ExtensionAPI, "events" | "getFlag" | "registerFlag">;
	private readonly exec: UnitExec;
	constructor(pi: Pick<ExtensionAPI, "events" | "getFlag" | "registerFlag">, exec: UnitExec) {
		this.pi = pi; this.exec = exec;
		// The guard must load the flag even when pi-todo is absent. Pi merges identical flag names.
		pi.registerFlag("todo-unit-file", { type: "string", description: "Executor-owned worker task seed" });
	}
	start(): void {
		this.seed = undefined;
		this.failure = undefined;
		const path = this.pi.getFlag("todo-unit-file");
		this.managed = path !== undefined;
		if (!this.managed) return;
		try {
			if (typeof path !== "string" || !path) throw new Error("missing seed path");
			this.seed = seedFile(path);
		} catch (error) { this.failure = String(error); }
	}
	private imported(ctx: ExtensionContext): Receipt {
		if (!this.seed || this.failure) throw new Error(this.failure ?? "missing seed");
		const provider = acquire(this.pi.events);
		if (!provider) throw new Error("task provider unavailable");
		const value = unwrap(provider.imported());
		if (!receipt(value) || value.localSessionId !== ctx.sessionManager.getSessionId()
			|| value.localSessionId !== this.seed.execution.requestId
			|| !sameExecution(value.execution, this.seed.execution) || !sameRef(value.unitRef, this.seed.binding.contract)
			|| value.digest !== this.seed.binding.exportDigest) throw new Error("missing or mismatched import acknowledgement");
		return value;
	}
	/** Called synchronously before any other before-agent-start await. */
	guard(ctx: ExtensionContext): boolean {
		if (!this.managed) return true;
		try {
			if (this.imported(ctx).seal !== null) throw new Error("worker execution is sealed");
			return true;
		} catch (error) {
			ctx.abort();
			ctx.ui.notify(`IMPORT_FAILED: ${String(error)}`, "error");
			return false;
		}
	}
	async settled(ctx: ExtensionContext): Promise<void> {
		if (!this.managed) return;
		const imported = this.imported(ctx);
		if (!imported.seal) return;
		const leaf = ctx.sessionManager.getLeafId();
		if (!leaf) throw new Error("IMPORT_FAILED: sealed session has no final leaf");
		await this.exec(["unit", "terminal-ack", "--input", JSON.stringify({
			execution: imported.execution, unitRef: imported.unitRef, digest: imported.digest,
			sealEntryId: imported.seal.entryId, sessionLeafId: leaf,
		})], ctx.cwd);
	}
}
