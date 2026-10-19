import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { clean, renderBoard, boardHeader, type Board, type Paint } from "./board.ts";
import type { TodoSnapshot } from "./work-todo.ts";

export interface WorkState {
	board: Board;
	todo: TodoSnapshot | undefined;
	error: string | undefined;
}

/** The current task in board style: a TASK title, then the task with pi-todo's summary at the right. */
export function taskLines(todo: TodoSnapshot | undefined, width: number, paint: Paint, prefix = "  "): string[] {
	if (!todo || todo.display.empty) return [];
	const summary = clean(todo.display.summary);
	const current = clean(todo.display.current);
	const title = ` ${paint.fg("muted", "TASK")}`;
	if (!current) return [title, truncateToWidth(`${prefix}${paint.fg("muted", summary)}`, width)];
	const tail = paint.fg("dim", summary);
	const room = width - visibleWidth(prefix) - 2 - 2 - visibleWidth(tail);
	const line = room >= 12
		? `${prefix}${paint.fg("accent", "◆")} ${truncateToWidth(current, room, "…", true)}  ${tail}`
		: `${prefix}${paint.fg("accent", "◆")} ${current}`;
	return [title, truncateToWidth(line, width)];
}

/** The persistent board above the editor: the dogsled board with the current task on top. */
export function workBoard(state: WorkState, width: number, paint: Paint): string[] {
	const { board, todo, error } = state;
	const task = taskLines(todo, width, paint);
	if (!board.rows.length && !task.length && !error) return [];
	const rows = renderBoard(board, width, paint);
	const lines = [rows[0] ?? boardHeader(board, width, paint)];
	if (error) lines.push(truncateToWidth(` ${paint.fg("warning", `! unavailable · ${clean(error)}`)}`, width));
	lines.push(...task, ...rows.slice(1));
	return lines.map(line => truncateToWidth(line, width));
}

/** A plain readout also works in modes where terminal UI is unavailable. */
export function workText(state: WorkState): string {
	const lines = ["WORK · todos: Pi branch · agents/jobs: Herdr owner pane"];
	if (state.error) lines.push(state.error);
	if (state.todo && !state.todo.display.empty) lines.push(...state.todo.display.text.split("\n").map(clean));
	for (const row of state.board.rows) lines.push(`${clean(row.name)} · ${clean(row.state)} · ${clean(row.host)}`);
	if (lines.length === 1) lines.push("No work.");
	return lines.join("\n");
}
