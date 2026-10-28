import assert from "node:assert/strict";
import { test } from "node:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import { age, type Board, parseBoard, renderBoard } from "./board.ts";

const plain = { fg: (_c: string, t: string) => t, bold: (t: string) => t };
const board: Board = {
	hosts: [
		{ label: "local", service: "up", reachable: true, busy: 1, cores: 12, eligible: true },
		{ label: "netcup", service: "up", reachable: true, busy: 2, cores: 16, eligible: true },
		{ label: "momokaya-2", service: "down", reachable: false, cores: 7, eligible: false, retry_ms: 50_000 },
	],
	rows: [
		{ group: "running", kind: "agent", id: "a", name: "api-dups", what: "pi · astra→opus 5.5", host: "momokaya-2", state: "working", activity: "edit src/api/dups.rs", age_ms: 14 * 60_000 },
		{ group: "to_land", kind: "agent", id: "b", name: "web-tests", what: "pi · opus 5.5", host: "netcup", state: "done", activity: "+2 commits · 4 files", age_ms: 3 * 60_000, ahead: 2 },
		{ group: "running", kind: "job", id: "c", name: "cargo test", what: "job", host: "local", state: "running", activity: "cargo test", age_ms: 4 * 60_000 },
		{ group: "attention", kind: "agent", id: "d", name: "i18n-catalog", what: "pi · sol", host: "netcup", state: "stalled", activity: "quiet 11m · last: rg \"t(\" web/\u001b[31m", age_ms: 22 * 60_000 },
		{ group: "waiting", kind: "agent", id: "e", name: "rust-lints", what: "pi · sol", host: "netcup", state: "queued", activity: "for netcup (1.6 load/core)", age_ms: 60_000 },
	],
};

test("groups rows in action order with one header and meters for each host", () => {
	const lines = renderBoard(board, 160, plain);
	assert.match(lines[0]!, /^ dogsled {2}1 ready to merge · 1 needs you · 2 working · 1 queued +local ▰▱▱▱ 1\/12 {2}netcup ▰▱▱▱ 2\/16 {2}momokaya-2 \?$/);
	const titles = lines.filter(l => /^ [A-Z]/.test(l)).map(l => l.trim());
	assert.deepEqual(titles, ["READY TO MERGE", "NEEDS YOU", "WORKING", "QUEUED"]);
	assert.ok(lines.some(l => l.includes("● web-tests") && l.includes("done 3m")));
	assert.ok(lines.some(l => l.includes("▸ cargo test") && l.includes("job")));
	// Control characters from a transcript never reach the terminal.
	assert.ok(lines.every(l => !l.includes("\u001b")));
});

test("fits 60, 100, and 160 columns, dropping activity and then the host first", () => {
	for (const width of [48, 60, 100, 160]) {
		const lines = renderBoard(board, width, plain);
		for (const line of lines) assert.ok(visibleWidth(line) <= width, `${width}: ${line}`);
		const row = lines.find(l => l.includes("api-dups"))!;
		assert.ok(row.includes("pi · astra→opus 5.5"), `${width}: ${row}`);
		assert.ok(row.endsWith("14m"), `${width}: ${row}`);
		assert.equal(row.includes("momokaya-2"), width >= 60, `${width}: ${row}`);
		assert.equal(row.includes("edit src/api"), width >= 100, `${width}: ${row}`);
	}
	// Too narrow for the meters: the header keeps only the counts.
	assert.ok(!renderBoard(board, 60, plain)[0]!.includes("netcup"));
});

test("every lifecycle state and stalled renders without state validation", () => {
	for (const state of ["queued", "starting", "working", "waiting", "asking", "blocked", "finishing", "done", "stopped", "lost", "unknown", "stalled", "future-state"]) {
		const input = { hosts: [], rows: [{ ...board.rows[0]!, state, activity: "" }] };
		const decoded = parseBoard(JSON.parse(JSON.stringify(input)));
		assert.equal(decoded.rows[0]!.state, state);
		assert.ok(renderBoard(decoded, 120, plain).some(line => line.includes("api-dups") && line.includes(state)), state);
	}
});

test("unknown groups retain their rows under NEEDS YOU", () => {
	for (const group of ["future-group", "constructor", "__proto__", ""]) {
		const input = parseBoard({ hosts: [], rows: [{ ...board.rows[0]!, group, state: "future-state" }] });
		const lines = renderBoard(input, 120, plain);
		assert.match(lines[0]!, /1 needs you/);
		assert.equal(lines[1], " NEEDS YOU");
		assert.ok(lines[2]!.includes("! api-dups"), group);
	}
});

test("an empty board draws nothing", () => {
	assert.deepEqual(renderBoard({ hosts: board.hosts, rows: [] }, 100, plain), []);
	assert.deepEqual([age(45_000), age(14 * 60_000), age(3 * 3_600_000), age(2 * 86_400_000)], ["45s", "14m", "3h", "2d"]);
});
