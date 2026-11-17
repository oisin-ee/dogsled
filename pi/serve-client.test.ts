import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { ServeClient, ServeDown, producerSocket } from "./serve-client.ts";

test("the producer socket is in the state directory when its path fits", () => {
	assert.equal(producerSocket({ DOGSLED_STATE: "/home/dev/.local/state/dogsled" }, "linux"), "/home/dev/.local/state/dogsled/producer.sock");
	assert.equal(producerSocket({ XDG_STATE_HOME: "/s" }, "darwin"), "/s/dogsled/producer.sock");
});

test("a long state path uses serve's hashed name in the private runtime directory", () => {
	const long = "/" + "long".repeat(30);
	const realpath = (path: string) => { assert.equal(path, long); return "/state/dogsled"; };
	// The first 16 bytes of SHA-256 over the canonical state path, as serve's socket_path names it.
	assert.equal(producerSocket({ DOGSLED_STATE: long, XDG_RUNTIME_DIR: "/run/user/1001" }, "linux", realpath, 1001),
		"/run/user/1001/dogsled/cd954d41b919952312b3c0ce044e88bf-producer.sock");
	assert.equal(producerSocket({ DOGSLED_STATE: long }, "darwin", realpath, 501), "/tmp/dogsled-501/cd954d41b919952312b3c0ce044e88bf-producer.sock");
});

test("requests reach serve over its socket without a bearer token, and events stream", async t => {
	const directory = mkdtempSync(join(tmpdir(), "dogsled-serve-client-"));
	const socket = join(directory, "producer.sock");
	const seen: { method?: string | undefined; url?: string | undefined; authorization?: string | undefined; body: string }[] = [];
	const server = createServer((request, response) => {
		let body = "";
		request.on("data", (chunk: Buffer) => { body += chunk.toString("utf8"); });
		request.on("end", () => {
			seen.push({ method: request.method, url: request.url, authorization: request.headers["authorization"], body });
			if (request.url?.endsWith("/requests")) {
				response.writeHead(200, { "content-type": "text/event-stream" });
				response.write("event: settings\ndata: {\"id\":\"one\"}\n\n");
				response.end(": done\n\n");
			} else if (request.url === "/missing") {
				response.statusCode = 404;
				response.end("{\"code\":\"settings_unavailable\"}");
			} else {
				response.setHeader("content-type", "application/json");
				response.end(JSON.stringify({ ok: true }));
			}
		});
	});
	await new Promise<void>(resolve => server.listen(socket, resolve));
	t.after(async () => {
		await new Promise<void>(resolve => { server.closeAllConnections(); server.close(() => resolve()); });
		rmSync(directory, { recursive: true, force: true });
	});
	const client = new ServeClient({}, socket);
	assert.deepEqual(await client.json("PUT", "/api/agents/w1%3Ap1/settings", { a: 1 }), { ok: true });
	assert.deepEqual(seen[0], { method: "PUT", url: "/api/agents/w1%3Ap1/settings", authorization: undefined, body: "{\"a\":1}" });
	const events = [];
	for await (const event of await client.events("/api/agents/w1%3Ap1/settings/requests", new AbortController().signal)) events.push(event);
	assert.deepEqual(events, [{ event: "settings", data: "{\"id\":\"one\"}" }]);
	await assert.rejects(client.json("GET", "/missing"), /dogsled serve GET \/missing failed: 404/);
});

test("a missing serve is ServeDown, and an aborted request keeps its own error", async () => {
	const client = new ServeClient({}, join(tmpdir(), "dogsled-no-such-socket", "producer.sock"));
	await assert.rejects(client.request("GET", "/api/agents/w1%3Ap1/activity"), ServeDown);
	const controller = new AbortController();
	controller.abort(new Error("stopped"));
	await assert.rejects(client.request("GET", "/x", { signal: controller.signal }), /stopped/);
});
