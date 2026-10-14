import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { type Exec, placeSpawn } from "./placement.ts";

const repo = realpathSync(mkdtempSync(join(tmpdir(), "wrangle-placement-")));
const tree = join(repo, ".worktrees", "feat-a");
mkdirSync(tree, { recursive: true });
const scratch = realpathSync(mkdtempSync(join(tmpdir(), "wrangle-scratch-")));

type Reply = { code?: number; stdout?: string; stderr?: string };

/** A fake `pi.exec`: answers the first rule whose prefix matches the argv, and records every call. */
function fake(options: {
	worktrees?: { path: string; branch?: string; is_linked_worktree: boolean; open_workspace_id?: string }[];
	panes?: { pane_id: string; agent?: string }[];
	branches?: string[];
	place?: Reply;
}) {
	const calls: string[][] = [];
	const exec: Exec = async (binary, args) => {
		calls.push([binary, ...args]);
		const reply = (r: Reply) => ({ code: r.code ?? 0, stdout: r.stdout ?? "", stderr: r.stderr ?? "" });
		if (binary === "git" && args[2] === "rev-parse" && args[3] === "--show-toplevel") {
			return args[1] === repo || args[1] === tree ? reply({ stdout: `${args[1]}\n` }) : reply({ code: 128, stderr: "not a git repository" });
		}
		if (binary === "git" && args[2] === "show-ref") {
			return reply({ code: (options.branches ?? []).includes(args.at(-1)!.replace("refs/heads/", "")) ? 0 : 1 });
		}
		if (binary === "git" && args[2] === "rev-parse" && args[3] === "HEAD") return reply({ stdout: "abc123\n" });
		if (binary === "git" && args[2] === "symbolic-ref") return reply({ stdout: "origin/main\n" });
		if (binary === "herdr" && args[0] === "worktree") {
			return reply({ stdout: JSON.stringify({ result: { source: { repo_root: repo }, worktrees: options.worktrees ?? [{ path: repo, branch: "main", is_linked_worktree: false, open_workspace_id: "w1" }] } }) });
		}
		if (binary === "herdr" && args[0] === "pane") return reply({ stdout: JSON.stringify({ result: { panes: options.panes ?? [] } }) });
		if (binary === "wrangle" && args[0] === "place") return reply(options.place ?? { stdout: JSON.stringify({ host: { id: "local" }, path: tree, pane_id: "w9:p1" }) });
		throw new Error(`unexpected call: ${binary} ${args.join(" ")}`);
	};
	return { exec, calls, place: () => calls.find(c => c[0] === "wrangle")?.slice(1) };
}

test("leaves other actions and an explicit placement alone", async () => {
	const f = fake({});
	for (const input of [{ action: "send", target: "w1:p1" }, { action: "spawn", placement: "new_tab", cwd: repo }]) {
		const before = structuredClone(input);
		assert.deepEqual(await placeSpawn(input, f.exec, "wrangle", repo), { block: false });
		assert.deepEqual(input, before);
	}
	assert.equal(f.calls.length, 0);
});

test("a prepared pane gets placement pane and loses cwd", async () => {
	const f = fake({});
	const input: Record<string, unknown> = { action: "spawn", pane: "w8M:p1", cwd: tree };
	assert.deepEqual(await placeSpawn(input, f.exec, "wrangle", repo), { block: false });
	assert.deepEqual(input, { action: "spawn", pane: "w8M:p1", placement: "pane" });
	assert.equal(f.calls.length, 0);
});

test("cwd in a worktree whose workspace is open uses that workspace's free pane", async () => {
	const f = fake({
		worktrees: [{ path: tree, branch: "feat/a", is_linked_worktree: true, open_workspace_id: "w8N" }],
		panes: [{ pane_id: "w8N:p1" }],
	});
	const input: Record<string, unknown> = { action: "spawn", label: "site home", cwd: tree };
	assert.deepEqual(await placeSpawn(input, f.exec, "wrangle", repo), { block: false });
	assert.deepEqual(input, { action: "spawn", label: "site home", placement: "pane", pane: "w8N:p1" });
	assert.equal(f.place(), undefined);
});

