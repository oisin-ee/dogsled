import type { ExtensionAPI, ExtensionContext, Theme } from "@earendil-works/pi-coding-agent";
import { randomUUID } from "node:crypto";
import { rename, rm, writeFile } from "node:fs/promises";

/** The provider owns rendering and state interpretation; this is an in-process capability. */
export interface TodoViewHost {
	theme: Pick<Theme, "fg" | "bold">;
	height: () => number;
	requestRender: () => void;
}
export interface TodoView {
	render(width: number): string[];
	handleInput(input: string): void;
	invalidate(): void;
	update(state: unknown): void;
	dispose(): void;
}
export type TodoStatus = "in_progress" | "pending" | "completed" | "abandoned";
export const TODO_STATUSES: readonly TodoStatus[] = ["in_progress", "pending", "completed", "abandoned"];
export interface TodoChild { status: TodoStatus; text: string; description?: string; unit: string | null }
export interface TodoItem { status: TodoStatus; text: string; description?: string; review: boolean; unit: string | null; children: TodoChild[] }
/** Typed rows the sidebar draws itself. Always four sections, in `TODO_STATUSES` order. */
export interface TodoSection { status: TodoStatus; count: number; items: TodoItem[] }
export interface TodoDisplay {
	sections: TodoSection[];
	empty: boolean;
	summary: string;
	current: string;
	text: string;
	state: unknown;
	createView(host: TodoViewHost): TodoView;
}
export interface TodoSnapshot {
	version: 3;
	presenter: string;
	request: string;
	publisher: string;
	revision: number;
	session: string;
	branch: string | null;
	display: TodoDisplay;
}

/** What the sidebar reads from the ledger: the provider's typed sections, never rendered lines. */
export interface TodoPayload {
	session: string;
	branch: string | null;
	revision: number;
	current: string;
	pane: string;
	sections: TodoSection[];
}

/** Atomic per-pane publication into Dogsled's ledger. One ordered writer per Pi session. */
export class TodoPublisher {
	private path: string | undefined;
	private published: TodoSnapshot | undefined;
	private queue = Promise.resolve();
	private readonly pane: string;
	private readonly failed: (error: unknown) => void;
	constructor(pane: string, failed: (error: unknown) => void) { this.pane = pane; this.failed = failed; }

	configure(value: unknown): void {
		if (!record(value) || typeof value["path"] !== "string") throw new Error("Invalid todo publication path");
		if (this.path !== value["path"]) this.published = undefined;
		this.path = value["path"];
	}

	publish(next: TodoSnapshot | undefined): void {
		const path = this.path;
		if (!path || (next && next === this.published)) return;
		const payload: TodoPayload | undefined = next
			? { session: next.session, branch: next.branch, revision: next.revision, current: next.display.current, pane: this.pane, sections: next.display.sections }
			: undefined;
		this.published = next;
		this.queue = this.queue.then(async () => {
			if (!payload) { await rm(path, { force: true }); return; }
			const temporary = `${path}.${randomUUID()}.tmp`;
			try {
				await writeFile(temporary, JSON.stringify(payload), { flag: "wx", mode: 0o600 });
				await rename(temporary, path);
			} finally { await rm(temporary, { force: true }); }
		}).catch(error => { this.published = undefined; this.failed(error); });
	}

	async clear(): Promise<void> {
		const path = this.path;
		this.published = undefined;
		this.queue = this.queue.then(async () => { if (path) await rm(path, { force: true }); }).catch(this.failed);
		await this.queue;
	}
	async flushed(): Promise<void> { await this.queue; }
}

type Bus = ExtensionAPI["events"];
function record(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null; }
function isSections(value: unknown): value is TodoSection[] {
	return Array.isArray(value) && value.length === TODO_STATUSES.length
		&& value.every((section, index) => record(section) && section["status"] === TODO_STATUSES[index]
			&& Number.isSafeInteger(section["count"]) && Array.isArray(section["items"]));
}
function isDisplay(value: unknown): boolean {
	return record(value) && typeof value["empty"] === "boolean" && typeof value["summary"] === "string"
		&& typeof value["current"] === "string" && typeof value["text"] === "string" && isSections(value["sections"]) && "state" in value && typeof value["createView"] === "function";
}
function isView(value: unknown): value is TodoView {
	return record(value) && ["render", "handleInput", "invalidate", "update", "dispose"].every(key => typeof value[key] === "function");
}

/** Validate the independently installed provider's capability, never its task schema. */
export function snapshot(value: unknown): TodoSnapshot | undefined {
	if (!record(value) || value["version"] !== 3 || typeof value["presenter"] !== "string" || typeof value["request"] !== "string"
		|| typeof value["publisher"] !== "string" || typeof value["session"] !== "string"
		|| (value["branch"] !== null && typeof value["branch"] !== "string")
		|| !Number.isSafeInteger(value["revision"]) || Number(value["revision"]) < 1 || !isDisplay(value["display"])) return undefined;
	return value as unknown as TodoSnapshot;
}

