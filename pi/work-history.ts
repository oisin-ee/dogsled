import { clean } from "./board.ts";

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

/** History comes from the owner's ledger, not a coincidentally equal pane on another host. */
export async function loadJobHistory(exec: WorkExec, owner: string): Promise<JobHistory> {
	try {
		const [itemText, jobText, hostText] = await Promise.all([exec(["items", "--owner", owner, "--json"]), exec(["ps", "--all", "--json"]), exec(["machine", "list", "--json"])]);
		const items: unknown = JSON.parse(itemText), jobs: unknown = JSON.parse(jobText), hosts: unknown = JSON.parse(hostText);
		if (!Array.isArray(hosts) || !hosts.every((host): host is { id: string; label: string } => record(host) && typeof host["id"] === "string" && typeof host["label"] === "string")) throw new Error("Invalid Dogsled host identities");
		if (!record(items) || !Array.isArray(items["items"]) || !Array.isArray(jobs) || !jobs.every(job)) throw new Error("Invalid Dogsled job history");
		const allowed = new Set<string>();
		for (const item of items["items"]) {
			if (!record(item) || typeof item["id"] !== "string" || typeof item["host"] !== "string" || typeof item["owner_pane"] !== "string") throw new Error("Invalid Dogsled owner item");
			if (item["owner_pane"] === owner && item["kind"] === "job") allowed.add(JSON.stringify([item["host"], item["id"]]));
		}
		return { jobs: jobs.filter(item => allowed.has(JSON.stringify([item.host, item.id]))), hosts };
	} catch (error) {
		return { jobs: [], error: `History unavailable: ${clean(String(error))}` };
	}
}
