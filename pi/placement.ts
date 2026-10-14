import { realpathSync } from "node:fs";
import { resolve } from "node:path";

/** The subset of `pi.exec` this module needs. */
export type Exec = (
	binary: string,
	args: string[],
	options?: { timeout?: number; signal?: AbortSignal },
) => Promise<{ code: number; stdout: string; stderr: string }>;

/** What happens to one `agents spawn` call: run it as mutated, or block it with a reason. */
export type Decision = { block: false } | { block: true; reason: string };

// Prepare (worktree creation, the repository's create hooks) has a 10-minute budget in wrangle.
const PLACE_TIMEOUT_MS = 11 * 60_000;
const QUICK_TIMEOUT_MS = 30_000;

interface Worktree {
	path: string;
	branch?: string;
	is_linked_worktree: boolean;
	open_workspace_id?: string;
}

function real(path: string): string {
	try {
		return realpathSync(path);
	} catch {
		return path;
	}
}

function slug(text: string): string {
	return text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40).replace(/-+$/, "") || "child";
}

/**
 * Give every `agents spawn` without a `placement` its own worktree workspace, which Herdr
 * groups under the repository's workspace. Without this, Shepherdr opens a tab in the
 * caller's workspace: the child shares the lead's checkout layout, and `wrangle close`
 * cannot close it. An explicit `placement` is the caller's choice and is left alone.
 *
 * Mutates `input` in place, as Pi's `tool_call` contract requires.
 */
export async function placeSpawn(
	input: Record<string, unknown>,
	exec: Exec,
	wrangle: string,
	cwd: string,
	signal?: AbortSignal,
): Promise<Decision> {
	if (input["action"] !== "spawn" || input["placement"] !== undefined) return { block: false };
	const quick = { timeout: QUICK_TIMEOUT_MS, ...(signal ? { signal } : {}) };

	async function run(binary: string, args: string[], timeout = QUICK_TIMEOUT_MS) {
		return exec(binary, args, { ...quick, timeout });
	}

	// A prepared pane: Shepherdr ignores `pane` unless `placement` names it, and rejects `cwd` with it.
	if (typeof input["pane"] === "string" && input["pane"]) {
		input["placement"] = "pane";
		delete input["cwd"];
		return { block: false };
	}

	const machine = typeof input["machine"] === "string" && input["machine"] !== "local" ? input["machine"] : undefined;
	if (machine && typeof input["cwd"] === "string") {
		return {
			block: true,
			reason:
				`agents spawn on machine ${machine} with only cwd would open a tab outside a worktree workspace. ` +
				`Run \`wrangle place --host ${machine} --branch <branch> --repo <local repo> --json\`, then spawn with placement="pane" and its pane_id.`,
		};
	}

	const dir = resolve(cwd, typeof input["cwd"] === "string" ? input["cwd"] : ".");
	const top = await run("git", ["-C", dir, "rev-parse", "--show-toplevel"]);
	if (top.code !== 0) {
		// Not a repository: no worktree to make, but still keep the child out of the caller's workspace.
		input["placement"] = "new_workspace";
		return { block: false };
	}
	const checkout = real(top.stdout.trim());
	const listed = await run("herdr", ["worktree", "list", "--cwd", checkout]);
	if (listed.code !== 0) throw new Error(`herdr worktree list failed: ${listed.stderr.trim() || listed.stdout.trim()}`);
	const result = JSON.parse(listed.stdout)["result"] as { source: { repo_root: string }; worktrees: Worktree[] };
	const repo = result.source.repo_root;
	const tree = result.worktrees.find(w => real(w.path) === checkout);

	let branch: string;
	let base: string | undefined;
	if (!machine && tree?.is_linked_worktree) {
		if (tree.open_workspace_id) {
			// The worktree's workspace is open: use its free pane, never a pane with an agent in it.
			const panes = await run("herdr", ["pane", "list", "--workspace", tree.open_workspace_id]);
			if (panes.code !== 0) throw new Error(`herdr pane list failed: ${panes.stderr.trim() || panes.stdout.trim()}`);
			const free = (JSON.parse(panes.stdout)["result"]["panes"] as { pane_id: string; agent?: string }[]).find(p => !p.agent);
			if (!free) {
				return {
					block: true,
					reason: `worktree ${tree.path} already has an agent in workspace ${tree.open_workspace_id}. Use another branch, or send that agent the task.`,
				};
			}
			input["placement"] = "pane";
			input["pane"] = free.pane_id;
			delete input["cwd"];
			return { block: false };
		}
		if (!tree.branch) {
			return { block: true, reason: `worktree ${tree.path} has a detached HEAD; spawn into a branch worktree.` };
		}
		branch = tree.branch;
	} else {
		// The main checkout, or a remote host: a new branch for this child, from the caller's committed HEAD.
		const label = [input["name"], input["label"]].find(v => typeof v === "string" && v.trim()) as string | undefined;
		const stem = `agent/${slug(label ?? "child")}`;
		branch = stem;
		for (let n = 2; (await run("git", ["-C", repo, "show-ref", "--verify", "--quiet", `refs/heads/${branch}`])).code === 0; n++) {
			branch = `${stem}-${n}`;
		}
		if (machine) {
			// A remote host cannot see unpushed commits; start from the default branch there.
			const head = await run("git", ["-C", repo, "symbolic-ref", "--short", "refs/remotes/origin/HEAD"]);
			base = head.code === 0 && head.stdout.trim() ? head.stdout.trim() : undefined;
		} else {
			const head = await run("git", ["-C", repo, "rev-parse", "HEAD"]);
			if (head.code !== 0) throw new Error(`git rev-parse HEAD failed in ${repo}`);
			base = head.stdout.trim();
		}
	}

	const args = ["place", "--json", "--host", machine ?? "local", "--repo", repo, "--branch", branch, ...(base ? ["--base", base] : [])];
	const placed = await run(wrangle, args, PLACE_TIMEOUT_MS);
	const out = ((): Record<string, unknown> => {
		try {
			return JSON.parse(placed.stdout.trim().split("\n").at(-1) ?? "") as Record<string, unknown>;
		} catch {
			return {};
		}
	})();
	if (out["queued"] === true) {
		return {
			block: true,
			reason: `No room on ${machine ?? "local"}; wrangle queued ticket ${String(out["ticket"])}. Continue other work and end your turn. Spawn again when wrangle reports the ticket admitted.`,
		};
	}
	const pane = out["pane_id"];
	if (placed.code !== 0 || typeof pane !== "string") {
		const why = typeof out["error"] === "string" ? out["error"] : placed.stderr.trim() || placed.stdout.trim() || `exit ${placed.code}`;
		return { block: true, reason: `wrangle place for branch ${branch} failed: ${why}` };
	}
	input["placement"] = "pane";
	input["pane"] = pane;
	delete input["cwd"];
	return { block: false };
}
