import type { ExtensionContext, Theme } from "@earendil-works/pi-coding-agent";
import { getKeybindings, Key, matchesKey, Text, ScrollView, truncateToWidth } from "@earendil-works/pi-tui";
import { boardColumns, boardHeader, boardRow, clean, cleanRow, groupTitle, GROUPS, type Group, type Row } from "./board.ts";
import { taskLines, type WorkState } from "./work.ts";
import type { TodoView, WorkTodo } from "./work-todo.ts";
import type { JobHistory, WorkExec } from "./work-history.ts";

export type WorkTab = "all" | "todos" | "agents" | "jobs";
const tabs: WorkTab[] = ["all", "todos", "agents", "jobs"];
export interface WorkEntry {
	value: string; label: string; description?: string;
	kind: "todo" | "agent" | "job" | "queued" | "history" | "empty";
	id?: string; host?: string; pane?: string; detail?: string; running?: boolean;
	/** The cleaned board row this entry draws, for agents, jobs, and queued work. */
	row?: Row;
}
export interface WorkData extends WorkState { history: JobHistory }
export interface WorkCursor { tab: WorkTab; selected: string | undefined; history: boolean }
export interface WorkAction { action: "inspect" | "stop" | "cancel" | "send" | "refresh"; entry: WorkEntry; cursor: WorkCursor }

