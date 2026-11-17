// The client for Pi's producer endpoint: `dogsled serve` answers it on `producer.sock` in
// the state directory on every host. A Pi session publishes its live activity and model
// settings there, and receives settings changes, whether or not its host runs the portal.
// The socket is private to the user (mode 0600), so requests carry no bearer token.
import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { join } from "node:path";
import { Readable } from "node:stream";
import { PortalClient, stateDirectory } from "./portal-client.ts";

const SOCKET = "producer.sock";

/** `dogsled serve` could not be reached on its producer socket. */
export class ServeDown extends Error {
	constructor(detail: string) {
		super(`dogsled serve is not running (${detail}). Start it with \`dogsled serve\`.`);
		this.name = "ServeDown";
	}
}

/** The longest socket path the platform accepts, excluding the terminating NUL. */
function socketPathLimit(platform: NodeJS.Platform): number {
	return platform === "linux" ? 107 : 103;
}

/**
 * The producer socket path, by the same rule as serve's `socket_path`: the state directory
 * when the path fits, else a hashed name in the private runtime directory.
 */
export function producerSocket(
	env: NodeJS.ProcessEnv = process.env,
	platform: NodeJS.Platform = process.platform,
	realpath: (path: string) => string = realpathSync,
	uid: number = process.getuid?.() ?? 0,
): string {
	const state = stateDirectory(env);
	const ordinary = join(state, SOCKET);
	if (Buffer.byteLength(ordinary) <= socketPathLimit(platform)) return ordinary;
	const runtime = env["XDG_RUNTIME_DIR"] ? join(env["XDG_RUNTIME_DIR"], "dogsled") : `/tmp/dogsled-${uid}`;
	const hash = createHash("sha256").update(realpath(state)).digest().subarray(0, 16).toString("hex");
	return join(runtime, `${hash}-producer.sock`);
}

/** HTTP over serve's producer socket, with the same request, JSON and event calls as the portal client. */
export class ServeClient extends PortalClient {
	protected override readonly server = "dogsled serve";
	private readonly socket: string;
	constructor(env: NodeJS.ProcessEnv = process.env, socket: string = producerSocket(env)) {
		super(env);
		this.socket = socket;
	}

	override async request(method: string, path: string, options: { body?: unknown; signal?: AbortSignal | undefined; accept?: string } = {}): Promise<Response> {
		const headers: Record<string, string> = { host: "localhost" };
		if (options.accept) headers["accept"] = options.accept;
		let payload: string | undefined;
		if (options.body !== undefined) {
			payload = JSON.stringify(options.body);
			headers["content-type"] = "application/json";
			headers["content-length"] = String(Buffer.byteLength(payload));
		}
		const signal = options.signal;
		if (signal?.aborted) throw signal.reason;
		return await new Promise<Response>((resolve, reject) => {
			const call = httpRequest({ socketPath: this.socket, path, method, headers, ...(signal ? { signal } : {}) }, reply => {
				const status = reply.statusCode ?? 502;
				const replyHeaders = new Headers();
				for (let index = 0; index + 1 < reply.rawHeaders.length; index += 2) {
					replyHeaders.append(reply.rawHeaders[index]!, reply.rawHeaders[index + 1]!);
				}
				const empty = method === "HEAD" || status === 204 || status === 304;
				if (empty) reply.resume();
				const body = empty ? null : Readable.toWeb(reply) as ReadableStream<Uint8Array>;
				resolve(new Response(body, { status, headers: replyHeaders }));
			});
			call.once("error", error => {
				reject(signal?.aborted ? error : new ServeDown(error.message));
			});
			call.end(payload);
		});
	}
}
