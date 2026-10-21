// The board widget: what this agent started, grouped by what it must do next. All data comes
// from `dogsled board --json`; this file only lays it out for a terminal width.
import type { ThemeColor } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { createConnection } from "node:net";

/** A stale socket path is not a running Herdr session. Never probe outside Herdr. */
export async function herdrAvailable(env: NodeJS.ProcessEnv = process.env): Promise<boolean> {
	const path = env["HERDR_SOCKET_PATH"];
	if (env["HERDR_ENV"] !== "1" || !path) return false;
	return new Promise(resolve => {
		const socket = createConnection(path);
		const finish = (available: boolean) => { socket.destroy(); resolve(available); };
		socket.once("connect", () => finish(true));
		socket.once("error", () => finish(false));
		socket.setTimeout(1_000, () => finish(false));
	});
}

export type Group = "to_land" | "attention" | "running" | "waiting";

export interface Row {
	group: Group;
	kind: string;
	id: string;
	name: string;
	what: string;
	host: string;
	pane?: string;
	state: string;
	activity: string;
	age_ms: number;
	quiet_ms?: number;
	ahead?: number;
	dirty?: boolean;
}

export interface HostLine {
	label: string;
	service: string;
	reachable: boolean;
	busy?: number;
	cores: number;
	eligible: boolean;
	reason?: string;
	retry_ms?: number;
}

export interface Board {
	hosts: HostLine[];
	rows: Row[];
}

/** The theme methods the board uses, so tests can paint with plain text. */
export interface Paint {
	fg(color: ThemeColor, text: string): string;
	bold(text: string): string;
}

const TITLES: Record<Group, string> = { to_land: "TO LAND", attention: "ATTENTION", running: "RUNNING", waiting: "WAITING" };
/** Groups in action order: what the owner must do first comes first. */
export const GROUPS: readonly Group[] = ["to_land", "attention", "running", "waiting"];
const ORDER = GROUPS;
const COUNTS: Record<Group, string> = { to_land: "to land", attention: "need attention", running: "running", waiting: "waiting" };

export function parseBoard(value: unknown): Board {
	const data = value as Partial<Board> | null;
	if (!data || !Array.isArray(data.hosts) || !Array.isArray(data.rows)) throw new Error("dogsled board returned an invalid board");
	return data as Board;
}

/** `45s`, `14m`, `3h`, `2d`. */
export function age(ms: number): string {
	const s = Math.max(0, Math.round(ms / 1000));
	if (s < 60) return `${s}s`;
	if (s < 3600) return `${Math.floor(s / 60)}m`;
	if (s < 86_400) return `${Math.floor(s / 3600)}h`;
	return `${Math.floor(s / 86_400)}d`;
}

/** Terminal text from dogsled is data: control characters never reach the screen. */
export function clean(text: string): string {
	return text.replace(/[\x00-\x1f\x7f-\x9f]/g, " ");
}

function icon(row: Row): [string, ThemeColor] {
	if (row.group === "to_land") return ["●", "success"];
	if (row.group === "attention") return ["!", "warning"];
	if (row.group === "waiting") return ["⋯", "muted"];
	if (row.kind === "job") return ["▸", "accent"];
	return row.state === "working" ? ["◐", "accent"] : ["○", "muted"];
}

function meter(host: HostLine, paint: Paint): string {
	if (!host.reachable || host.busy === undefined || host.cores === 0) {
		return `${host.label} ${paint.fg("error", "?")}`;
	}
	// Any load shows: a block is a quarter of the cores, rounded up.
	const filled = Math.min(4, Math.ceil((host.busy / host.cores) * 4));
	const bar = "▰".repeat(filled) + "▱".repeat(4 - filled);
	const color: ThemeColor = host.eligible ? "success" : "warning";
	return `${host.label} ${paint.fg(color, bar)} ${paint.fg("dim", `${Math.round(host.busy)}/${host.cores}`)}`;
}

