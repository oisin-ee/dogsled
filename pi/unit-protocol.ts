import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export interface UnitRef { sessionId: string; unitId: string; revision: number; contractEntryId: string }
export interface Execution { executor: string; ownerId: string; requestId: string }
export interface Binding { contract: UnitRef; execution: Execution; exportDigest: string }
export interface Exported { version: number; provider: string; contract: { ref: UnitRef; scope: { repositoryId: string } }; digest: string; [key: string]: unknown }
export interface Prepared { execution: Execution; binding: Binding; exported: Exported }
export interface Receipt { version: number; execution: Execution; unitRef: UnitRef; digest: string; localSessionId: string; seal: { entryId: string; disposition: "completed" | "abandoned" } | null }
export type Result = { ok: true; value: unknown } | { ok: false; error: { code: string; message: string } };
export interface TaskProvider {
	inspect(id: string): Result;
	claim(input: { expected: UnitRef; execution: Execution }): Result;
	settle(input: unknown): Result;
	imported(): Result;
}
export type Placement = { host?: string; branch?: string; base?: string };
export function record(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }
function text(value: unknown): value is string { return typeof value === "string" && value.trim().length > 0; }
export function reference(value: unknown): value is UnitRef {
	return record(value) && text(value.sessionId) && text(value.unitId) && Number.isSafeInteger(value.revision) && Number(value.revision) > 0 && text(value.contractEntryId);
}
export function execution(value: unknown): value is Execution {
	return record(value) && value.executor === "dogsled" && text(value.ownerId) && text(value.requestId);
}
export function sameRef(a: UnitRef, b: UnitRef): boolean {
	return a.sessionId === b.sessionId && a.unitId === b.unitId && a.revision === b.revision && a.contractEntryId === b.contractEntryId;
}
export function sameExecution(a: Execution, b: Execution): boolean {
	return a.executor === b.executor && a.ownerId === b.ownerId && a.requestId === b.requestId;
}
export function binding(value: unknown): value is Binding {
	return record(value) && reference(value.contract) && execution(value.execution) && typeof value.exportDigest === "string" && /^[a-f0-9]{64}$/.test(value.exportDigest);
}
export function exported(value: unknown): value is Exported {
	return record(value) && value.version === 1 && value.provider === "pi-todo" && record(value.contract)
		&& reference(value.contract.ref) && record(value.contract.scope) && text(value.contract.scope.repositoryId)
		&& typeof value.digest === "string" && /^[a-f0-9]{64}$/.test(value.digest);
}
export function prepared(value: unknown): value is Prepared {
	return record(value) && execution(value.execution) && binding(value.binding) && exported(value.exported)
		&& sameExecution(value.execution, value.binding.execution) && sameRef(value.binding.contract, value.exported.contract.ref)
		&& value.binding.exportDigest === value.exported.digest;
}
export function receipt(value: unknown): value is Receipt {
	return record(value) && value.version === 1 && execution(value.execution) && reference(value.unitRef)
		&& typeof value.digest === "string" && /^[a-f0-9]{64}$/.test(value.digest) && text(value.localSessionId)
		&& (value.seal === null || (record(value.seal) && text(value.seal.entryId) && ["completed", "abandoned"].includes(String(value.seal.disposition))));
}
function provider(value: unknown): value is TaskProvider {
	return record(value) && ["inspect", "claim", "settle", "imported"].every(key => typeof value[key] === "function");
}
export function acquire(events: ExtensionAPI["events"]): TaskProvider | undefined {
	const replies: unknown[] = [];
	events.emit("oisin.tasks.acquire.v1", { version: 1, reply: (value: unknown) => { replies.push(value); } });
	if (replies.length !== 1 || !provider(replies[0])) return undefined;
	return replies[0];
}
export function unwrap(result: Result): unknown {
	if (!record(result) || typeof result.ok !== "boolean") throw new Error("PROVIDER_UNAVAILABLE: invalid capability result");
	if (!result.ok) throw new Error(`${result.error.code}: ${result.error.message}`);
	return result.value;
}
export function unitInput(input: Record<string, unknown>): { id: string; placement: Placement } {
	if (Object.keys(input).some(key => !["unit", "host", "branch", "base"].includes(key)) || !record(input.unit)
		|| Object.keys(input.unit).length !== 1 || !text(input.unit.id)) throw new Error("REQUEST_CONFLICT: unit spawn cannot include a brief, profile, cwd, or prepared envelope");
	const placement: Placement = {};
	for (const key of ["host", "branch", "base"] as const) {
		if (input[key] === undefined) continue;
		if (!text(input[key])) throw new Error(`REQUEST_CONFLICT: ${key} must be a nonempty string`);
		placement[key] = input[key];
	}
	if (placement.base && !placement.branch) throw new Error("REQUEST_CONFLICT: base requires branch");
	return { id: input.unit.id, placement };
}

/** Clone transport is not repository identity. Keep path case; do not accept local paths. */
export function repositoryIdentity(origin: string): string {
	const value = origin.trim();
	const scp = value.includes("://") ? null : /^(?:[^/@:\s]+@)?(?<host>[^/:\s]+):(?<path>[^\s]+)$/.exec(value);
	const url = new URL(scp?.groups ? `ssh://${scp.groups["host"]}/${scp.groups["path"]}` : value);
	const path = url.pathname.replace(/^\//, "").replace(/\/$/, "").replace(/\.git$/, "");
	if (!["ssh:", "https:", "http:", "git:"].includes(url.protocol) || !url.hostname || !path || url.search || url.hash
		|| path.split("/").some(part => !part || part === "." || part === "..")) throw new Error("REQUEST_CONFLICT: origin must identify a remote repository host/path");
	return `${url.hostname.toLowerCase()}/${path}`;
}