/** A view lease catches provider failures on every path and disposes exactly once. */
class HostedTodo implements TodoView {
	private disposed = false;
	private readonly view: TodoView;
	private readonly failed: (error: unknown) => void;
	private readonly closed: () => void;
	constructor(view: TodoView, failed: (error: unknown) => void, closed: () => void) {
		this.view = view; this.failed = failed; this.closed = closed;
	}
	private call<T>(action: () => T, fallback: T): T {
		if (this.disposed) return fallback;
		try { return action(); } catch (error) { this.failed(error); return fallback; }
	}
	render(width: number): string[] { return this.call(() => this.view.render(width), []); }
	handleInput(input: string): void { this.call(() => this.view.handleInput(input), undefined); }
	invalidate(): void { this.call(() => this.view.invalidate(), undefined); }
	update(state: unknown): void { this.call(() => this.view.update(state), undefined); }
	dispose(): void {
		if (this.disposed) return;
		this.disposed = true;
		this.closed();
		try { this.view.dispose(); } catch (error) { this.failed(error); }
	}
}

/** A session-scoped display lease. The provider remains the only task-state writer. */
export class WorkTodo {
	private subscriptions: (() => void)[] = [];
	private ctx: ExtensionContext | undefined;
	private presenter = "";
	private request = "";
	private retired = new Set<string>();
	private views = new Set<HostedTodo>();
	private rejecting = false;
	current: TodoSnapshot | undefined;
	error: string | undefined;

	private readonly bus: Bus;
	private readonly changed: () => void;
	private readonly open: () => void;
	constructor(bus: Bus, changed: () => void, open: () => void) { this.bus = bus; this.changed = changed; this.open = open; }

	start(ctx: ExtensionContext): void {
		this.stop();
		if (ctx.mode !== "tui") return;
		this.ctx = ctx;
		this.presenter = randomUUID();
		this.retired.clear();
		this.subscriptions = [
			this.bus.on("oisin.todo.ready", value => {
				if (!record(value) || value["version"] !== 3 || typeof value["publisher"] !== "string") { this.reject(); return; }
				if (this.retired.has(value["publisher"])) return;
				this.ask();
			}),
			this.bus.on("oisin.todo.snapshot", value => this.receive(value)),
			this.bus.on("oisin.todo.stop", value => {
				if (!record(value) || !this.current || value["publisher"] !== this.current.publisher) return;
				this.retired.add(this.current.publisher);
				this.release();
				this.changed();
			}),
			this.bus.on("oisin.work.open", value => {
				if (record(value) && value["version"] === 3 && value["presenter"] === this.presenter && value["request"] === this.request) this.open();
			}),
		];
		this.ask();
	}

	private ask(): void {
		this.release();
		this.request = randomUUID();
		this.bus.emit("oisin.work.request", { version: 3, presenter: this.presenter, request: this.request });
	}

	private reject(error?: unknown): void {
		if (this.rejecting) return;
		this.rejecting = true;
		try {
			this.error = "Todo integration unavailable";
			this.release();
			if (error !== undefined) this.ctx?.ui.notify(`Work display unavailable: ${String(error)}`, "error");
			this.changed();
		} finally { this.rejecting = false; }
	}

	private preflight(next: TodoSnapshot): void {
		if (!this.ctx) return;
		const view: unknown = next.display.createView({ theme: this.ctx.ui.theme, height: () => 1, requestRender() {} });
		if (!isView(view)) throw new Error("Invalid todo component");
		try {
			view.update(next.display.state);
			const lines: unknown = view.render(1);
			if (!Array.isArray(lines) || !lines.every(line => typeof line === "string")) throw new Error("Invalid todo render output");
		} finally { view.dispose(); }
	}

	private receive(value: unknown): void {
		if (!record(value) || value["presenter"] !== this.presenter || value["request"] !== this.request) return;
		const next = snapshot(value);
		if (!next) { this.reject(); return; }
		if (!this.ctx || next.session !== this.ctx.sessionManager.getSessionId() || next.branch !== this.ctx.sessionManager.getLeafId() || this.retired.has(next.publisher)) return;
		if (this.current?.publisher === next.publisher && next.revision <= this.current.revision) return;
		try {
			this.preflight(next);
			if (this.current && this.current.publisher !== next.publisher) {
				this.retired.add(this.current.publisher);
				this.disposeViews();
			}
			this.current = next;
			this.error = undefined;
			for (const view of this.views) view.update(next.display.state);
			if (this.current !== next) return;
			this.changed();
			if (this.current !== next) return;
			this.bus.emit("oisin.work.claim", { version: 3, presenter: this.presenter, request: this.request, publisher: next.publisher, revision: next.revision });
		} catch (error) { this.reject(error); }
	}

	/** An inline expansion owns this instance until it collapses or its lease ends. */
	createView(host: TodoViewHost): TodoView | undefined {
		if (!this.current) return undefined;
		try {
			const created: unknown = this.current.display.createView(host);
			if (!isView(created)) throw new Error("Invalid todo component");
			const view = new HostedTodo(created, error => this.reject(error), () => this.views.delete(view));
			this.views.add(view);
			return view;
		} catch (error) { this.reject(error); return undefined; }
	}

	private disposeViews(): void { for (const view of [...this.views]) view.dispose(); }
	private release(): void {
		const claimed = this.current !== undefined;
		this.current = undefined;
		this.disposeViews();
		if (claimed) this.bus.emit("oisin.work.release", { version: 3, presenter: this.presenter, request: this.request });
	}

	stop(): void {
		for (const off of this.subscriptions) off();
		this.subscriptions = [];
		this.release();
		this.ctx = undefined;
		this.error = undefined;
	}
}
