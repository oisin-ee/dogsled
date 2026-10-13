import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

function object(value: unknown): Record<string, unknown> | undefined {
	return value !== null && typeof value === "object" && !Array.isArray(value)
		? value as Record<string, unknown> : undefined;
}

function widgetLines(value: unknown): string[] {
	const data = object(value);
	if (!data || !Array.isArray(data["items"]) || !Array.isArray(data["queue"])) {
		throw new Error("wrangle items returned an invalid item store");
	}
	const lines: string[] = [];
	for (const value of data["items"]) {
		const item = object(value);
		if (item?.["kind"] === "job" && item["closed_ms"] === null) {
			lines.push(`job ${String(item["id"])} · ${String(item["host"])} · ${String(item["pane"])}`);
		}
	}
	for (const value of data["queue"]) {
		const ticket = object(value);
		if (ticket?.["state"] === "pending") lines.push(`queued ${String(ticket["id"])}`);
	}
	return lines.map(line => line.replace(/[\x00-\x1f\x7f-\x9f]/g, " "));
}

export default function wrangle(pi: ExtensionAPI) {
	const binary = process.env["WRANGLE_BIN"] ?? "wrangle";
	const owner = process.env["HERDR_PANE_ID"];
	let timer: ReturnType<typeof setInterval> | undefined;
	let generation = 0;
	let refresh: Promise<void> | undefined;

	async function exec(args: string[]) {
		const result = await pi.exec(binary, args);
		if (result.code !== 0) throw new Error(`wrangle ${args[0]} failed: ${result.stderr.trim() || `exit ${result.code}`}`);
		return result.stdout;
	}

	function update(ctx: ExtensionContext): Promise<void> {
		if (!owner || ctx.mode !== "tui") return Promise.resolve();
		if (refresh) return refresh;
		const current = generation;
		refresh = (async () => {
			const lines = widgetLines(JSON.parse(await exec(["items", "--owner", owner, "--json"])));
			if (current === generation) ctx.ui.setWidget("wrangle", lines.length ? lines : undefined, { placement: "aboveEditor" });
		})().finally(() => { refresh = undefined; });
		return refresh;
	}

	pi.on("session_start", async (_event, ctx) => {
		if (timer) clearInterval(timer);
		generation++;
		await update(ctx);
		if (owner && ctx.mode === "tui") {
			timer = setInterval(() => { void update(ctx).catch(error => ctx.ui.notify(String(error), "error")); }, 10_000);
			timer.unref();
		}
	});
	pi.on("session_shutdown", (_event, ctx) => {
		generation++;
		if (timer) clearInterval(timer);
		timer = undefined;
		if (ctx.mode === "tui") ctx.ui.setWidget("wrangle", undefined);
	});
	pi.on("tool_result", async (event, ctx) => {
		if (event.toolName === "agents" && !event.isError && owner) {
			const candidates: unknown[] = [event.structuredContent, event.details];
			for (const block of event.content) {
				if (block.type === "text") {
					try { candidates.push(JSON.parse(block.text)); } catch { /* Non-JSON tool text is not a spawn result. */ }
				}
			}
			const spawned = candidates.map(object).find(value => value?.["spawned"] === true);
			if (spawned) {
				const machine = spawned["machine"], target = spawned["target"], name = spawned["name"];
				if (typeof machine !== "string" || typeof target !== "string" || typeof name !== "string") {
					throw new Error("agents spawn result is missing machine, target, or name");
				}
				await exec(["track", machine, target, "--name", name]);
			}
		}
		await update(ctx);
	});
	pi.on("before_agent_start", async (event) => {
		const status = await exec(["status", "--brief"]);
		return { systemPrompt: `${event.systemPrompt}\n\n## Host status\n${status.trim()}\n` };
	});
}
