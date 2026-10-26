// The client for the loopback portal server, `dogsled web`. The portal owns the
// interactive parts of Pi: the interview form and the plan review. This module
// only speaks HTTP to it. Every call carries the portal's bearer token, and the
// server answers with the board, a form, a review, or a decision.
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const DEFAULT_PORT = 7465;

/** `dogsled web` could not be reached, or its token file is missing. */
export class PortalDown extends Error {
	constructor(detail: string) {
		super(`dogsled web is not running (${detail}). Start it with \`dogsled web\`.`);
		this.name = "PortalDown";
	}
}

/** The port `dogsled web` listens on: `DOGSLED_WEB_PORT`, else 7465. */
export function portalPort(env: NodeJS.ProcessEnv): number {
	const port = Number(env["DOGSLED_WEB_PORT"]);
	return Number.isInteger(port) && port > 0 && port < 65536 ? port : DEFAULT_PORT;
}

/** The dogsled state directory: `DOGSLED_STATE`, else `$XDG_STATE_HOME/dogsled`, else `~/.local/state/dogsled`. */
export function stateDirectory(env: NodeJS.ProcessEnv): string {
	const state = env["DOGSLED_STATE"];
	if (state) return state;
	const xdg = env["XDG_STATE_HOME"];
	if (xdg) return join(xdg, "dogsled");
	return join(homedir(), ".local", "state", "dogsled");
}

/** One Server-Sent Event: its name and its data. */
export interface ServerEvent { event: string; data: string }

/** Parse a Server-Sent Events stream into events. Comments and unnamed fields are skipped. */
export async function* serverEvents(body: ReadableStream<Uint8Array>): AsyncGenerator<ServerEvent> {
	const reader = body.getReader();
	const decoder = new TextDecoder();
	let buffer = "";
	let event = "message";
	let data: string[] = [];
	const take = (): ServerEvent | undefined => {
		if (!data.length) { event = "message"; return undefined; }
		const out = { event, data: data.join("\n") };
		event = "message";
		data = [];
		return out;
	};
	const line = (text: string): void => {
		if (!text || text.startsWith(":")) return;
		const colon = text.indexOf(":");
		const field = colon < 0 ? text : text.slice(0, colon);
		let value = colon < 0 ? "" : text.slice(colon + 1);
		if (value.startsWith(" ")) value = value.slice(1);
		if (field === "event") event = value;
		else if (field === "data") data.push(value);
	};
	try {
		for (;;) {
			const { done, value } = await reader.read();
			if (done) break;
			buffer += decoder.decode(value, { stream: true });
			for (;;) {
				const index = buffer.search(/\r\n|\r|\n/);
				if (index < 0) break;
				// A trailing "\r" may be the first half of "\r\n"; wait for the next chunk.
				if (buffer[index] === "\r" && index + 1 === buffer.length) break;
				const separator = /^\r\n|^\r|^\n/.exec(buffer.slice(index))?.[0] ?? "\n";
				const text = buffer.slice(0, index);
				buffer = buffer.slice(index + separator.length);
				line(text);
				if (text === "") { const out = take(); if (out) yield out; }
			}
		}
		if (buffer) line(buffer);
		const out = take();
		if (out) yield out;
	} finally {
		await reader.cancel().catch(() => undefined);
	}
}

/** The HTTP client for the loopback portal server. Every call carries the bearer token. */
export class PortalClient {
	readonly env: NodeJS.ProcessEnv;
	constructor(env: NodeJS.ProcessEnv = process.env) { this.env = env; }

	/** The port this client talks to. */
	get port(): number { return portalPort(this.env); }

	/** The portal's bearer token, read from the state directory. */
	token(): string {
		const file = join(stateDirectory(this.env), "web", "token");
		try { return readFileSync(file, "utf8").trim(); }
		catch { throw new PortalDown(`no token at ${file}`); }
	}

	/** Send a request. A server that cannot be reached is a `PortalDown` error; an aborted `signal` keeps its own error. */
	async request(method: string, path: string, options: { body?: unknown; signal?: AbortSignal | undefined; accept?: string } = {}): Promise<Response> {
		const headers: Record<string, string> = {
			authorization: `Bearer ${this.token()}`,
			// The portal only accepts a request that changes state from its own origin.
			origin: `http://127.0.0.1:${this.port}`,
		};
		if (options.accept) headers["accept"] = options.accept;
		const init: RequestInit = { method, headers };
		if (options.body !== undefined) {
			headers["content-type"] = "application/json";
			init.body = JSON.stringify(options.body);
		}
		if (options.signal) init.signal = options.signal;
		try {
			return await fetch(`http://127.0.0.1:${this.port}${path}`, init);
		} catch (error) {
			if (options.signal?.aborted) throw error;
			const cause = error instanceof Error ? error.cause : undefined;
			const detail = cause instanceof Error ? cause.message : error instanceof Error ? error.message : String(error);
			throw new PortalDown(detail);
		}
	}

	/** Send a request and return its JSON body; any other status is an error. An empty body is `undefined`. */
	async json<T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
		const response = await this.request(method, path, { body, signal });
		if (!response.ok) throw new Error(`dogsled web ${method} ${path} failed: ${response.status} ${(await response.text()).trim()}`);
		const text = await response.text();
		if (!text.trim()) return undefined as T;
		return JSON.parse(text) as T;
	}

	/** Open a Server-Sent Events stream. */
	async events(path: string, signal: AbortSignal): Promise<AsyncGenerator<ServerEvent>> {
		const response = await this.request("GET", path, { accept: "text/event-stream", signal });
		if (!response.ok || !response.body) throw new Error(`dogsled web GET ${path} failed: ${response.status}`);
		return serverEvents(response.body);
	}
}

/** Resolve after `ms`, or early when `signal` aborts. */
export function sleep(ms: number, signal?: AbortSignal): Promise<void> {
	return new Promise(resolve => {
		if (signal?.aborted) { resolve(); return; }
		const done = () => { clearTimeout(timer); signal?.removeEventListener("abort", done); resolve(); };
		const timer = setTimeout(done, ms);
		signal?.addEventListener("abort", done, { once: true });
	});
}
