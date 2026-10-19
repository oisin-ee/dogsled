import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { visibleWidth } from "@earendil-works/pi-tui";
import { items, openPanel, Panel, type PanelData } from "./panel.ts";

// SelectList draws with the active theme, which Pi initialises at startup.
initTheme("dark");
const plain = { fg: (_c: string, t: string) => t, bold: (t: string) => t } as any;
const keys = { getKeys: () => ["escape"] } as any;
const TAB = "\t", SHIFT_TAB = "\u001b[Z", ENTER = "\r", DOWN = "\u001b[B";

const data: PanelData = {
	now: 100_000,
	board: {
		hosts: [
			{ label: "local", service: "up", reachable: true, busy: 1, cores: 12, eligible: true },
			{ label: "netcup", service: "up", reachable: false, cores: 16, eligible: false, reason: "ssh: timed out", retry_ms: 140_000 },
		],
		rows: [{ group: "running", kind: "agent", id: "a", name: "api", what: "pi · sol", host: "local", state: "working", activity: "edit x", age_ms: 60_000 }],
	},
	machines: [{ id: "8103", label: "netcup", target: "netcup-dev", enabled: true }],
	profiles: [{ name: "general", description: "Ordinary units", model: "alias/sol", thinking: "medium", prompt_path: "/p/general/prompt.md" }],
	inbox: [{ direction: "received", id: "r1", kind: "reply", peer: "api (w2:p1 on m)", state: "claimed, offered again in 20s", open: true, summary: "Done.", created_ms: 40_000 }],
	errors: {},
};

test("Status shows each host's retry and reads an unreachable host again on Enter", () => {
	const status = items("status", data);
	assert.match(status[1]!.label, /^netcup {2}unreachable · retry in 40s$/);
	assert.deepEqual(status[1]!.action, { tab: "status", do: "retry", host: "netcup" });
	assert.equal(status[0]!.action, undefined, "local is never retried");
	assert.match(status[2]!.label, /^api {2}pi · sol · local · working$/);
	assert.match(items("inbox", data)[0]!.label, /^← reply from api \(w2:p1 on m\) {2}claimed, offered again in 20s$/);
	assert.equal(items("profiles", data)[0]!.action?.do, "edit");
	assert.deepEqual(items("hosts", data).map(i => i.action?.do), ["reconnect", "rename", "disable", "add"]);
	assert.equal(items("hosts", { ...data, errors: { hosts: "herdr failed" } })[0]!.action?.do, "refresh");
});

test("tabs cycle both ways, Enter returns the row's action, and every line fits", () => {
	const chosen: unknown[] = [];
	const panel = new Panel("status", data, plain, keys, a => chosen.push(a));
	assert.ok(panel.render(100)[0]!.includes("[Status]"));
	panel.handleInput(SHIFT_TAB);
	assert.ok(panel.render(100)[0]!.includes("[Inbox]"));
	panel.handleInput(TAB);
	panel.handleInput(TAB);
	assert.ok(panel.render(100)[0]!.includes("[Hosts]"));
	panel.handleInput(DOWN);
	panel.handleInput(ENTER);
	assert.deepEqual(chosen, [{ tab: "hosts", do: "rename", machine: data.machines[0] }]);
	for (const width of [60, 100, 160]) for (const line of panel.render(width)) assert.ok(visibleWidth(line) <= width, line);
});

test("openPanel runs the chosen action and reopens on the same tab until closed", async () => {
	const dir = mkdtempSync(join(tmpdir(), "dogsled-panel-"));
	const prompt = join(dir, "prompt.md");
	writeFileSync(prompt, "old\n");
	const calls: string[][] = [];
	const exec = async (args: string[]) => {
		calls.push(args);
		if (args[0] === "board") return JSON.stringify(data.board);
		if (args[0] === "machine" && args[1] === "list") return JSON.stringify(data.machines);
		if (args[0] === "profiles") return JSON.stringify([{ ...data.profiles[0], prompt_path: prompt }]);
		if (args[0] === "inbox") return JSON.stringify({ lines: data.inbox });
		if (args[0] === "retry") return JSON.stringify({ reachable: true });
		return "{}";
	};
	const script: Array<(panel: Panel) => void> = [
		p => { p.handleInput(DOWN); p.handleInput(ENTER); },
		p => { p.handleInput(TAB); p.handleInput(TAB); p.handleInput(ENTER); },
		p => p.handleInput("\u001b"),
	];
	const tabs: string[] = [];
	const notices: string[] = [];
	const ctx = {
		ui: {
			custom: (factory: any) => new Promise(resolve => {
				const panel: Panel = factory(undefined, plain, keys, resolve);
				tabs.push(panel.render(100)[0]!.match(/\[(\w+)\]/)![1]!);
				script.shift()!(panel);
			}),
			editor: async (_title: string, text: string) => `${text}new\n`,
			notify: (text: string) => notices.push(text),
		},
	} as any;
	await openPanel(ctx, exec, () => 100_000);
	assert.deepEqual(tabs, ["Status", "Status", "Profiles"]);
	assert.ok(calls.some(c => c.join(" ") === "retry netcup --json"));
	assert.equal(readFileSync(prompt, "utf8"), "old\nnew\n");
	assert.deepEqual(notices, ["netcup is reachable", `Saved ${prompt}. New general children use it.`]);
});
