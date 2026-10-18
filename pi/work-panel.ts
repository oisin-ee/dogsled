import type { ExtensionContext, KeybindingsManager, Theme } from "@earendil-works/pi-coding-agent";
import { Key, matchesKey, SelectList, Text, ScrollView, truncateToWidth } from "@earendil-works/pi-tui";
import { clean } from "./board.ts";
import type { WorkState } from "./work.ts";
import type { JobHistory, WorkExec } from "./work-history.ts";

export type WorkTab = "all" | "todos" | "agents" | "jobs";
const tabs: WorkTab[] = ["all", "todos", "agents", "jobs"];
export interface WorkEntry {
	value: string; label: string; description?: string;
	kind: "todo" | "agent" | "job" | "queued" | "history" | "empty";
	id?: string; host?: string; pane?: string; detail?: string; running?: boolean;
}
export interface WorkData extends WorkState { history: JobHistory }
export interface WorkCursor { tab: WorkTab; selected: string | undefined; history: boolean }
export interface WorkAction { action: "inspect" | "stop" | "cancel" | "send" | "refresh"; entry: WorkEntry; cursor: WorkCursor }

/** The same content in every tab, without task mutation or duplicate job rows. */
export function workEntries(data: WorkData, tab: WorkTab, history: boolean): WorkEntry[] {
	const todos: WorkEntry[] = (data.todo?.tasks ?? []).map(task => ({ value: `todo:${task.content}`, label: `${task.status === "in_progress" ? "→" : task.status === "completed" ? "✓" : task.status === "abandoned" ? "−" : "·"} ${clean(task.content)}`, description: task.status, kind: "todo", detail: task.content }));
	const canonicalHost = (label: string) => data.history.hosts?.find(host => host.label === label)?.id ?? label;
	const rows: WorkEntry[] = data.board.rows.map(row => ({ value: `${row.kind}:${canonicalHost(row.host)}:${row.id}`, id: row.id, host: canonicalHost(row.host), ...(row.pane ? { pane: row.pane } : {}), kind: row.group === "waiting" ? "queued" : row.kind === "job" ? "job" : "agent", label: `${row.group === "attention" || row.group === "to_land" ? "!" : "●"} ${clean(row.name)}`, description: `${clean(row.state)} · ${clean(row.host)}`, running: row.group === "running" }));
	const visible = tab === "todos" ? todos : tab === "agents" ? rows.filter(row => row.value.startsWith("agent:")) : tab === "jobs" ? rows.filter(row => row.value.startsWith("job:")) : [...rows.filter(row => row.label.startsWith("!")), ...todos.filter(row => row.description === "in_progress"), ...rows.filter(row => !row.label.startsWith("!")), ...todos.filter(row => row.description === "pending")];
	if (tab === "all" || tab === "jobs") {
		const known = new Set(rows.map(row => row.value));
		const done = data.history.jobs.filter(job => job.ended_ms !== null && !known.has(`job:${job.host}:${job.id}`));
		if (done.length) visible.push({ value: "history", kind: "history", label: `${history ? "▾" : "▸"} Completed jobs (${done.length})` });
		if (history) visible.push(...done.map(job => ({ value: `job:${job.host}:${job.id}`, id: job.id, host: job.host, kind: "job" as const, label: `${job.exit === 0 ? "✓" : "!"} ${clean(job.cmd.join(" "))}`, description: `${clean(job.state)} · exit ${job.exit ?? "unknown"} · ${clean(job.host)}`, running: false })));
		if (data.history.error) visible.push({ value: "history-error", kind: "empty", label: data.history.error });
	}
	if (data.error) visible.unshift({ value: "error", kind: "empty", label: clean(data.error) });
	return visible.length ? visible : [{ value: "empty", kind: "empty", label: "No work in this view." }];
}

