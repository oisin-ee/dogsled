import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Box, Text } from "@earendil-works/pi-tui";
import { type Board, clean, parseBoard, renderBoard } from "./board.ts";
import { type Details, InboxReader, MESSAGE_TYPE } from "./inbox.ts";
import { openPanel } from "./panel.ts";

const REFRESH_MS = 10_000;

export default function wrangle(pi: ExtensionAPI) {
	const binary = process.env["WRANGLE_BIN"] ?? "wrangle";
	const owner = process.env["HERDR_PANE_ID"];
	let timer: ReturnType<typeof setInterval> | undefined;
	let generation = 0;
	let refresh: Promise<void> | undefined;
	let board: Board = { hosts: [], rows: [] };
	let failure: string | undefined;

	// The wrangle commands as tools, declared to the model like built-ins (`mcp__wrangle__*`).
	// The server reads HERDR_PANE_ID from this process's environment to know its caller.
	pi.registerMcpServer("wrangle", {
		command: binary,
		args: ["mcp"],
		exposure: "direct",
		description: "Spawn Pi, Claude, and Codex children and jobs on Herdr hosts, and message any agent through its inbox.",
	});

	async function exec(args: string[]) {
		const result = await pi.exec(binary, args);
		if (result.code !== 0) throw new Error(`wrangle ${args[0]} failed: ${result.stderr.trim() || `exit ${result.code}`}`);
		return result.stdout;
	}

	const inbox = new InboxReader(pi, exec);

	pi.registerMessageRenderer<Details>(MESSAGE_TYPE, (message, { expanded, outputPad }, theme) => {
		const records = message.details?.records ?? [];
		const box = new Box(outputPad, 1, text => theme.bg("customMessageBg", text));
		const body = records.map(r => {
			const head = `${theme.fg("customMessageLabel", theme.bold(`wrangle ${clean(r.kind)}`))} ${theme.fg("muted", `from ${clean(r.sender)}`)}`;
			const lines = r.text.split("\n");
			const shown = expanded || lines.length <= 12 ? r.text : `${lines.slice(0, 12).join("\n")}\n${theme.fg("dim", `… ${lines.length - 12} more lines`)}`;
			return `${head}\n${shown}`;
		}).join("\n\n");
		box.addChild(new Text(body || String(message.content), 0, 0));
		return box;
	});

	let panelOpen = false;

	function draw(ctx: ExtensionContext) {
		// The panel lists the same rows; while it is open the widget steps aside.
		ctx.ui.setWidget("wrangle", panelOpen || board.rows.length === 0 ? undefined : (_tui, theme) => ({
			render: (width: number) => renderBoard(board, width, theme),
			invalidate: () => {},
		}), { placement: "aboveEditor" });
	}

	function update(ctx: ExtensionContext): Promise<void> {
		if (!owner) return Promise.resolve();
		if (refresh) return refresh;
		const current = generation;
		refresh = (async () => {
			await inbox.drain(ctx);
			if (ctx.mode !== "tui") return;
			try {
				const next = parseBoard(JSON.parse(await exec(["board", "--json"])));
				failure = undefined;
				if (current !== generation) return;
				board = next;
				draw(ctx);
			} catch (error) {
				// Say so once, not on every refresh; the last board stays on screen.
				const text = String(error);
				if (text !== failure) ctx.ui.notify(text, "error");
				failure = text;
			}
		})().finally(() => { refresh = undefined; });
		return refresh;
	}

	pi.on("session_start", async (_event, ctx) => {
		if (timer) clearInterval(timer);
		generation++;
		if (!owner) return;
		timer = setInterval(() => { void update(ctx); }, REFRESH_MS);
		timer.unref();
		await inbox.start(ctx);
		await update(ctx);
	});
	pi.on("session_shutdown", (_event, ctx) => {
		generation++;
		if (timer) clearInterval(timer);
		timer = undefined;
		inbox.stop();
		if (ctx.mode === "tui") ctx.ui.setWidget("wrangle", undefined);
	});
	// A delivered message is in the transcript once Pi ends it: only then is it acknowledged.
	pi.on("message_end", async event => { await inbox.ended(event.message); });
	pi.on("agent_settled", () => { inbox.settled(); });
	pi.on("before_agent_start", async (event) => {
		// Context is optional, including when the binary is absent or fails.
		try {
			const result = await pi.exec(binary, ["hook", "context", "--harness", "pi"], { timeout: 6_000 });
			if (result.code !== 0 || !result.stdout.trim()) return;
			return { systemPrompt: `${event.systemPrompt}\n\n## Host status\n${result.stdout.trim()}\n` };
		} catch {
			return;
		}
	});
	pi.registerCommand("wrangle", {
		description: "Hosts, your children and jobs, profiles, and messages",
		handler: async (_args, ctx) => {
			if (ctx.mode !== "tui") {
				ctx.ui.notify(await exec(["board"]), "info");
				return;
			}
			panelOpen = true;
			draw(ctx);
			try {
				await openPanel(ctx, exec);
			} finally {
				panelOpen = false;
				draw(ctx);
			}
			await update(ctx);
		},
	});
}
