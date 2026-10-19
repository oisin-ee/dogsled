// The Pi reader of this pane's dogsled inbox. Dogsled never types into a terminal: this
// extension claims records, adds them to the session as one custom message, and
// acknowledges them when Pi ends that message, after it is in the transcript. A record that
// is claimed and never acknowledged is offered again by dogsled with backoff, so a crash or
// an aborted run repeats a message and never loses it.
import { watch, type FSWatcher } from "node:fs";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

export const MESSAGE_TYPE = "dogsled";

export interface Delivered {
	id: string;
	kind: string;
	sender: string;
	text: string;
	created_ms: number;
}

export interface Details {
	records: Delivered[];
}

interface Claimed extends Delivered {
	/** What the agent reads: the sender, the kind, and the text. */
	message: string;
}

type Exec = (args: string[]) => Promise<string>;

function parseClaimed(stdout: string): Claimed[] {
	const value: unknown = JSON.parse(stdout);
	if (!Array.isArray(value)) throw new Error("dogsled inbox claim returned no list");
	return value.map(item => {
		const r = item as Partial<Claimed> | null;
		if (typeof r?.id !== "string" || typeof r.message !== "string" || typeof r.kind !== "string") {
			throw new Error("dogsled inbox claim returned a record without id, kind, or message");
		}
		return { id: r.id, kind: r.kind, sender: String(r.sender ?? "dogsled"), text: String(r.text ?? ""), created_ms: Number(r.created_ms ?? 0), message: r.message };
	});
}

/** The ids a finished message carries, when it is one of ours. */
export function deliveredIds(message: unknown): string[] {
	const m = message as { role?: unknown; customType?: unknown; details?: Partial<Details> } | null;
	if (m?.role !== "custom" || m.customType !== MESSAGE_TYPE || !Array.isArray(m.details?.records)) return [];
	return m.details.records.map(r => r.id).filter((id): id is string => typeof id === "string");
}

export class InboxReader {
	private watcher: FSWatcher | undefined;
	private running: Promise<void> | undefined;
	private again = false;
	private stopped = true;
	private failure: string | undefined;
	/** Sent to Pi and not yet ended. A claim that returns one again, because its lease ran
	 * out while Pi held it in the steering queue, does not send it twice. */
	private readonly inFlight = new Set<string>();

	private readonly pi: Pick<ExtensionAPI, "sendMessage">;
	private readonly exec: Exec;

	constructor(pi: Pick<ExtensionAPI, "sendMessage">, exec: Exec) {
		this.pi = pi;
		this.exec = exec;
	}

	/** Register as this pane's reader, deliver what is waiting, and watch for more. */
	async start(ctx: ExtensionContext): Promise<void> {
		this.stop();
		this.stopped = false;
		await this.drain(ctx);
		try {
			const where = JSON.parse(await this.exec(["inbox", "path", "--json"])) as { dir?: unknown; file?: unknown };
			if (typeof where.dir !== "string" || typeof where.file !== "string") throw new Error("dogsled inbox path returned no dir or file");
			const file = where.file;
			// Each write replaces the file, so the directory is watched, not the file.
			this.watcher = watch(where.dir, { persistent: false }, (_event, name) => {
				if (name === file) void this.drain(ctx);
			});
			this.watcher.on("error", () => { this.watcher?.close(); this.watcher = undefined; });
		} catch {
			// The periodic refresh still drains the inbox; without a watch, delivery waits for it.
		}
	}

	stop(): void {
		this.stopped = true;
		this.watcher?.close();
		this.watcher = undefined;
		this.inFlight.clear();
	}

	/** Claim and inject. Calls that arrive while one runs fold into one rerun. */
	drain(ctx: ExtensionContext): Promise<void> {
		if (this.stopped) return Promise.resolve();
		if (this.running) {
			this.again = true;
			return this.running;
		}
		this.running = (async () => {
			do {
				this.again = false;
				await this.once(ctx);
			} while (this.again && !this.stopped);
		})().finally(() => { this.running = undefined; });
		return this.running;
	}

	/** Pi ended `message`: if it is ours, its records reached the transcript. An ack is
	 * idempotent, so a record sent twice is acknowledged by whichever copy ends first. */
	async ended(message: unknown): Promise<void> {
		const ids = deliveredIds(message);
		if (ids.length === 0) return;
		await this.exec(["inbox", "ack", ...ids]);
		for (const id of ids) this.inFlight.delete(id);
	}

	/** Pi will not continue on its own. A message it never ended, because an abort dropped
	 * the steering queue, is offered again once its lease runs out. */
	settled(): void {
		this.inFlight.clear();
	}

	private async once(ctx: ExtensionContext): Promise<void> {
		let records: Claimed[];
		try {
			records = parseClaimed(await this.exec(["inbox", "claim", "--harness", "pi", "--json"]));
			this.failure = undefined;
		} catch (error) {
			// Say so once, not on every refresh; dogsled keeps the records until a claim succeeds.
			const text = `dogsled inbox: ${error instanceof Error ? error.message : String(error)}`;
			if (text !== this.failure && ctx.hasUI) ctx.ui.notify(text, "warning");
			this.failure = text;
			return;
		}
		const fresh = records.filter(r => !this.inFlight.has(r.id));
		if (fresh.length === 0 || this.stopped) return;
		for (const r of fresh) this.inFlight.add(r.id);
		const message = {
			customType: MESSAGE_TYPE,
			content: fresh.map(r => r.message).join("\n\n"),
			display: true,
			details: { records: fresh.map(({ message: _, ...r }) => r) } satisfies Details,
		};
		// A running agent reads it at its next step; an idle one starts a turn for it.
		if (ctx.isIdle()) this.pi.sendMessage(message, { triggerTurn: true });
		else this.pi.sendMessage(message, { deliverAs: "steer" });
	}
}