/** One interaction. The existing refresh coordinator supplies changes without another timer. */
export class WorkPanel {
	private list!: SelectList;
	private entries: WorkEntry[] = [];
	private data: WorkData;
	readonly cursor: WorkCursor;
	private readonly theme: Theme;
	private readonly keys: KeybindingsManager;
	private readonly done: (action: WorkAction | undefined) => void;
	private readonly renderRequest: () => void;
	private readonly rows: () => number;
	private height = 0;

	constructor(data: WorkData, cursor: WorkCursor, theme: Theme, keys: KeybindingsManager, done: (action: WorkAction | undefined) => void, request: () => void, rows: () => number) {
		this.data = data; this.cursor = cursor; this.theme = theme; this.keys = keys; this.done = done; this.renderRequest = request; this.rows = rows;
		this.rebuild();
	}
	update(data: WorkData): void { this.data = data; this.rebuild(); this.renderRequest(); }
	private rebuild(preserveSelection = true): void {
		if (preserveSelection) this.cursor.selected = this.list?.getSelectedItem()?.value ?? this.cursor.selected;
		this.entries = workEntries(this.data, this.cursor.tab, this.cursor.history);
		this.height = Math.max(1, Math.min(14, Math.floor(this.rows() * 0.8) - 5));
		this.list = new SelectList(this.entries, this.height, {
			selectedPrefix: text => this.theme.fg("accent", text), selectedText: text => this.theme.fg("accent", text),
			description: text => this.theme.fg("muted", text), scrollInfo: text => this.theme.fg("dim", text), noMatch: text => this.theme.fg("muted", text),
		}, { minPrimaryColumnWidth: 8, maxPrimaryColumnWidth: 60 });
		this.list.setSelectedIndex(Math.max(0, this.entries.findIndex(entry => entry.value === this.cursor.selected)));
		this.list.onCancel = () => this.done(undefined);
		this.list.onSelect = () => this.act("inspect");
	}
	private act(action: WorkAction["action"]): void {
		const selected = this.list.getSelectedItem();
		const entry = this.entries.find(item => item.value === selected?.value);
		if (!entry) return;
		if (entry.kind === "history" && action === "inspect") { this.cursor.history = !this.cursor.history; this.rebuild(); this.renderRequest(); return; }
		if (entry.kind === "empty" && action !== "refresh") return;
		if (action === "stop" && (entry.kind !== "job" || !entry.running)) return;
		if (action === "cancel" && entry.kind !== "queued") return;
		if (action === "send" && entry.kind !== "agent") return;
		this.cursor.selected = entry.value;
		this.done({ action, entry, cursor: this.cursor });
	}
	handleInput(input: string): void {
		const step = matchesKey(input, Key.tab) ? 1 : matchesKey(input, Key.shift("tab")) ? -1 : 0;
		if (step) { this.cursor.tab = tabs[(tabs.indexOf(this.cursor.tab) + step + tabs.length) % tabs.length]!; this.cursor.selected = undefined; this.rebuild(false); this.renderRequest(); return; }
		if (input === "r") this.act("refresh");
		else if (input === "s") this.act("stop");
		else if (input === "c") this.act("cancel");
		else if (input === "m") this.act("send");
		else this.list.handleInput(input);
	}
	invalidate(): void { this.list.invalidate(); }
	render(width: number): string[] {
		if (this.height !== Math.max(1, Math.min(14, Math.floor(this.rows() * 0.8) - 5))) this.rebuild();
		const tabbar = tabs.map(tab => tab === this.cursor.tab ? `[${tab.toUpperCase()}]` : tab).join(" · ");
		return [this.theme.fg("accent", `WORK  ${tabbar}`), this.theme.fg("dim", "Todos: Pi branch · Agents/jobs: Herdr owner pane"), ...this.list.render(width), this.theme.fg("dim", "Tab view · Enter inspect · s stop · c cancel · m message"), this.theme.fg("dim", `r refresh · ${this.keys.getKeys("tui.select.cancel").join("/")} close`)].map(line => truncateToWidth(line, width));
	}
}

