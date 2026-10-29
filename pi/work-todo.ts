import type { TodoPresenter, TodoWork } from "./todo/src/work.ts";
import type { ExtensionContext, Theme } from "@earendil-works/pi-coding-agent";
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
export interface TodoChild { status: TodoStatus; text: string; description?: string | undefined; unit: string | null | undefined }
export interface TodoItem { status: TodoStatus; text: string; description?: string | undefined; review: boolean; unit: string | null | undefined; children: readonly TodoChild[] }
/** Typed rows the sidebar draws itself. Always four sections, in `TODO_STATUSES` order. */
export interface TodoSection { status: TodoStatus; count: number; items: readonly TodoItem[] }
export interface TodoDisplay {
	sections: readonly TodoSection[];
	empty: boolean;
	summary: string;
	current: string;
	text: string;
	state: unknown;
	createView(host: TodoViewHost): TodoView;
}
export interface TodoSnapshot {
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
	sections: readonly TodoSection[];
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

function record(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null; }

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
export class WorkTodo implements TodoPresenter {
	private detach: (() => void) | undefined;
	private ctx: ExtensionContext | undefined;
	private views = new Set<HostedTodo>();
	private rejecting = false;
	current: TodoSnapshot | undefined;
	error: string | undefined;

	private readonly todo: Pick<TodoWork, "attach">;
	private readonly changed: () => void;
	private readonly onOpen: () => void;
	constructor(todo: Pick<TodoWork, "attach">, changed: () => void, open: () => void) { this.todo = todo; this.changed = changed; this.onOpen = open; }
	open(): void { this.onOpen(); }

	start(ctx: ExtensionContext): void {
		this.stop();
		if (ctx.mode !== "tui") return;
		this.ctx = ctx;
		const detach = this.todo.attach(this);
		if (this.error || this.ctx !== ctx) detach();
		else this.detach = detach;
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
		const view = next.display.createView({ theme: this.ctx.ui.theme, height: () => 1, requestRender() {} });
		try {
			view.update(next.display.state);
			const lines: unknown = view.render(1);
			if (!Array.isArray(lines) || !lines.every(line => typeof line === "string")) throw new Error("Invalid todo render output");
		} finally { view.dispose(); }
	}

	show(next: TodoSnapshot): boolean {
		if (!Number.isSafeInteger(next.revision) || next.revision < 1) { this.reject(); return false; }
		if (!this.ctx || next.session !== this.ctx.sessionManager.getSessionId() || next.branch !== this.ctx.sessionManager.getLeafId()) return false;
		if (this.current && next.revision <= this.current.revision) return false;
		try {
			this.preflight(next);
			this.current = next;
			this.error = undefined;
			for (const view of this.views) view.update(next.display.state);
			if (this.current !== next) return false;
			this.changed();
			return this.current === next;
		} catch (error) { this.reject(error); return false; }
	}

	/** An inline expansion owns this instance until it collapses or its lease ends. */
	createView(host: TodoViewHost): TodoView | undefined {
		if (!this.current) return undefined;
		try {
			const created = this.current.display.createView(host);
			const view = new HostedTodo(created, error => this.reject(error), () => this.views.delete(view));
			this.views.add(view);
			return view;
		} catch (error) { this.reject(error); return undefined; }
	}

	private disposeViews(): void { for (const view of [...this.views]) view.dispose(); }
	private release(): void {
		const detach = this.detach;
		this.detach = undefined;
		this.current = undefined;
		this.disposeViews();
		detach?.();
	}

	stop(): void {
		const claimed = this.current !== undefined;
		this.release();
		this.ctx = undefined;
		this.error = undefined;
		if (claimed) this.changed();
	}
}
