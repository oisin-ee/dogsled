// The board widget: what this agent started, grouped by what it must do next. All data comes
// from `wrangle board --json`; this file only lays it out for a terminal width.
import type { ThemeColor } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

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
const ORDER: Group[] = ["to_land", "attention", "running", "waiting"];
const COUNTS: Record<Group, string> = { to_land: "to land", attention: "need attention", running: "running", waiting: "waiting" };

export function parseBoard(value: unknown): Board {
	const data = value as Partial<Board> | null;
	if (!data || !Array.isArray(data.hosts) || !Array.isArray(data.rows)) throw new Error("wrangle board returned an invalid board");
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

/** Terminal text from wrangle is data: control characters never reach the screen. */
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

/** The board as lines no wider than `width`. Activity goes first on a narrow terminal, then
 * the host column, so the name and the harness stay readable. */
export function renderBoard(board: Board, width: number, paint: Paint): string[] {
	if (board.rows.length === 0) return [];
	const counts = ORDER.map(g => [g, board.rows.filter(r => r.group === g).length] as const)
		.filter(([, n]) => n > 0)
		.map(([g, n]) => `${n} ${COUNTS[g]}`)
		.join(" · ");
	const left = ` ${paint.bold(paint.fg("accent", "wrangle"))}  ${counts}`;
	const meters = board.hosts.map(h => meter(h, paint)).join("  ");
	const gap = width - visibleWidth(left) - visibleWidth(meters) - 1;
	const lines = [gap >= 2 ? `${left}${" ".repeat(gap)}${meters}` : left];

	const rows = board.rows.map(r => ({ ...r, name: clean(r.name), what: clean(r.what), host: clean(r.host), activity: clean(r.activity || r.state) }));
	const nameW = Math.min(24, Math.max(...rows.map(r => visibleWidth(r.name))));
	const whatW = Math.min(22, Math.max(...rows.map(r => visibleWidth(r.what))));
	const hostW = Math.min(14, Math.max(...rows.map(r => visibleWidth(r.host))));
	const right = (r: Row) => r.group === "to_land" ? `${r.state} ${age(r.age_ms)}` : age(r.age_ms);
	const rightW = Math.max(...rows.map(r => visibleWidth(right(r))));
	const fixed = 4 + nameW + 2 + whatW + 2 + rightW;
	const showHost = fixed + hostW + 2 <= width;
	const activityW = width - fixed - (showHost ? hostW + 2 : 0) - 2;
	const showActivity = activityW >= 16;

	for (const group of ORDER) {
		const members = rows.filter(r => r.group === group);
		if (members.length === 0) continue;
		lines.push(` ${paint.fg(group === "attention" ? "warning" : "muted", TITLES[group])}`);
		for (const row of members) {
			const [glyph, color] = icon(row);
			let line = `  ${paint.fg(color, glyph)} ${pad(row.name, nameW)}  ${paint.fg("muted", pad(row.what, whatW))}`;
			if (showHost) line += `  ${pad(row.host, hostW)}`;
			if (showActivity) line += `  ${paint.fg("dim", pad(row.activity, activityW))}`;
			const used = visibleWidth(line);
			const tail = right(row);
			line += " ".repeat(Math.max(2, width - used - visibleWidth(tail))) + paint.fg("dim", tail);
			lines.push(line);
		}
	}
	return lines.map(line => truncateToWidth(line, width));
}