/** Strip terminal control sequences from output while retaining line breaks. */
function safeOutput(value: string): string {
	return value.replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, "").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "").split("\n").map(clean).join("\n");
}

export async function inspectEntry(entry: WorkEntry, exec: WorkExec): Promise<string> {
	if (entry.kind === "todo") return entry.detail ?? "";
	if (entry.kind === "queued") return `${entry.label}\n${entry.description ?? ""}\nWaiting for placement. Cancel explicitly to remove this ticket.`;
	if (!entry.id) return "This record is unavailable.";
	const output = await exec(["read", entry.kind === "job" ? entry.id : entry.pane ?? entry.id, "--host", entry.host ?? "local"]);
	const hint = entry.kind === "job" ? `Recent job output (Wrangle's bounded tail).\nFull log: wrangle logs ${clean(entry.id)}\n\n` : "";
	return hint + safeOutput(output);
}

/** Actions run after closing the list, so confirmation/input dialogs own focus. */
export async function runWorkAction(action: WorkAction, ctx: ExtensionContext, exec: WorkExec, signal: AbortSignal): Promise<string | undefined> {
	if (signal.aborted) return undefined;
	const entry = action.entry;
	if (action.action === "inspect") return inspectEntry(entry, exec);
	if (action.action === "stop" && entry.kind === "job" && entry.running && entry.id && await ctx.ui.confirm("Stop job?", clean(entry.label), { signal }) && !signal.aborted) await exec(["stop", entry.id]);
	if (action.action === "cancel" && entry.kind === "queued" && entry.id && await ctx.ui.confirm("Cancel queued work?", clean(entry.label), { signal }) && !signal.aborted) await exec(["cancel", entry.id]);
	if (action.action === "send" && entry.kind === "agent") {
		const message = await ctx.ui.input(`Message ${clean(entry.label)}`, undefined, { signal });
		if (!signal.aborted && message?.trim() && entry.id) await exec(["send", entry.pane ?? entry.id, "--host", entry.host ?? "local", "--message", message]);
	}
	return undefined;
}

export async function showWorkDetail(ctx: ExtensionContext, title: string, body: string, signal: AbortSignal): Promise<void> {
	if (signal.aborted) return;
	await ctx.ui.custom<void>((tui, theme, keys, done) => {
		const abort = () => done();
		signal.addEventListener("abort", abort, { once: true });
		const text = new Text(safeOutput(body) || "No output yet.", 0, 0);
		const scroll = new ScrollView(text, { overscroll: "contain", scrollbar: "hidden" });
		return {
			dispose() { signal.removeEventListener("abort", abort); },
			render(width: number) {
				const height = Math.max(1, Math.min(24, Math.floor(tui.terminal.rows * 0.8) - 3));
				scroll.updateLayout(text.render(width).length, height, () => tui.requestRender());
				return [theme.fg("accent", truncateToWidth(title, width)), ...scroll.render(width).slice(scroll.scrollTop, scroll.scrollTop + height), theme.fg("dim", truncateToWidth("↑↓ / PageUp / PageDown scroll · Esc back", width))];
			},
			invalidate() { text.invalidate(); scroll.invalidate(); },
			handleInput(input: string) {
				if (keys.matches(input, "tui.select.cancel")) { done(); return; }
				if (matchesKey(input, Key.up)) scroll.scrollBy(-1);
				if (matchesKey(input, Key.down)) scroll.scrollBy(1);
				if (matchesKey(input, Key.pageUp)) scroll.scrollBy(-scroll.viewportHeight);
				if (matchesKey(input, Key.pageDown)) scroll.scrollBy(scroll.viewportHeight);
				tui.requestRender();
			},
		};
	}, { overlay: true, overlayOptions: { width: "90%", maxHeight: "85%", anchor: "center" } });
}
