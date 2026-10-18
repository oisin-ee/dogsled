import { readFileSync } from "node:fs";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Box, Text, Key } from "@earendil-works/pi-tui";
import { type Board, clean, parseBoard } from "./board.ts";
import { WorkTodo } from "./work-todo.ts";
import { workStrip, workText, type WorkState } from "./work.ts";
import { type Details, InboxReader, MESSAGE_TYPE } from "./inbox.ts";
import { openPanel } from "./panel.ts";
import { loadJobHistory, type JobHistory } from "./work-history.ts";
import { WorkPanel, WorkDetail, runWorkAction, type WorkAction, type WorkCursor, type WorkData, type WorkTab } from "./work-panel.ts";

const REFRESH_MS = 10_000;

/** The version this extension ships with; the wrangle binary on PATH must match it. */
export const VERSION: string = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).version;

/** Why `output` of `wrangle --version` does not match this extension, or undefined when it does. */
export function versionMismatch(output: string): string | undefined {
	const found = /^wrangle (\S+)/.exec(output.trim())?.[1];
	if (found === VERSION) return undefined;
	return `wrangle ${found ?? "(unknown version)"} is on PATH, but this extension is ${VERSION}. Install wrangle ${VERSION} on every host so the two match.`;
}

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
	let workOpen = false;
	let history: JobHistory = { jobs: [] };
	let workPanel: WorkPanel | undefined;
	let workDetail: WorkDetail | undefined;
	let detail: { title: string; body: string } | undefined;
	let cursor: WorkCursor = { tab: "all", selected: undefined, history: false };
	let workAbort: AbortController | undefined;
	let context: ExtensionContext | undefined;
	const todos = new WorkTodo(pi.events, () => { if (context) draw(context); }, () => {
		if (context) void openWork(context, "todos").catch(error => context?.ui.notify(String(error), "error"));
	});

	function state(): WorkState {
		return { board, todo: todos.current, error: failure ?? todos.error };
	}

	function workData(): WorkData { return { ...state(), history }; }

	function collapseWork(ctx: ExtensionContext): void {
		workAbort?.abort();
		workAbort = undefined;
		workOpen = false;
		workPanel = undefined;
		workDetail = undefined;
		detail = undefined;
		history = { jobs: [] };
		draw(ctx);
	}

	async function openWork(ctx: ExtensionContext, tab: WorkTab): Promise<void> {
		if (ctx.mode !== "tui") { ctx.ui.notify(workText(state()), "info"); return; }
		if (panelOpen) return;
		workOpen = true;
		detail = undefined;
		workDetail = undefined;
		cursor = { tab, selected: undefined, history: false };
		workPanel = undefined;
		const current = generation;
		draw(ctx);
		// Draw immediately; remote history must not delay expansion or typing.
		if (refresh) await refresh;
		if (current === generation && workOpen) await update(ctx);
	}

	async function act(ctx: ExtensionContext, action: WorkAction | undefined): Promise<void> {
		if (!action) { collapseWork(ctx); return; }
		if (workAbort) return;
		const interaction = new AbortController();
		workAbort = interaction;
		const current = generation;
		try {
			const body = await runWorkAction(action, ctx, exec, interaction.signal);
			if (current !== generation || interaction.signal.aborted) return;
			if (body !== undefined) { detail = { title: action.entry.label, body }; workDetail = undefined; }
			draw(ctx);
			await update(ctx);
		} catch (error) {
			if (current === generation && !interaction.signal.aborted) {
				detail = { title: "Work action unavailable", body: String(error) };
				workDetail = undefined;
				draw(ctx);
			}
		} finally {
			if (workAbort === interaction) workAbort = undefined;
		}
	}

	function draw(ctx: ExtensionContext) {
		if (ctx.mode !== "tui") return;
		workPanel?.update(workData());
		const data = state();
		ctx.ui.setWidget("wrangle", panelOpen || (!workOpen && !data.board.rows.length && !data.todo?.tasks.length && !data.error) ? undefined : (tui, theme) => {
			if (workOpen) {
				if (detail) {
					workDetail ??= new WorkDetail(detail.title, detail.body, theme, () => tui.terminal.rows, () => tui.requestRender());
					workDetail.setTheme(theme);
					return workDetail;
				}
				workDetail = undefined;
				workPanel = new WorkPanel(workData(), cursor, theme, action => { void act(ctx, action); }, () => tui.requestRender(), () => tui.terminal.rows);
				return workPanel;
			}
			return { render: (width: number) => workStrip(data, width, theme), invalidate() {} };
		}, { placement: "aboveEditor" });
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
				if (current !== generation) return;
				failure = undefined;
				board = next;
				if (workOpen) {
					const loaded = await loadJobHistory(exec, owner);
					if (current !== generation) return;
					if (workOpen) history = loaded;
				}
				draw(ctx);
			} catch (error) {
				// Say so once, not on every refresh; the last board stays on screen.
				const text = String(error);
				if (current !== generation) return;
				if (text !== failure) ctx.ui.notify(text, "error");
				failure = text;
				draw(ctx);
			}
		})().finally(() => { refresh = undefined; });
		return refresh;
	}

	pi.on("session_start", async (_event, ctx) => {
		if (timer) clearInterval(timer);
		generation++;
		workAbort?.abort();
		workAbort = undefined;
		workOpen = false;
		workPanel = undefined;
		workDetail = undefined;
		detail = undefined;
		history = { jobs: [] };
		context = ctx;
		board = { hosts: [], rows: [] };
		failure = owner ? undefined : "Wrangle needs a Herdr owner pane";
		todos.start(ctx);
		if (!owner) { draw(ctx); return; }
		timer = setInterval(() => { void update(ctx); }, REFRESH_MS);
		timer.unref();
		await inbox.start(ctx);
		try {
			const mismatch = versionMismatch(await exec(["--version"]));
			if (mismatch) ctx.ui.notify(mismatch, "warning");
		} catch (error) {
			ctx.ui.notify(String(error), "error");
		}
		await update(ctx);
	});
	pi.on("session_shutdown", (_event, ctx) => {
		generation++;
		workAbort?.abort();
		workAbort = undefined;
		workOpen = false;
		workPanel = undefined;
		workDetail = undefined;
		detail = undefined;
		if (timer) clearInterval(timer);
		timer = undefined;
		inbox.stop();
		todos.stop();
		context = undefined;
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
	async function toggleWork(ctx: ExtensionContext): Promise<void> {
		if (workOpen) collapseWork(ctx);
		else await openWork(ctx, "all");
	}
	pi.registerCommand("work", {
		description: "Expand or collapse work above the editor (Alt+W)",
		handler: async (_args, ctx) => { await toggleWork(ctx); },
	});
	pi.registerShortcut(Key.alt("w"), { description: "Expand or collapse work above the editor", handler: toggleWork });
	for (const [key, input, description] of [
		["j", "\u001b[B", "Next work item / scroll down"], ["k", "\u001b[A", "Previous work item / scroll up"],
		["]", "\t", "Next work view"], ["i", "\r", "Inspect work / return to list"],
		["s", "s", "Stop selected job"], ["c", "c", "Cancel selected queued work"],
		["m", "m", "Message selected agent"], ["r", "r", "Refresh work"],
	] as const) {
		pi.registerShortcut(Key.alt(key), { description, handler: async (_ctx) => {
			if (!workOpen || panelOpen || workAbort || !context) return;
			if (detail) {
				if (key === "i") { detail = undefined; draw(context); }
				else if (key === "j" || key === "k") workDetail?.handleInput(input);
			} else workPanel?.handleInput(input);
		} });
	}
	pi.registerCommand("wrangle", {
		description: "Hosts, your children and jobs, profiles, and messages",
		handler: async (_args, ctx) => {
			if (ctx.mode !== "tui") {
				ctx.ui.notify(await exec(["board"]), "info");
				return;
			}
			if (panelOpen || workAbort) return;
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
