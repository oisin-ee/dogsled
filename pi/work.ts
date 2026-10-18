import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { clean, type Board, type Paint } from "./board.ts";
import type { TodoSnapshot } from "./work-todo.ts";

export interface WorkState {
	board: Board;
	todo: TodoSnapshot | undefined;
	error: string | undefined;
}

/** One bounded row. Attention survives before progress and descriptive labels. */
export function workStrip(state: WorkState, width: number, paint: Paint): string[] {
	const { board, todo, error } = state;
	if (!board.rows.length && !todo?.tasks.length && !error) return [];
	const attention = board.rows.filter(row => row.group === "attention" || row.group === "to_land");
	const urgent = attention.length ? `!${attention.length}` : error ? "!" : "";
	const prefix = `WORK${urgent ? ` ${urgent}` : ""}`;
	const counts: string[] = [];
	if (todo?.tasks.length) counts.push(`${todo.tasks.filter(task => task.status === "completed").length}/${todo.tasks.length} todos`);
	const jobs = board.rows.filter(row => row.kind === "job").length;
	const agents = board.rows.filter(row => row.kind !== "job").length;
	if (agents) counts.push(`${agents} agent${agents === 1 ? "" : "s"}`);
	if (jobs) counts.push(`${jobs} job${jobs === 1 ? "" : "s"}`);
	const item = attention[0];
	const detail = error ? `unavailable · ${clean(error)}` : item ? `${clean(item.name)} · ${clean(item.state)}`
		: todo?.tasks.find(task => task.status === "in_progress")?.content;
	let line = prefix;
	for (const count of counts) {
		const next = `${line} · ${count}`;
		if (visibleWidth(next) + 8 <= width) line = next;
	}
	if (detail && width - visibleWidth(line) > 12) line += ` · ${truncateToWidth(clean(detail), width - visibleWidth(line) - 11)}`;
	if (visibleWidth(line) + 8 <= width) line += "  /work";
	return [truncateToWidth(paint.fg(urgent ? "warning" : "accent", line), width)];
}

/** A plain readout also works in modes where custom terminal UI is unavailable. */
export function workText(state: WorkState): string {
	const lines = ["WORK · todos: Pi branch · agents/jobs: Herdr owner pane"];
	if (state.error) lines.push(state.error);
	for (const task of state.todo?.tasks ?? []) lines.push(`${task.status}: ${clean(task.content)}`);
	for (const row of state.board.rows) lines.push(`${clean(row.name)} · ${clean(row.state)} · ${clean(row.host)}`);
	if (lines.length === 1) lines.push("No work.");
	return lines.join("\n");
}
