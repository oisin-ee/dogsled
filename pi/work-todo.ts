import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { randomUUID } from "node:crypto";

export interface WorkTask { content: string; status: "pending" | "in_progress" | "completed" | "abandoned" }
export interface TodoSnapshot {
	version: 1;
	presenter: string;
	request: string;
	publisher: string;
	revision: number;
	session: string;
	branch: string | null;
	tasks: WorkTask[];
}

type Bus = ExtensionAPI["events"];
function record(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null; }

/** Validate the independently installed provider's wire envelope, not rendered widget text. */
export function snapshot(value: unknown): TodoSnapshot | undefined {
	if (!record(value) || value["version"] !== 1 || typeof value["presenter"] !== "string" || typeof value["request"] !== "string"
		|| typeof value["publisher"] !== "string" || typeof value["session"] !== "string"
		|| (value["branch"] !== null && typeof value["branch"] !== "string")
		|| !Number.isSafeInteger(value["revision"]) || Number(value["revision"]) < 1 || !Array.isArray(value["tasks"])) return undefined;
	if (!value["tasks"].every((task: unknown) => record(task) && typeof task["content"] === "string"
		&& ["pending", "in_progress", "completed", "abandoned"].includes(String(task["status"])))) return undefined;
	return value as unknown as TodoSnapshot;
}

/** A session-scoped display lease. The provider remains the only task-state writer. */
export class WorkTodo {
	private readonly bus: Bus;
	private readonly changed: () => void;
	private readonly open: () => void;
	private subscriptions: (() => void)[] = [];
	private ctx: ExtensionContext | undefined;
	private presenter = "";
	private request = "";
	private retired = new Set<string>();
	current: TodoSnapshot | undefined;
	error: string | undefined;

	constructor(bus: Bus, changed: () => void, open: () => void) { this.bus = bus; this.changed = changed; this.open = open; }

	start(ctx: ExtensionContext): void {
		this.stop();
		if (ctx.mode !== "tui") return;
		this.ctx = ctx;
		this.presenter = randomUUID();
		this.retired.clear();
		this.subscriptions = [
			this.bus.on("oisin.todo.ready", value => {
				if (!record(value) || value["version"] !== 1 || typeof value["publisher"] !== "string") { this.reject(); return; }
				if (this.retired.has(value["publisher"])) return;
				this.ask();
			}),
			this.bus.on("oisin.todo.snapshot", value => this.receive(value)),
			this.bus.on("oisin.todo.stop", value => {
				if (!record(value) || !this.current || value["publisher"] !== this.current.publisher) return;
				this.retired.add(this.current.publisher);
				this.current = undefined;
				this.changed();
			}),
			this.bus.on("oisin.work.open", value => {
				if (record(value) && value["version"] === 1 && value["presenter"] === this.presenter && value["request"] === this.request) this.open();
			}),
		];
		this.ask();
	}

	private ask(): void {
		this.request = randomUUID();
		this.bus.emit("oisin.work.request", { version: 1, presenter: this.presenter, request: this.request });
	}

	private reject(): void {
		this.error = "Todo integration unavailable";
		this.release();
		this.changed();
	}

	private receive(value: unknown): void {
		if (!record(value) || value["presenter"] !== this.presenter || value["request"] !== this.request) return;
		const next = snapshot(value);
		if (!next) { this.reject(); return; }
		if (!this.ctx || next.session !== this.ctx.sessionManager.getSessionId() || next.branch !== this.ctx.sessionManager.getLeafId() || this.retired.has(next.publisher)) return;
		if (this.current?.publisher === next.publisher && next.revision <= this.current.revision) return;
		if (this.current && this.current.publisher !== next.publisher) this.retired.add(this.current.publisher);
		this.current = next;
		this.error = undefined;
		try {
			// The strip must render successfully before its provider relinquishes the widget.
			this.changed();
			this.bus.emit("oisin.work.claim", { version: 1, presenter: this.presenter, request: this.request, publisher: next.publisher, revision: next.revision });
		} catch (error) {
			this.error = "Todo integration unavailable";
			this.release();
			this.ctx.ui.notify(`Work display unavailable: ${String(error)}`, "error");
		}
	}

	private release(): void {
		if (this.presenter) this.bus.emit("oisin.work.release", { version: 1, presenter: this.presenter, request: this.request });
		this.current = undefined;
	}

	stop(): void {
		for (const off of this.subscriptions) off();
		this.subscriptions = [];
		this.release();
		this.ctx = undefined;
		this.error = undefined;
	}
}
