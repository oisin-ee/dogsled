// `/dogsled`: the control panel. It takes the board widget's place while open, so each row
// shows once. Status (the default), Hosts, Profiles, and Inbox, all from
// `dogsled board|machine|profiles|inbox --json`. The panel only shows choices. Each choice
// closes it with an action that the command runs, so dialogs never open inside the panel.
import { readFile, writeFile } from "node:fs/promises";
import type { ExtensionCommandContext, KeybindingsManager, Theme } from "@earendil-works/pi-coding-agent";
import { getSelectListTheme } from "@earendil-works/pi-coding-agent";
import { Key, matchesKey, type SelectItem, SelectList, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { age, type Board, clean } from "./board.ts";

export type Tab = "status" | "hosts" | "profiles" | "inbox";
export const TABS: Tab[] = ["status", "hosts", "profiles", "inbox"];
const TITLES: Record<Tab, string> = { status: "Status", hosts: "Hosts", profiles: "Profiles", inbox: "Inbox" };

export interface Machine { id: string; label: string; target: string; enabled: boolean }
export interface Profile { name: string; description: string; model?: string; thinking?: string; role?: string; prompt_path?: string }
export interface InboxLine { direction: "received" | "sent"; id: string; kind: string; peer: string; state: string; open: boolean; summary: string; created_ms: number }

export interface PanelData {
	board: Board;
	machines: Machine[];
	profiles: Profile[];
	inbox: InboxLine[];
	/** Any source that failed to load, by tab. */
	errors: Partial<Record<Tab, string>>;
	now: number;
}

/** What the user chose; the command runs it, then reopens the panel on `tab`. */
export type Action =
	| { tab: Tab; do: "retry"; host: string }
	| { tab: Tab; do: "reconnect" | "rename" | "enable" | "disable"; machine: Machine }
	| { tab: Tab; do: "add" }
	| { tab: Tab; do: "edit"; profile: Profile }
	| { tab: Tab; do: "refresh" }
	| undefined;

type Item = SelectItem & { action?: NonNullable<Action> };

function retryText(retry: number | undefined, now: number): string {
	if (retry === undefined) return "";
	return retry > now ? ` · retry in ${age(retry - now)}` : " · retrying";
}

export function items(tab: Tab, data: PanelData): Item[] {
	const error = data.errors[tab];
	if (error) return [{ value: "error", label: `Could not load: ${clean(error)}`, description: "Enter loads again", action: { tab, do: "refresh" } }];
	switch (tab) {
		case "status": {
			const hosts: Item[] = data.board.hosts.map(h => ({
				value: `host:${h.label}`,
				label: `${clean(h.label)}  ${h.reachable ? `service ${h.service}` : "unreachable"}${h.busy === undefined ? "" : ` · ${Math.round(h.busy)}/${h.cores} busy`}${retryText(h.retry_ms, data.now)}`,
				description: clean(h.reason ?? (h.eligible ? "has room" : "full")) + (h.label === "local" ? "" : " · Enter reads it again now"),
				...(h.label === "local" ? {} : { action: { tab, do: "retry" as const, host: h.label } }),
			}));
			const rows: Item[] = data.board.rows.map(r => ({
				value: `row:${r.id}`,
				label: `${clean(r.name)}  ${clean(r.what)} · ${clean(r.host)} · ${clean(r.state)}`,
				description: clean(r.activity) || `started ${age(r.age_ms)} ago`,
			}));
			return [...hosts, ...rows];
		}
		case "hosts": {
			const list: Item[] = data.machines.flatMap(m => {
				const name = `${clean(m.label)} (${clean(m.target)})`;
				return [
					{ value: `reconnect:${m.id}`, label: `${name}  ${m.enabled ? "enabled" : "disabled"}`, description: "Enter: authenticate in a new tab", action: { tab, do: "reconnect" as const, machine: m } },
					{ value: `rename:${m.id}`, label: `  rename ${clean(m.label)}`, action: { tab, do: "rename" as const, machine: m } },
					{ value: `toggle:${m.id}`, label: `  ${m.enabled ? "disable" : "enable"} ${clean(m.label)}`, action: { tab, do: m.enabled ? "disable" as const : "enable" as const, machine: m } },
				];
			});
			return [...list, { value: "add", label: "Add a machine", description: "Runs herdr machine add in a new tab", action: { tab, do: "add" } }];
		}
		case "profiles":
			if (data.profiles.length === 0) return [{ value: "none", label: "No profiles", description: "Add one under ~/.config/dogsled/profiles/<name>/" }];
			return data.profiles.map(p => ({
				value: `profile:${p.name}`,
				label: `${clean(p.name)}  ${clean(p.model ?? "default model")}${p.thinking ? ` · ${clean(p.thinking)}` : ""}${p.role ? ` · ${clean(p.role)}` : ""}`,
				description: clean(p.description) + (p.prompt_path ? " · Enter edits prompt.md" : ""),
				...(p.prompt_path ? { action: { tab, do: "edit" as const, profile: p } } : {}),
			}));
		case "inbox":
			if (data.inbox.length === 0) return [{ value: "none", label: "No messages" }];
			return data.inbox.map(l => ({
				value: `${l.direction}:${l.id}`,
				label: `${l.direction === "received" ? "←" : "→"} ${clean(l.kind)} ${l.direction === "received" ? "from" : "to"} ${clean(l.peer)}  ${clean(l.state)}`,
				description: `${age(data.now - l.created_ms)} ago${l.summary ? ` · ${clean(l.summary)}` : ""}`,
			}));
	}
}

/** The panel component: a tab bar, the board preview on Status, and one list per tab. */
export class Panel {
	private list!: SelectList;
	private entries: Item[] = [];
	private tab: Tab;
	private readonly data: PanelData;
	private readonly theme: Theme;
	private readonly keys: KeybindingsManager;
	private readonly done: (action: Action) => void;

	constructor(tab: Tab, data: PanelData, theme: Theme, keys: KeybindingsManager, done: (action: Action) => void) {
		this.tab = tab;
		this.data = data;
		this.theme = theme;
		this.keys = keys;
		this.done = done;
		this.select(tab);
	}

	private select(tab: Tab): void {
		this.tab = tab;
		this.entries = items(tab, this.data);
		// Labels carry the state, so the label column takes what it needs, up to 60 columns;
		// the description gets the rest.
		const widest = Math.max(...this.entries.map(e => visibleWidth(e.label))) + 2;
		this.list = new SelectList(this.entries, 12, getSelectListTheme(), { minPrimaryColumnWidth: 12, maxPrimaryColumnWidth: Math.min(60, widest) });
		this.list.onSelect = item => {
			const action = this.entries.find(e => e.value === item.value)?.action;
			if (action) this.done(action);
		};
		this.list.onCancel = () => this.done(undefined);
	}

	handleInput(data: string): void {
		const step = matchesKey(data, Key.tab) ? 1 : matchesKey(data, Key.shift("tab")) ? -1 : 0;
		if (step !== 0) {
			this.select(TABS[(TABS.indexOf(this.tab) + step + TABS.length) % TABS.length]!);
			return;
		}
		if (data === "r") {
			this.done({ tab: this.tab, do: "refresh" });
			return;
		}
		this.list.handleInput(data);
	}

	invalidate(): void {
		this.list.invalidate();
	}

	render(width: number): string[] {
		const t = this.theme;
		const bar = TABS.map(tab => tab === this.tab ? t.bold(t.fg("accent", `[${TITLES[tab]}]`)) : t.fg("muted", ` ${TITLES[tab]} `)).join(" ");
		const lines = [` ${t.bold("dogsled")}  ${bar}`, ""];
		lines.push(...this.list.render(width), "", t.fg("dim", ` tab/shift+tab switch · enter act · r refresh · ${this.keys.getKeys("tui.select.cancel").join("/")} close`));
		return lines.map(line => truncateToWidth(line, width));
	}
}

type Exec = (args: string[]) => Promise<string>;

async function load(exec: Exec, now: number): Promise<PanelData> {
	const data: PanelData = { board: { hosts: [], rows: [] }, machines: [], profiles: [], inbox: [], errors: {}, now };
	const read = async <T>(tab: Tab, args: string[], set: (value: T) => void) => {
		try { set(JSON.parse(await exec(args)) as T); } catch (error) { data.errors[tab] = error instanceof Error ? error.message : String(error); }
	};
	await Promise.all([
		read<Board>("status", ["board", "--json"], v => { data.board = v; }),
		read<Machine[]>("hosts", ["machine", "list", "--json"], v => { data.machines = v; }),
		read<Profile[]>("profiles", ["profiles", "--json"], v => { data.profiles = v; }),
		read<{ lines: InboxLine[] }>("inbox", ["inbox", "list", "--json"], v => { data.inbox = v.lines; }),
	]);
	return data;
}

/** Edit a profile's prompt in Pi's editor; its external-editor key opens `$EDITOR`. */
export async function editPrompt(ctx: ExtensionCommandContext, profile: Profile): Promise<string | undefined> {
	const path = profile.prompt_path!;
	const before = await readFile(path, "utf8");
	const after = await ctx.ui.editor(`${profile.name}: ${path}`, before);
	if (after === undefined || after === before) return undefined;
	await writeFile(path, after);
	return `Saved ${path}. New ${profile.name} children use it.`;
}

/** Run one panel action. Returns a notice for the user. */
async function run(action: NonNullable<Action>, ctx: ExtensionCommandContext, exec: Exec): Promise<string | undefined> {
	switch (action.do) {
		case "refresh":
			return undefined;
		case "retry": {
			const report = JSON.parse(await exec(["retry", action.host, "--json"])) as { reachable?: boolean; error?: string } | null;
			return report?.reachable ? `${action.host} is reachable` : `${action.host}: ${report?.error ?? "unreachable"}`;
		}
		case "reconnect":
			await exec(["machine", "reconnect", action.machine.id, "--json"]);
			return `Reconnecting ${action.machine.label} in a new tab; answer its prompts there.`;
		case "rename": {
			const label = await ctx.ui.input(`Rename ${action.machine.label}`, action.machine.label);
			if (!label || label === action.machine.label) return undefined;
			await exec(["machine", "rename", action.machine.id, label, "--json"]);
			return `Renamed ${action.machine.label} to ${label}`;
		}
		case "enable":
		case "disable":
			if (action.do === "disable" && !await ctx.ui.confirm(`Disable ${action.machine.label}?`, "Dogsled stops placing work there. Herdr keeps the machine saved.")) return undefined;
			await exec(["machine", action.do, action.machine.id, "--json"]);
			return `${action.do === "enable" ? "Enabled" : "Disabled"} ${action.machine.label}`;
		case "add": {
			const target = await ctx.ui.input("SSH target", "user@host");
			if (!target) return undefined;
			const label = await ctx.ui.input("Label (empty: Herdr's default)", "");
			await exec(["machine", "add", target, ...(label ? ["--label", label] : []), "--json"]);
			return `Adding ${target} in a new tab; answer its prompts there.`;
		}
		case "edit":
			return editPrompt(ctx, action.profile);
	}
}

/** Open the panel, run what the user picks, and reopen it until they close it. */
export async function openPanel(ctx: ExtensionCommandContext, exec: Exec, now: () => number = Date.now): Promise<void> {
	let tab: Tab = "status";
	for (;;) {
		const data = await load(exec, now());
		const action = await ctx.ui.custom<Action>((_tui, theme, keys, done) => new Panel(tab, data, theme, keys, done));
		if (!action) return;
		tab = action.tab;
		try {
			const notice = await run(action, ctx, exec);
			if (notice) ctx.ui.notify(notice, "info");
		} catch (error) {
			ctx.ui.notify(error instanceof Error ? error.message : String(error), "error");
		}
	}
}
