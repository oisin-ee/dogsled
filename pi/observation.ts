// The one publication client of a Pi session. `dogsled board --watch` subscribes to serve's
// publication and prints the owner's board and job history once per revision; this client
// only parses those frames. It never derives, probes, or polls state.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { type Board, clean, parseBoard } from "./board.ts";
import { type JobHistory, parseJobHistory } from "./work-history.ts";

export interface Frame { revision: number; board: Board; history: JobHistory }

/** A frame, or why the publication is unavailable. The last frame stays the caller's to keep. */
export type Observed = { frame: Frame; error?: undefined } | { frame?: undefined; error: string };

/** The parts of a child process this client uses, so tests can supply one. */
export interface Watcher {
	stdout: NodeJS.ReadableStream | null;
	stderr: NodeJS.ReadableStream | null;
	kill(): unknown;
	/** After exit and after its output is read, so no frame is lost. */
	once(event: "close", listener: (code: number | null) => void): unknown;
	once(event: "error", listener: (error: Error) => void): unknown;
}
export type Spawn = (binary: string, args: string[]) => Watcher;

const spawnWatcher: Spawn = (binary, args) => spawn(binary, args, { stdio: ["ignore", "pipe", "pipe"] });

/** One line of `dogsled board --watch --json`. */
export function parseFrame(line: string): Observed {
	let value: unknown;
	try { value = JSON.parse(line); } catch { return { error: "Dogsled observation sent an invalid frame" }; }
	if (!value || typeof value !== "object") return { error: "Dogsled observation sent an invalid frame" };
	const frame = value as Record<string, unknown>;
	if (typeof frame["revision"] !== "number") return { error: "Dogsled observation sent an invalid frame" };
	if (typeof frame["error"] === "string") return { error: clean(frame["error"]) };
	try {
		return { frame: { revision: frame["revision"], board: parseBoard(frame["board"]), history: parseJobHistory(frame["history"]) } };
	} catch (error) {
		return { error: clean(String(error)) };
	}
}

export class Observation {
	private watcher: Watcher | undefined;
	private generation = 0;
	private revision = 0;

	private readonly binary: string;
	private readonly owner: string;
	private readonly listener: (observed: Observed) => void;
	private readonly spawner: Spawn;

	constructor(binary: string, owner: string, listener: (observed: Observed) => void, spawner: Spawn = spawnWatcher) {
		this.binary = binary;
		this.owner = owner;
		this.listener = listener;
		this.spawner = spawner;
	}

	get running(): boolean { return this.watcher !== undefined; }

	/** Subscribe unless subscribed. After the watcher exits, the next call subscribes again. */
	start(): void {
		if (this.watcher) return;
		const generation = ++this.generation;
		const current = () => generation === this.generation;
		let watcher: Watcher;
		try {
			watcher = this.spawner(this.binary, ["board", "--watch", "--json", "--owner", this.owner]);
		} catch (error) {
			this.listener({ error: `Dogsled observation unavailable: ${clean(String(error))}` });
			return;
		}
		this.watcher = watcher;
		let stderr = "";
		watcher.stderr?.setEncoding("utf8");
		watcher.stderr?.on("data", (chunk: string) => { stderr = (stderr + chunk).slice(-2_000); });
		const lines = createInterface({ input: watcher.stdout! });
		lines.on("line", line => {
			if (!current()) return;
			const observed = parseFrame(line);
			// Revisions only move forward within one subscription.
			if (observed.frame) {
				if (observed.frame.revision <= this.revision) return;
				this.revision = observed.frame.revision;
			}
			this.listener(observed);
		});
		let done = false;
		const ended = (detail: string) => {
			if (done) return;
			done = true;
			lines.close();
			if (!current()) return;
			this.watcher = undefined;
			this.revision = 0;
			this.listener({ error: `Dogsled observation unavailable: ${clean(detail)}` });
		};
		watcher.once("close", code => ended(stderr.trim() || `dogsled board exited with ${code ?? "a signal"}`));
		watcher.once("error", error => ended(String(error)));
	}

	/** End the subscription. Frames and exits from it are ignored from now on. */
	stop(): void {
		this.generation++;
		this.revision = 0;
		const watcher = this.watcher;
		this.watcher = undefined;
		watcher?.kill();
	}
}
