import assert from "node:assert/strict";
import { test } from "node:test";
import { PortalClient, portalUrl } from "./portal-client.ts";

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
