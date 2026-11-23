/**
 * `end_turn`: the agent's final reply for the turn, as a native tool that ends the run.
 *
 * Dogsled records the kind; the transcript keeps the message, which the portal and the
 * inbox show as the turn's reply even when later check-ins or reminders follow it.
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { getMarkdownTheme } from "@earendil-works/pi-coding-agent";
import { Container, Markdown, Text } from "@earendil-works/pi-tui";

export const END_TURN = "end_turn";
export const END_TURN_KINDS = ["reply", "done", "ask", "blocked", "waiting"] as const;
export type EndTurnKind = typeof END_TURN_KINDS[number];

const LABELS: Record<EndTurnKind, string> = { reply: "Reply", done: "Done", ask: "Question", blocked: "Blocked", waiting: "Waiting" };

export const END_TURN_DESCRIPTION = "End every turn with this tool, as your last action. Put your whole final reply in `message`: it is what your owner reads, even when a check-in or reminder arrives later. `kind` says how the turn ends: done (whole task complete), ask (you need an answer from your owner), blocked (you cannot continue), waiting (for your own open children), or reply (anything else). Commit before done: dogsled closes you after that turn and keeps your branch. A refusal records nothing: fix the cause and call again. Write nothing after a successful call.";

type Run = (args: string[]) => Promise<{ code: number; stdout: string; stderr: string }>;

/** The checked arguments of a call, or the reason they are refused. */
export function endTurnArgs(params: unknown): { kind: EndTurnKind; message: string } {
	const { kind, message } = (params ?? {}) as { kind?: unknown; message?: unknown };
	if (typeof kind !== "string" || !(END_TURN_KINDS as readonly string[]).includes(kind)) throw new Error(`end_turn kind must be one of: ${END_TURN_KINDS.join(", ")}`);
	if (typeof message !== "string" || !message.trim()) throw new Error("end_turn needs a message: the reply your owner reads");
	return { kind: kind as EndTurnKind, message };
}

export function registerEndTurn(pi: ExtensionAPI, run: Run): void {
	pi.registerTool({
		name: END_TURN,
		label: "End turn",
		description: END_TURN_DESCRIPTION,
		promptSnippet: "End every turn with your final reply as the message.",
		parameters: {
			type: "object",
			required: ["kind", "message"],
			properties: {
				kind: { type: "string", enum: [...END_TURN_KINDS], description: "How the turn ends" },
				message: { type: "string", description: "Your whole final reply, in Markdown" },
			},
		} as const,
		executionMode: "sequential",
		async execute(_toolCallId, params) {
			const { kind, message } = endTurnArgs(params);
			const result = await run(["end-turn", "--kind", kind, "--message", message, "--json"]);
			if (result.code !== 0) throw new Error(result.stderr.trim() || `dogsled end-turn failed: exit ${result.code}`);
			return {
				content: [{ type: "text", text: "Turn ended. Write nothing more." }],
				details: { kind },
				terminate: true,
			};
		},
		// The message is the reply, so it reads as one, not as tool arguments.
		renderCall(args, theme) {
			const { kind, message } = (args ?? {}) as { kind?: unknown; message?: unknown };
			const label = typeof kind === "string" && kind in LABELS ? LABELS[kind as EndTurnKind] : "End turn";
			const view = new Container();
			view.addChild(new Text(theme.fg("toolTitle", theme.bold(label)), 0, 0));
			if (typeof message === "string" && message) view.addChild(new Markdown(message, 0, 0, getMarkdownTheme()));
			return view;
		},
		renderResult(result, _options, theme, context) {
			if (!context.isError) return new Text("", 0, 0);
			const text = result.content[0];
			return new Text(theme.fg("error", text?.type === "text" ? text.text : "end_turn was refused"), 0, 0);
		},
	});
}