function pad(text: string, width: number): string {
	return truncateToWidth(text, width, "…", true);
}

/** A row whose transcript text is safe to draw. */
export function cleanRow(row: Row): Row {
	return { ...row, name: clean(row.name), what: clean(row.what), host: clean(row.host), state: clean(row.state), activity: clean(row.activity || row.state) };
}

/** Column widths shared by every row drawn together, so the columns line up. */
export interface Columns {
	name: number;
	what: number;
	host: number;
	activity: number;
	showHost: boolean;
	showActivity: boolean;
}

function right(row: Row): string {
	return row.group === "to_land" ? `${row.state} ${age(row.age_ms)}` : age(row.age_ms);
}

/** Activity goes first on a narrow terminal, then the host column, so the name and the
 * harness stay readable. */
export function boardColumns(rows: Row[], width: number): Columns {
	if (rows.length === 0) return { name: 0, what: 0, host: 0, activity: 0, showHost: false, showActivity: false };
	const name = Math.min(24, Math.max(...rows.map(r => visibleWidth(r.name))));
	const what = Math.min(22, Math.max(...rows.map(r => visibleWidth(r.what))));
	const host = Math.min(14, Math.max(...rows.map(r => visibleWidth(r.host))));
	const rightW = Math.max(...rows.map(r => visibleWidth(right(r))));
	const fixed = 4 + name + 2 + what + 2 + rightW;
	const showHost = fixed + host + 2 <= width;
	const activity = width - fixed - (showHost ? host + 2 : 0) - 2;
	return { name, what, host, activity, showHost, showActivity: activity >= 16 };
}

/** `dogsled  <counts>` on the left and one load meter per host on the right. */
export function boardHeader(board: Board, width: number, paint: Paint): string {
	const counts = ORDER.map(g => [g, board.rows.filter(r => r.group === g).length] as const)
		.filter(([, n]) => n > 0)
		.map(([g, n]) => `${n} ${COUNTS[g]}`)
		.join(" · ");
	const left = ` ${paint.bold(paint.fg("accent", "dogsled"))}${counts ? `  ${counts}` : ""}`;
	const meters = board.hosts.map(h => meter(h, paint)).join("  ");
	const gap = width - visibleWidth(left) - visibleWidth(meters) - 1;
	return truncateToWidth(gap >= 2 ? `${left}${" ".repeat(gap)}${meters}` : left, width);
}

export function groupTitle(group: Group, paint: Paint): string {
	return ` ${paint.fg(group === "attention" ? "warning" : "muted", TITLES[group])}`;
}

/** One cleaned row. `prefix` is two columns wide: blank, or a selection marker. */
export function boardRow(row: Row, columns: Columns, width: number, paint: Paint, prefix = "  "): string {
	const [glyph, color] = icon(row);
	let line = `${prefix}${paint.fg(color, glyph)} ${pad(row.name, columns.name)}  ${paint.fg("muted", pad(row.what, columns.what))}`;
	if (columns.showHost) line += `  ${pad(row.host, columns.host)}`;
	if (columns.showActivity) line += `  ${paint.fg("dim", pad(row.activity, columns.activity))}`;
	const tail = right(row);
	line += " ".repeat(Math.max(2, width - visibleWidth(line) - visibleWidth(tail))) + paint.fg("dim", tail);
	return truncateToWidth(line, width);
}

/** The board as lines no wider than `width`. */
export function renderBoard(board: Board, width: number, paint: Paint): string[] {
	if (board.rows.length === 0) return [];
	const rows = board.rows.map(cleanRow);
	const columns = boardColumns(rows, width);
	const lines = [boardHeader(board, width, paint)];
	for (const group of ORDER) {
		const members = rows.filter(r => r.group === group);
		if (members.length === 0) continue;
		lines.push(groupTitle(group, paint));
		for (const row of members) lines.push(boardRow(row, columns, width, paint));
	}
	return lines.map(line => truncateToWidth(line, width));
}