test("a worktree whose workspace already runs an agent is refused", async () => {
	const f = fake({
		worktrees: [{ path: tree, branch: "feat/a", is_linked_worktree: true, open_workspace_id: "w8N" }],
		panes: [{ pane_id: "w8N:p1", agent: "pi" }],
	});
	const decision = await placeSpawn({ action: "spawn", cwd: tree }, f.exec, "wrangle", repo);
	assert.equal(decision.block, true);
	assert.match(decision.block ? decision.reason : "", /already has an agent in workspace w8N/);
});

test("a worktree with no open workspace is opened through wrangle place on its own branch", async () => {
	const f = fake({ worktrees: [{ path: tree, branch: "feat/a", is_linked_worktree: true }] });
	const input: Record<string, unknown> = { action: "spawn", cwd: tree };
	assert.deepEqual(await placeSpawn(input, f.exec, "wrangle", repo), { block: false });
	assert.deepEqual(f.place(), ["place", "--json", "--host", "local", "--repo", repo, "--branch", "feat/a"]);
	assert.deepEqual(input, { action: "spawn", placement: "pane", pane: "w9:p1" });
});

test("the main checkout gets a new agent branch from HEAD, skipping taken names", async () => {
	const f = fake({ branches: ["agent/site-home"] });
	const input: Record<string, unknown> = { action: "spawn", label: "Site home!" };
	assert.deepEqual(await placeSpawn(input, f.exec, "wrangle", repo), { block: false });
	assert.deepEqual(f.place(), ["place", "--json", "--host", "local", "--repo", repo, "--branch", "agent/site-home-2", "--base", "abc123"]);
	assert.deepEqual(input, { action: "spawn", label: "Site home!", placement: "pane", pane: "w9:p1" });
});

test("a remote machine places a branch from the default branch on that host", async () => {
	const f = fake({ place: { stdout: JSON.stringify({ host: { id: "peer" }, path: "/remote/x", pane_id: "wH:p1" }) } });
	const input: Record<string, unknown> = { action: "spawn", machine: "peer", name: "u1" };
	assert.deepEqual(await placeSpawn(input, f.exec, "wrangle", repo), { block: false });
	assert.deepEqual(f.place(), ["place", "--json", "--host", "peer", "--repo", repo, "--branch", "agent/u1", "--base", "origin/main"]);
	assert.deepEqual(input, { action: "spawn", machine: "peer", name: "u1", placement: "pane", pane: "wH:p1" });
});

test("a remote machine with only cwd is refused with the place command", async () => {
	const f = fake({});
	const decision = await placeSpawn({ action: "spawn", machine: "peer", cwd: "/remote/rondo" }, f.exec, "wrangle", repo);
	assert.equal(decision.block, true);
	assert.match(decision.block ? decision.reason : "", /wrangle place --host peer/);
	assert.equal(f.calls.length, 0);
});

test("a queued or failed placement blocks the spawn with the reason", async () => {
	const queued = fake({ place: { code: 1, stdout: JSON.stringify({ queued: true, ticket: "t-1" }) } });
	const q = await placeSpawn({ action: "spawn", label: "x" }, queued.exec, "wrangle", repo);
	assert.match(q.block ? q.reason : "", /queued ticket t-1/);
	const failed = fake({ place: { code: 2, stdout: JSON.stringify({ error: "prepare hook failed", deadline: false }) } });
	const e = await placeSpawn({ action: "spawn", label: "x" }, failed.exec, "wrangle", repo);
	assert.match(e.block ? e.reason : "", /wrangle place for branch agent\/x failed: prepare hook failed/);
});

test("a directory outside any repository gets a new workspace", async () => {
	const f = fake({});
	const input: Record<string, unknown> = { action: "spawn", cwd: scratch };
	assert.deepEqual(await placeSpawn(input, f.exec, "wrangle", repo), { block: false });
	assert.deepEqual(input, { action: "spawn", cwd: scratch, placement: "new_workspace" });
});
