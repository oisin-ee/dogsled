import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { age, clean, type Board, type Paint, type Row } from "./board.ts";
import type { TodoSnapshot } from "./work-todo.ts";

export interface WorkState {
	board: Board;
	todo: TodoSnapshot | undefined;
	error: string | undefined;
}

/** A small, persistent readout: names and current work, not just counts. */
export function workStrip(state: WorkState, width: number, paint: Paint): string[] {
	const { board, todo, error } = state;
	if (!board.rows.length && !todo?.tasks.length && !error) return [];
	const lines = [paint.fg("dim", "WORK · Alt+W expand")];
	if (error) lines.push(paint.fg("warning", `! unavailable · ${clean(error)}`));
	const tasks = todo?.tasks ?? [];
	const active = tasks.find(task => task.status === "in_progress");
	const next = tasks.find(task => task.status === "pending");
	const current = active ?? next;
	if (current) lines.push(`${paint.fg("accent", active ? "→ Task" : "· Next")}  ${clean(current.content)}`);
	else if (tasks.length) lines.push(paint.fg("muted", `Tasks · ${tasks.filter(task => task.status === "completed").length}/${tasks.length} completed`));
	const priority = (row: Row) => row.group === "attention" || row.group === "to_land" ? 0 : row.group === "running" ? 1 : 2;
	for (const [label, members] of [
		["Agents", board.rows.filter(row => row.kind !== "job")],
		["Jobs", board.rows.filter(row => row.kind === "job")],
	] as const) {
		const rows = [...members].sort((a, b) => priority(a) - priority(b));
		if (!rows.length) continue;
		let line = `${label}  `;
		let shown = 0;
		for (const row of rows) {
			const warning = priority(row) === 0;
			const entry = `${warning ? "! " : ""}${clean(row.name)} · ${clean(row.state)} · ${age(row.age_ms)}`;
			const remaining = rows.length - shown - 1;
			const suffix = remaining ? `  +${remaining} more` : "";
			const join = shown ? "  |  " : "";
			if (shown && visibleWidth(line + join + entry + suffix) > width) break;
			const room = Math.max(0, width - visibleWidth(line + join + suffix));
			line += join + paint.fg(warning ? "warning" : "muted", truncateToWidth(entry, room));
			shown++;
		}
		if (shown < rows.length) line += paint.fg("dim", `  +${rows.length - shown} more`);
		lines.push(line);
	}
	return lines.map(line => truncateToWidth(line, width));
}

/** A plain readout also works in modes where terminal UI is unavailable. */
export function workText(state: WorkState): string {
	const lines = ["WORK · todos: Pi branch · agents/jobs: Herdr owner pane"];
	if (state.error) lines.push(state.error);
	for (const task of state.todo?.tasks ?? []) lines.push(`${task.status}: ${clean(task.content)}`);
	for (const row of state.board.rows) lines.push(`${clean(row.name)} · ${clean(row.state)} · ${clean(row.host)}`);
	if (lines.length === 1) lines.push("No work.");
	return lines.join("\n");
}
