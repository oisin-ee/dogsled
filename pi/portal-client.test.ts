import assert from "node:assert/strict";
import { test } from "node:test";
import { ChildProcess } from "node:child_process";
import { openInBrowser, PortalClient, portalUrl } from "./portal-client.ts";

test("browser launch failures report the recovery URL without throwing", t => {
	const url = "http://127.0.0.1:7465/?question=q-1";
	const diagnostic = t.mock.method(console, "error", () => undefined);
	for (const failure of ["error", "exit", "signal", "success"]) {
		const child = new ChildProcess();
		const unref = t.mock.method(child, "unref", () => undefined);
		openInBrowser(url, (command, args, options) => {
			assert.equal(command, process.platform === "darwin" ? "open" : "xdg-open");
			assert.deepEqual(args, [url]);
			assert.deepEqual(options, { detached: true, stdio: "ignore" });
			return child;
		});
		assert.equal(unref.mock.callCount(), 1);
		if (failure === "error") child.emit("error", new Error("spawn ENOENT"));
		else child.emit("exit", failure === "success" ? 0 : failure === "exit" ? 1 : null, failure === "signal" ? "SIGTERM" : null);
	}
	assert.equal(diagnostic.mock.callCount(), 3);
	for (const [index, reason] of ["ENOENT", "exited with 1", "SIGTERM"].entries()) {
		const message = String(diagnostic.mock.calls[index]?.arguments[0]);
		assert.ok(message.includes(reason));
		assert.ok(message.includes(url));
	}
});

test("portal URLs use the client's configured port and preserve the deep link", () => {
	const env = { DOGSLED_WEB_PORT: "12345" };
	const path = "/?question=q-1&pane=pane%20%26%20name";
	assert.equal(portalUrl(path, env), `http://127.0.0.1:${new PortalClient(env).port}${path}`);
	assert.equal(portalUrl(path, env), `http://127.0.0.1:12345${path}`);
});

test("portal URLs and requests share the default port for missing or invalid configuration", () => {
	for (const value of [undefined, "", "invalid", "0", "65536", "-1", "7465.5"]) {
		const env: NodeJS.ProcessEnv = value === undefined ? {} : { DOGSLED_WEB_PORT: value };
		assert.equal(new PortalClient(env).port, 7465);
		assert.equal(portalUrl("/?review=rev-1", env), "http://127.0.0.1:7465/?review=rev-1");
	}
});
