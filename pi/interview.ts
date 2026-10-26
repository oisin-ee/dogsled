// The `interview` tool: the model asks the user a structured form, and the
// portal's form server carries the questions and the answers. The tool keeps
// the name, the parameters, and the result shape of pi-interview 0.13.0, so
// the model asks the way it always did. Only the transport differs: the form
// lives on the loopback portal server, not in a local window.
import { readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { AgentToolResult, ExtensionAPI, ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { PortalClient, sleep } from "./portal-client.ts";

/** How long the tool waits between two polls for the answers. */
const POLL_MS = 500;

/** The parameter schema of pi-interview 0.13.0, so the model calls it the same way. */
const interviewParameters = {
	type: "object",
	required: ["questions"],
	properties: {
		questions: { type: "string", description: "Inline JSON string with questions, or path to a questions JSON / saved interview HTML file" },
		timeout: { type: "number", description: "Seconds before auto-timeout", default: 600 },
		verbose: { type: "boolean", description: "Enable debug logging", default: false },
		async: { type: "boolean", description: "Return immediately instead of waiting for the user. The answer arrives later as a message tagged with the returned interview ID; keep working and do not poll.", default: false },
		theme: {
			type: "object",
			properties: {
				mode: { type: "string", enum: ["auto", "light", "dark"] },
				name: { type: "string" },
				lightPath: { type: "string" },
				darkPath: { type: "string" },
				toggleHotkey: { type: "string" },
			},
			additionalProperties: false,
		},
	},
} as const;

/** The tool description of pi-interview 0.13.0, unchanged. */
const interviewDescription =
	"Present an interactive form to gather user responses. " +
	"Opens in a configured Orca browser tab, a native macOS window (Glimpse), or a standard browser. " +
	"Use proactively when: choosing between multiple approaches, gathering requirements before implementation, " +
	"exploring design tradeoffs, or when decisions have multiple dimensions worth discussing. " +
	"Provides better UX than back-and-forth chat for structured input. " +
	"Image responses and attachments are returned as file paths - use read tool directly to display them. " +
	"Pass questions as inline JSON string directly (preferred) or as a path to a JSON file. " +
	'Questions JSON format: { "title": "...", "description": "...", "questions": [{ "id": "q1", "type": "single|multi|text|image|info", "question": "...", "options": ["A", "B"], "content": { "source": "...", "lang": "ts" }, "media": { "type": "image|chart|mermaid|table|html", ... } }] }. ' +
	"Options can be strings or objects: { label: string, content?: { source, lang?, file?, lines?, highlights?, title?, showSource? } }. " +
	"Always set recommended with context explaining your reasoning. Recommended options show a 'Recommended' badge and are pre-selected for the user. " +
	'Use conviction: "slight" when unsure (does NOT pre-select), conviction: "strong" when very confident (shows Recommended badge). ' +
	"Omit conviction for normal recommendations (pre-selects). " +
	'Use weight: "critical" for key decisions (visually prominent), weight: "minor" for low-stakes questions (compact card). ' +
	"When questions have recommendations, set description to guide review (e.g., 'Review my suggestions and adjust as needed'). " +
	"Questions can have a content field to display code or markdown above options. lang: \"md\" or \"markdown\" defaults to markdown preview unless showSource is true. Types: single (radio), multi (checkbox), text (textarea), image (file upload), info (non-interactive). " +
	'Media blocks: { type: "image", src, alt, caption }, { type: "table", table: { headers, rows, highlights }, caption }, { type: "chart", chart: { data, options }, caption }, { type: "mermaid", mermaid: "graph LR\\n..." }, { type: "html", html }. ' +
	"Info type is a non-interactive content panel for displaying context with media. Media position: above (default), below, side (two-column).";

/** One answer to one question, as the portal returns it. */
export interface ResponseItem {
	id: string;
	value: string | string[] | { option: string; note?: string } | { option: string; note?: string }[];
	attachments?: string[];
}

/** The result shape of pi-interview 0.13.0. */
export interface InterviewDetails {
	status: "completed" | "cancelled" | "timeout" | "aborted" | "queued" | "started";
	responses: ResponseItem[];
	url: string;
}

/** One question of a form, as the model wrote it. */
export interface Question {
	id: string;
	type: "single" | "multi" | "text" | "image" | "info";
	question: string;
}

function isChoiceValue(value: unknown): value is { option: string; note?: string } {
	return typeof value === "object" && value !== null && !Array.isArray(value) && typeof (value as { option?: unknown }).option === "string";
}

/** The value of one answer, as the model reads it. */
function formatResponseValue(value: ResponseItem["value"]): string {
	if (Array.isArray(value)) {
		if (value.every(isChoiceValue)) return value.map(item => item.note ? `${item.option} (${item.note})` : item.option).join(", ");
		return value.join(", ");
	}
	if (isChoiceValue(value)) return value.note ? `${value.option} (${value.note})` : value.option;
	return value;
}

function hasAnswerValue(value: ResponseItem["value"]): boolean {
	if (Array.isArray(value)) {
		if (value.every(isChoiceValue)) return value.some(item => item.option.trim() !== "");
		return value.some(item => typeof item === "string" && item.trim() !== "");
	}
	if (isChoiceValue(value)) return value.option.trim() !== "";
	return typeof value === "string" && value.trim() !== "";
}

function hasAnswer(response: ResponseItem): boolean {
	return hasAnswerValue(response.value) || Boolean(response.attachments?.length);
}

/** The answered questions, in the order the form asked them. */
function buildAnsweredResponses(responses: ResponseItem[], questions: Question[]): ResponseItem[] {
	const byId = new Map(responses.filter(response => response && typeof response.id === "string").map(response => [response.id, response]));
	return questions.flatMap(question => {
		const response = byId.get(question.id);
		if (!response || !hasAnswer(response)) return [];
		return [response];
	});
}

/** The answered questions as the model reads them: a summary and the structured data. */
function formatAnsweredResponses(responses: ResponseItem[], questions: Question[]): string {
	const answered = buildAnsweredResponses(responses, questions);
	if (answered.length === 0) return "(none)";
	const questionById = new Map(questions.map(question => [question.id, question]));
	const summary = answered.map(response => {
		const question = questionById.get(response.id);
		if (!question) return `- ${response.id}`;
		let line = `- ${question.question}: ${formatResponseValue(response.value)}`;
		if (response.attachments?.length) line += ` [attachments: ${response.attachments.join(", ")}]`;
		return line;
	}).join("\n");
	return `${summary}\n\nStructured response data:\n\n\`\`\`json\n${JSON.stringify(answered, null, 2)}\n\`\`\``;
}

/** The questions of a form, read from the inline JSON or from a questions file. */
export function loadQuestions(input: string, cwd: string): Question[] {
	const trimmed = input.trimStart();
	const inline = trimmed.startsWith("{") || /^`{3,}(?:json|jsonc)?\s*\n?\s*\{/i.test(trimmed);
	const text = inline ? trimmed : readFileSync(resolveQuestionPath(input, cwd), "utf8");
	let data: unknown;
	try { data = JSON.parse(text); }
	catch (error) { throw new Error(`Invalid questions JSON: ${error instanceof Error ? error.message : String(error)}`); }
	const questions = (data as { questions?: unknown }).questions;
	if (!Array.isArray(questions)) throw new Error("Invalid questions JSON: no questions array");
	return questions.map((question, index) => {
		const q = question as Partial<Question>;
		if (typeof q?.id !== "string" || typeof q?.question !== "string") throw new Error(`Invalid questions JSON: question ${index} has no id or question`);
		return { id: q.id, type: q.type ?? "text", question: q.question };
	});
}

function resolveQuestionPath(input: string, cwd: string): string {
	const expanded = input.startsWith("~/") ? join(process.env["HOME"] ?? "", input.slice(2)) : input;
	return isAbsolute(expanded) ? expanded : join(cwd, expanded);
}

/**
 * The answers the portal returns, and the questions they belong to. The portal
 * may answer with a bare list, with `answers`, or with `responses`; the
 * questions are optional, because the tool can name the answers from the
 * questions the model sent.
 */
function parseAnswers(value: unknown): { responses: ResponseItem[]; questions: Question[] } {
	const payload = value as { answers?: unknown; responses?: unknown; questions?: unknown } | null;
	const list = Array.isArray(value) ? value : Array.isArray(payload?.answers) ? payload.answers : payload?.responses;
	const responses = Array.isArray(list) ? list as ResponseItem[] : [];
	const questions = Array.isArray(payload?.questions) ? payload.questions as Question[] : [];
	return { responses: responses.filter(response => response && typeof response.id === "string"), questions };
}

/** The `interview` tool: ask the user a form through the portal and wait for the answers. */
export class InterviewTool {
	private readonly client: PortalClient;
	private readonly deliver: (content: string) => void;
	/** The background waits of the async interviews, so the session can close them. */
	private readonly pending = new Set<AbortController>();
	constructor(client: PortalClient = new PortalClient(), deliver: (content: string) => void = () => undefined) {
		this.client = client;
		this.deliver = deliver;
	}

	/** Run one interview. The tool's `signal` withdraws the form. */
	async run(params: { questions: string; timeout?: number; verbose?: boolean; async?: boolean }, ctx: ExtensionToolContext, signal: AbortSignal | undefined): Promise<AgentToolResult<InterviewDetails>> {
		const cwd = ctx.cwd;
		const session = ctx.sessionManager.getSessionId();
		const pane = process.env["HERDR_PANE_ID"] ?? "";
		const created = await this.client.json<{ id?: unknown }>("POST", "/api/questions", {
			questions: params.questions,
			pane,
			session,
			cwd,
		});
		const id = typeof created?.id === "string" ? created.id : "";
		if (!id) throw new Error("dogsled web did not return an interview id");

		if (params.async) {
			// The form runs on its own: the tool's own signal no longer owns it, and the
			// answers arrive as a message, like pi-interview does.
			const own = new AbortController();
			this.pending.add(own);
			void this.wait(id, params.questions, ctx, own.signal, params.timeout)
				.then(answers => { this.deliver(`Interview ${id} finished.\n\n${answers.text}`); })
				.catch(error => { this.deliver(`Interview ${id} did not finish: ${error instanceof Error ? error.message : String(error)}`); })
				.finally(() => { this.pending.delete(own); });
			return {
				content: [{ type: "text", text: `Interview ${id} started. The answer arrives later as a message; keep working and do not poll.` }],
				details: { status: "started", responses: [], url: "" },
			};
		}

		try {
			const answers = await this.wait(id, params.questions, ctx, signal, params.timeout);
			return {
				content: [{ type: "text", text: `User completed the interview form.\n\nAnswered responses:\n${answers.text}` }],
				details: { status: "completed", responses: answers.responses, url: "" },
			};
		} catch (error) {
			// An aborted interview withdraws its form, so the portal does not wait for an answer.
			if (signal?.aborted) {
				await this.client.request("DELETE", `/api/questions/${encodeURIComponent(id)}`).catch(() => undefined);
				return { content: [{ type: "text", text: "Interview was aborted." }], details: { status: "aborted", responses: [], url: "" } };
			}
			throw error;
		}
	}

	/** Close the background waits of the async interviews. */
	close(): void {
		for (const controller of this.pending) controller.abort();
		this.pending.clear();
	}

	/** Poll the portal until it has the answers, the form is withdrawn, or the wait ends. */
	private async wait(id: string, questions: string, ctx: ExtensionToolContext, signal: AbortSignal | undefined, timeout: number | undefined): Promise<{ text: string; responses: ResponseItem[] }> {
		const seconds = timeout ?? 600;
		const deadline = Date.now() + Math.max(1, seconds) * 1000;
		for (;;) {
			if (signal?.aborted) throw new Error("Interview was aborted.");
			const response = await this.client.request("GET", `/api/questions/${encodeURIComponent(id)}/answer`, { signal });
			if (response.status === 204) {
				if (Date.now() >= deadline) throw new Error(`Interview form timed out after ${seconds} seconds.`);
				await sleep(POLL_MS, signal);
				continue;
			}
			if (response.status === 410) throw new Error("The interview form was withdrawn before it was answered.");
			if (!response.ok) throw new Error(`dogsled web GET /api/questions/${id}/answer failed: ${response.status} ${(await response.text()).trim()}`);
			const { responses, questions: asked } = parseAnswers(await response.json());
			const names = asked.length ? asked : this.askedQuestions(questions, ctx);
			return { text: formatAnsweredResponses(responses, names), responses };
		}
	}

	/** The questions the model sent, used to name the answers when the portal sends none. */
	private askedQuestions(questions: string, ctx: ExtensionToolContext): Question[] {
		try { return loadQuestions(questions, ctx.cwd); }
		catch { return []; }
	}
}

/** Register the `interview` tool. */
export function registerInterview(pi: ExtensionAPI, client: PortalClient = new PortalClient()): InterviewTool {
	const tool = new InterviewTool(client, content => { pi.sendUserMessage(content); });
	pi.registerTool({
		name: "interview",
		label: "Interview",
		description: interviewDescription,
		promptSnippet: "Gather structured user input through an interactive form for requirements, tradeoffs, or multi-dimensional decisions.",
		parameters: interviewParameters,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			const question = params as { questions?: unknown };
			if (typeof question.questions !== "string" || !question.questions) throw new Error("interview needs the questions, as inline JSON or a path to a questions file");
			return await tool.run(question as { questions: string }, ctx, signal);
		},
	});
	return tool;
}
