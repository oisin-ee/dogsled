export interface WorkJob {
	id: string; host: string; owner: string; cmd: string[]; state: string;
	ended_ms: number | null; exit: number | null;
}
export interface JobHistory { jobs: WorkJob[]; hosts?: { id: string; label: string }[]; error?: string }
export type WorkExec = (args: string[]) => Promise<string>;
function record(value: unknown): value is Record<string, unknown> { return value !== null && typeof value === "object"; }
function job(value: unknown): value is WorkJob {
	return record(value) && ["id", "host", "owner", "state"].every(key => typeof value[key] === "string")
		&& Array.isArray(value["cmd"]) && value["cmd"].every(part => typeof part === "string")
		&& (value["ended_ms"] === null || typeof value["ended_ms"] === "number")
		&& (value["exit"] === null || typeof value["exit"] === "number");
}

/** Serve publishes each owner's history, joined on host and job id; this only checks its shape. */
export function parseJobHistory(value: unknown): JobHistory {
	if (!record(value) || !Array.isArray(value["jobs"]) || !value["jobs"].every(job)) throw new Error("Invalid Dogsled job history");
	const hosts = value["hosts"];
	if (!Array.isArray(hosts) || !hosts.every((host): host is { id: string; label: string } => record(host) && typeof host["id"] === "string" && typeof host["label"] === "string")) throw new Error("Invalid Dogsled host identities");
	return { jobs: value["jobs"], hosts: hosts.map(host => ({ id: host.id, label: host.label })) };
}
