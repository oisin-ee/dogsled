import assert from "node:assert/strict";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { createEventBus, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import dogsled from "./index.ts";

test("the merged extension registers todo once and answers one provider acquisition", () => {
	const events = createEventBus();
	const tools: string[] = [];
	const api = {
		events,
		on() { return () => {}; },
		registerTool(tool: { name: string }) { tools.push(tool.name); },
		getAllTools() { return tools.map(name => ({ name, sourceInfo: { path: fileURLToPath(new URL("./index.ts", import.meta.url)) } })); },
		registerCommand() {},
		registerShortcut() {},
		registerFlag() {},
		getFlag() { return undefined; },
		registerMcpServer() {},
		registerMessageRenderer() {},
	} as unknown as ExtensionAPI;
	dogsled(api);
	assert.equal(tools.filter(name => name === "todo").length, 1);
	const replies: unknown[] = [];
	events.emit("oisin.tasks.acquire.v1", { version: 1, reply(provider: unknown) { replies.push(provider); } });
	assert.equal(replies.length, 1);
	assert.equal(typeof (replies[0] as { inspect: unknown }).inspect, "function");
});