/** The same content in every tab, without task mutation or duplicate job rows. */
export function workEntries(data: WorkData, tab: WorkTab, history: boolean): WorkEntry[] {
	const todos: WorkEntry[] = data.todo && !data.todo.display.empty ? [{ value: "todos", label: clean(data.todo.display.summary), description: clean(data.todo.display.current), kind: "todo" }] : [];
	const canonicalHost = (label: string) => data.history.hosts?.find(host => host.label === label)?.id ?? label;
	// Board order: the task, then the groups in action order, as the collapsed board draws them.
	const rows: WorkEntry[] = GROUPS.flatMap(group => data.board.rows.filter(row => row.group === group)).map(raw => {
		const row = cleanRow(raw);
		return { value: `${raw.kind}:${canonicalHost(raw.host)}:${raw.id}`, id: raw.id, host: canonicalHost(raw.host), ...(raw.pane ? { pane: raw.pane } : {}), kind: raw.group === "waiting" ? "queued" : raw.kind === "job" ? "job" : "agent", label: `${raw.group === "attention" || raw.group === "to_land" ? "!" : "●"} ${row.name} · ${row.state}`, description: row.host, running: raw.group === "running", row };
	});
	const visible = tab === "todos" ? todos : tab === "agents" ? rows.filter(row => row.value.startsWith("agent:")) : tab === "jobs" ? rows.filter(row => row.value.startsWith("job:")) : [...todos, ...rows];
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
	private selected = 0;
	private entries: WorkEntry[] = [];
	private data: WorkData;
	readonly cursor: WorkCursor;
	private theme: Theme;
	private todoView: TodoView | undefined;
	private publisher: string | undefined;
	private readonly todos: WorkTodo | undefined;
	private readonly done: (action: WorkAction | undefined) => void;
	private readonly renderRequest: () => void;
	private readonly rows: () => number;
	private height = 0;

	constructor(data: WorkData, cursor: WorkCursor, theme: Theme, done: (action: WorkAction | undefined) => void, request: () => void, rows: () => number, todos?: WorkTodo) {
		this.todos = todos;
		this.data = data; this.cursor = cursor; this.theme = theme; this.done = done; this.renderRequest = request; this.rows = rows;
		this.rebuild();
	}
	update(data: WorkData): void {
		this.data = data;
		if (this.publisher !== data.todo?.publisher) { this.todoView?.dispose(); this.todoView = undefined; }
		this.publisher = data.todo?.publisher;
		this.rebuild(); this.renderRequest();
	}
	setTheme(theme: Theme): void { this.theme = theme; this.invalidate(); }
	dispose(): void { this.todoView?.dispose(); this.todoView = undefined; }
	private hosted(): TodoView | undefined {
		if (!this.todoView && this.data.todo && this.todos) {
			const panel = this;
			this.publisher = this.data.todo.publisher;
			this.todoView = this.todos.createView({
				theme: { fg: (color, text) => panel.theme.fg(color, text), bold: text => panel.theme.bold(text) },
				height: () => panel.listHeight(), requestRender: this.renderRequest,
			});
		}
		return this.todoView;
	}
	// Half the terminal: the header, the view line, this list, its scroll line, and two hint lines.
	private listHeight(): number { return Math.max(1, Math.min(12, Math.floor(this.rows() / 2) - 5)); }
	private rebuild(preserveSelection = true): void {
		if (preserveSelection) this.cursor.selected = this.entries[this.selected]?.value ?? this.cursor.selected;
		this.entries = workEntries(this.data, this.cursor.tab, this.cursor.history);
		this.height = this.listHeight();
		this.selected = Math.max(0, this.entries.findIndex(entry => entry.value === this.cursor.selected));
	}
	private move(step: number): void {
		const count = this.entries.length;
		this.selected = (this.selected + step + count) % count;
		this.cursor.selected = this.entries[this.selected]?.value;
	}
	private act(action: WorkAction["action"]): void {
		const entry = this.entries[this.selected];
		if (!entry) return;
		if (entry.kind === "todo" && action === "inspect") { this.cursor.tab = "todos"; this.rebuild(false); this.renderRequest(); return; }
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
		if (this.cursor.tab === "todos" && input !== "r") { this.hosted()?.handleInput(input); return; }
		if (input === "r") this.act("refresh");
		else if (input === "s") this.act("stop");
		else if (input === "c") this.act("cancel");
		else if (input === "m") this.act("send");
		else {
			const keys = getKeybindings();
			if (keys.matches(input, "tui.select.up")) this.move(-1);
			else if (keys.matches(input, "tui.select.down")) this.move(1);
			else if (keys.matches(input, "tui.select.confirm")) this.act("inspect");
			else if (keys.matches(input, "tui.select.cancel")) this.done(undefined);
		}
		this.renderRequest();
	}
	invalidate(): void { this.todoView?.invalidate(); }
	/** The entries in the collapsed board's style, each selectable line tagged with its entry. */
	private lines(width: number): { text: string; entry?: number }[] {
		const lines: { text: string; entry?: number }[] = [];
		const columns = boardColumns(this.entries.flatMap(entry => entry.row ? [entry.row] : []), width);
		let group: Group | undefined;
		this.entries.forEach((entry, index) => {
			const mark = index === this.selected ? this.theme.fg("accent", "→ ") : "  ";
			if (entry.row) {
				if (entry.row.group !== group) { group = entry.row.group; lines.push({ text: groupTitle(group, this.theme) }); }
				lines.push({ text: boardRow(entry.row, columns, width, this.theme, mark), entry: index });
				return;
			}
			group = undefined;
			const task = entry.kind === "todo" ? taskLines(this.data.todo, width, this.theme, mark) : [];
			if (task.length) { lines.push({ text: task[0]! }, ...task.slice(1).map(text => ({ text, entry: index }))); return; }
			const label = entry.kind === "empty" ? this.theme.fg("muted", entry.label) : entry.label;
			lines.push({ text: `${mark}${label}${entry.description ? `  ${this.theme.fg("dim", entry.description)}` : ""}`, entry: index });
		});
		return lines;
	}
	/** At most `height` lines around the selection, with a position line when some are hidden. */
	private visible(width: number): string[] {
		const lines = this.lines(width);
		if (lines.length <= this.height) return lines.map(line => line.text);
		const at = lines.findIndex(line => line.entry === this.selected);
		const start = Math.max(0, Math.min(at - Math.floor(this.height / 2), lines.length - this.height));
		return [...lines.slice(start, start + this.height).map(line => line.text), this.theme.fg("dim", `  (${this.selected + 1}/${this.entries.length})`)];
	}
	render(width: number): string[] {
		if (this.height !== this.listHeight()) this.rebuild();
		const views = tabs.map(tab => tab === this.cursor.tab ? this.theme.fg("accent", `[${tab.toUpperCase()}]`) : this.theme.fg("muted", tab)).join(this.theme.fg("dim", " · "));
		const body = this.cursor.tab === "todos" ? this.hosted()?.render(width) ?? ["Todo view unavailable"] : this.visible(width);
		return [boardHeader(this.data.board, width, this.theme), ` ${views}`, ...body, this.theme.fg("dim", " Alt+W collapse · Alt+J/K select · Alt+] view · Alt+I inspect"), this.theme.fg("dim", " Alt+S stop · Alt+C cancel · Alt+M message · Alt+R refresh")].map(line => truncateToWidth(line, width));
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

/** Explicit actions may open confirmation/input dialogs; the work widget stays inline. */
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

/** Details occupy the same above-editor widget, never an overlay. */
export class WorkDetail {
	private readonly text: Text;
	private readonly scroll: ScrollView;
	private readonly title: string;
	private theme: Theme;
	private readonly rows: () => number;
	private readonly request: () => void;
	constructor(title: string, body: string, theme: Theme, rows: () => number, request: () => void) {
		this.title = title; this.theme = theme; this.rows = rows; this.request = request;
		this.text = new Text(safeOutput(body) || "No output yet.", 0, 0);
		this.scroll = new ScrollView(this.text, { overscroll: "contain", scrollbar: "hidden" });
	}
	render(width: number): string[] {
		const height = Math.max(1, Math.min(12, Math.floor(this.rows() / 2) - 2));
		this.scroll.updateLayout(this.text.render(width).length, height, this.request);
		return [this.theme.fg("accent", truncateToWidth(this.title, width)), ...this.scroll.render(width).slice(this.scroll.scrollTop, this.scroll.scrollTop + height), this.theme.fg("dim", truncateToWidth("Alt+J/K scroll · Alt+I back · Alt+W collapse", width))];
	}
	setTheme(theme: Theme): void { this.theme = theme; this.invalidate(); }
	invalidate(): void { this.text.invalidate(); this.scroll.invalidate(); }
	handleInput(input: string): void {
		if (matchesKey(input, Key.up)) this.scroll.scrollBy(-1);
		if (matchesKey(input, Key.down)) this.scroll.scrollBy(1);
		this.request();
	}
}
