// The `interview` tool: the model asks the user a structured form, and the
// portal's form server carries the questions and the answers. The tool keeps
// the name, the parameters, and the result shape of pi-interview 0.13.0, so
// the model asks the way it always did. Only the transport differs: the form
// lives on the loopback portal server, not in a local window.
import { randomUUID } from "node:crypto";
import { readFileSync } from "node:fs";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";
import type { AgentToolResult, ExtensionAPI, ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { PortalClient, PortalDown, sleep, startOneShot } from "./portal-client.ts";

/** How long the tool waits between two polls for the answers. */
const POLL_MS = 500;

/** The parameter schema of pi-interview 0.13.0, so the model calls it the same way. */
const interviewParameters = {
	type: "object",
	required: ["questions"],
	properties: {
		questions: { type: "string", description: "Inline JSON string with questions, or path to a questions JSON file" },
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

/** A questions file, parsed. The portal renders it as a form. */
export type QuestionsFile = Record<string, unknown>;

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

/**
 * The questions of a form, read from the inline JSON or from a questions file.
 * The model may pass either form, as pi-interview does. The file is read
 * relative to the working directory, and a broken file is an error before any
 * call to the portal.
 */
export function loadQuestions(input: string, cwd: string): QuestionsFile {
	const trimmed = input.trimStart();
	const inline = trimmed.startsWith("{") || /^`{3,}(?:json|jsonc)?\s*\n?\s*\{/i.test(trimmed);
	const text = inline ? trimmed : readQuestionsFile(input, cwd);
	const data = parseQuestionsJson(text);
	validateQuestionsFile(data);
	return data;
}

/** The questions of a form, in the order the form asks them. */
function formQuestions(file: QuestionsFile): Question[] {
	const list = file["questions"];
	if (!Array.isArray(list)) return [];
	return list.flatMap(entry => {
		const question = (entry ?? {}) as Partial<Question>;
		if (typeof question.id !== "string" || typeof question.question !== "string") return [];
		return [{ id: question.id, type: questionType(question.type), question: question.question }];
	});
}

/** The type of a question, when it is one the form knows. */
function questionType(value: unknown): Question["type"] {
	return value === "single" || value === "multi" || value === "text" || value === "image" || value === "info" ? value : "text";
}

/** Read the questions file, or say that it is not there. */
function readQuestionsFile(input: string, cwd: string): string {
	const path = resolveQuestionPath(input, cwd);
	try { return readFileSync(path, "utf8"); }
	catch (error) {
		const code = (error as { code?: string }).code;
		if (code === "ENOENT") throw new Error(`Questions file not found: ${path}`);
		throw new Error(`Cannot read the questions file ${path}: ${error instanceof Error ? error.message : String(error)}`);
	}
}

/** The JSON text of a questions file, without the code fence a model may wrap it in. */
function questionsJsonText(text: string): string {
	const json = text.trim();
	const fence = json.match(/^`{3,}(?:json|jsonc)?\s*\n([\s\S]*?)\n\s*`{3,}\s*$/i);
	return (fence ? (fence[1] ?? "") : json).trim();
}

/** The parsed questions, or an error that names the problem. */
function parseQuestionsJson(text: string): unknown {
	try { return JSON.parse(questionsJsonText(text)); }
	catch (error) { throw new Error(`Invalid questions JSON: ${error instanceof Error ? error.message : String(error)}`); }
}

/** The parsed questions must be a file with a questions array. */
function validateQuestionsFile(data: unknown): asserts data is QuestionsFile {
	if (!data || typeof data !== "object" || Array.isArray(data)) throw new Error("Invalid questions JSON: no questions object");
	const list = (data as { questions?: unknown }).questions;
	if (!Array.isArray(list)) throw new Error("Invalid questions JSON: no questions array");
	list.forEach((entry, index) => {
		const question = (entry ?? {}) as Partial<Question>;
		if (typeof question.id !== "string" || typeof question.question !== "string") throw new Error(`Invalid questions JSON: question ${index} has no id or question`);
	});
}

function resolveQuestionPath(input: string, cwd: string): string {
	const expanded = input.startsWith("~/") ? join(process.env["HOME"] ?? "", input.slice(2)) : input;
	return isAbsolute(expanded) ? expanded : join(cwd, expanded);
}

/**
 * Whether a source names a file on this host, rather than a URL the browser
 * can load on its own.
 *
 * A form the model writes names its images by the path they have on the
 * agent's machine. That machine is not the machine the browser runs on, so
 * the path never loads. An `http:` or `data:` source already carries its
 * bytes, so it is left alone.
 */
function isLocalSource(src: string): boolean {
	return !/^(?:https?:|data:)/i.test(src);
}

/** The bytes of an image, or an error that names the file. */
function readImage(path: string): Buffer {
	try {
		return readFileSync(path);
	} catch (error) {
		const code = (error as { code?: string }).code;
		if (code === "ENOENT") throw new Error(`Image not found: ${path}`);
		throw new Error(`Cannot read the image ${path}: ${error instanceof Error ? error.message : String(error)}`);
	}
}

/** The URL the portal serves an attachment from. */
function attachmentUrl(path: string): string {
	const name = path.split(/[\\/]/).pop() ?? "";
	return `/api/attachments/${name}`;
}

/**
 * The answers the portal returns. The contract is one body: the id of the
 * form, and the responses of the form. The questions are not part of it, so
 * the tool names the answers from the questions the model sent. Any other
 * body is an error that names the shape it expected.
 */
function parseAnswers(value: unknown): ResponseItem[] {
	const payload = value as { id?: unknown; responses?: unknown } | null;
	if (!payload || typeof payload !== "object" || Array.isArray(payload) || typeof payload.id !== "string" || !Array.isArray(payload.responses)) {
		throw new Error('dogsled web returned an unexpected answer shape: expected {"id": string, "responses": ResponseItem[]}');
	}
	return payload.responses.filter(isResponseItem);
}

/** One answer of the portal, in the shape the contract names. */
function isResponseItem(value: unknown): value is ResponseItem {
	if (!value || typeof value !== "object" || Array.isArray(value)) return false;
	const item = value as { id?: unknown; value?: unknown; attachments?: unknown };
	if (typeof item.id !== "string" || item.value === undefined) return false;
	return item.attachments === undefined || Array.isArray(item.attachments);
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
		// The questions are parsed before the call, so a broken file or bad JSON
		// is a tool error and never reaches the portal.
		const file = loadQuestions(params.questions, cwd);
		let created: { id?: unknown };
		try {
			// The images of the form are uploaded first, so the browser loads
			// them from the portal instead of from the agent's own disk.
			const questions = await this.uploadMedia(file, cwd, signal);
			created = await this.client.json<{ id?: unknown }>("POST", "/api/questions", {
				questions, pane, session, cwd,
			}, signal);
		} catch (error) {
			if (!(error instanceof PortalDown)) throw error;
			// The portal is down, so the form goes to the one-shot interview
			// with the questions as the model wrote them.
			return this.fallback(file, params, ctx, signal);
		}
		const id = typeof created?.id === "string" ? created.id : "";
		if (!id) throw new Error("dogsled web did not return an interview id");

		if (params.async) {
			// The form runs on its own: the tool's own signal no longer owns it, and the
			// answers arrive as a message, like pi-interview does.
			const own = new AbortController();
			this.pending.add(own);
			void this.wait(id, file, own.signal, params.timeout)
				.then(answers => { this.deliver(`Interview ${id} finished.\n\n${answers.text}`); })
				.catch(error => { this.deliver(`Interview ${id} did not finish: ${error instanceof Error ? error.message : String(error)}`); })
				.finally(() => { this.pending.delete(own); });
			return {
				content: [{ type: "text", text: `Interview ${id} started. The answer arrives later as a message; keep working and do not poll.` }],
				details: { status: "started", responses: [], url: "" },
			};
		}

		try {
			const answers = await this.wait(id, file, signal, params.timeout);
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

	/** Run the same form on a temporary server when the portal cannot be reached. */
	private async fallback(file: QuestionsFile, params: { timeout?: number; async?: boolean }, ctx: ExtensionToolContext, signal: AbortSignal | undefined): Promise<AgentToolResult<InterviewDetails>> {
		const directory = await mkdtemp(join(tmpdir(), "dogsled-interview-"));
		const own = new AbortController();
		const abort = params.async || !signal ? own.signal : AbortSignal.any([own.signal, signal]);
		this.pending.add(own);
		let background = false;
		try {
			const path = join(directory, "questions.json");
			await writeFile(path, JSON.stringify(file), { mode: 0o600 });
			const command = startOneShot([
				"interview", "--file", path, "--pane", process.env["HERDR_PANE_ID"] ?? "",
				"--session", ctx.sessionManager.getSessionId(), "--cwd", ctx.cwd,
				"--timeout", String(Math.max(1, Math.ceil(params.timeout ?? 600))),
			], this.client.env, abort);
			await command.started;
			const answer = command.result.then(stdout => parseAnswers(JSON.parse(stdout)));
			if (params.async) {
				const id = randomUUID();
				background = true;
				void answer.finally(async () => {
					this.pending.delete(own);
					await rm(directory, { recursive: true, force: true });
				}).then(responses => {
					this.deliver(`Interview ${id} finished.\n\n${formatAnsweredResponses(responses, formQuestions(file))}`);
				}).catch(error => {
					if (!own.signal.aborted) this.deliver(`Interview ${id} did not finish: ${error instanceof Error ? error.message : String(error)}`);
				});
				return {
					content: [{ type: "text", text: `Interview ${id} started. The answer arrives later as a message; keep working and do not poll.` }],
					details: { status: "started", responses: [], url: "" },
				};
			}
			const responses = await answer;
			return {
				content: [{ type: "text", text: `User completed the interview form.\n\nAnswered responses:\n${formatAnsweredResponses(responses, formQuestions(file))}` }],
				details: { status: "completed", responses, url: "" },
			};
		} catch (error) {
			if (abort.aborted) return { content: [{ type: "text", text: "Interview was aborted." }], details: { status: "aborted", responses: [], url: "" } };
			throw error;
		} finally {
			if (!background) {
				this.pending.delete(own);
				await rm(directory, { recursive: true, force: true });
			}
		}
	}

	/** Close the background waits of the async interviews. */
	close(): void {
		for (const controller of this.pending) controller.abort();
		this.pending.clear();
	}

	/**
	 * The form with its local images uploaded to the portal.
	 *
	 * An image whose `src` names a file on this host is a path the browser
	 * cannot reach, because the file lives on the agent's machine and the
	 * browser runs on the owner's. The bytes go to the portal as an
	 * attachment, and the `src` becomes the URL the portal serves.
	 *
	 * The portal is asked whether it is there before the first local file is
	 * read, so a portal that is down falls back to the one-shot interview
	 * with the questions as the model wrote them, and a missing file stays a
	 * tool error only when there is a portal to upload it to.
	 */
	private async uploadMedia(file: QuestionsFile, cwd: string, signal: AbortSignal | undefined): Promise<QuestionsFile> {
		const list = file["questions"];
		if (!Array.isArray(list)) return file;
		const local = this.localImages(list, cwd);
		if (local.length === 0) return file;
		// The portal that would hold the images has to be there.
		await this.client.request("GET", "/api/questions", { signal });
		const questions: Record<string, unknown>[] = [];
		for (const entry of list) {
			if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
				questions.push(entry as Record<string, unknown>);
				continue;
			}
			const question: Record<string, unknown> = { ...(entry as Record<string, unknown>) };
			const media = question["media"];
			if (!media || typeof media !== "object" || Array.isArray(media)) {
				questions.push(question);
				continue;
			}
			const block = media as Record<string, unknown>;
			const src = block["src"];
			if (block["type"] !== "image" || typeof src !== "string" || !src || !isLocalSource(src)) {
				questions.push(question);
				continue;
			}
			const path = resolveQuestionPath(src, cwd);
			const bytes = readImage(path);
			const url = await this.uploadAttachment(bytes, path, signal);
			question["media"] = { ...block, src: url };
			questions.push(question);
		}
		return { ...file, questions };
	}

	/** The local images of a form, as the paths they name. */
	private localImages(list: unknown[], cwd: string): string[] {
		const paths: string[] = [];
		for (const entry of list) {
			if (!entry || typeof entry !== "object" || Array.isArray(entry)) continue;
			const media = (entry as Record<string, unknown>)["media"];
			if (!media || typeof media !== "object" || Array.isArray(media)) continue;
			const block = media as Record<string, unknown>;
			const src = block["src"];
			if (block["type"] !== "image" || typeof src !== "string" || !src || !isLocalSource(src)) continue;
			paths.push(resolveQuestionPath(src, cwd));
		}
		return paths;
	}

	/**
	 * The bytes of an image, as an attachment of the portal.
	 *
	 * A portal that cannot be reached is a `PortalDown` error, so the form
	 * falls back to the one-shot interview. A portal that refuses the image
	 * is an error that names the file.
	 */
	private async uploadAttachment(bytes: Buffer, path: string, signal: AbortSignal | undefined): Promise<string> {
		let response: Response;
		const init: RequestInit = {
			method: "POST",
			headers: {
				authorization: `Bearer ${this.client.token()}`,
				"content-type": "application/octet-stream",
			},
			body: new Uint8Array(bytes),
		};
		if (signal) init.signal = signal;
		try {
			response = await fetch(`http://127.0.0.1:${this.client.port}/api/attachments`, init);
		} catch (error) {
			if (signal?.aborted) throw error;
			const cause = error instanceof Error ? error.cause : undefined;
			const detail = cause instanceof Error ? cause.message : error instanceof Error ? error.message : String(error);
			throw new PortalDown(detail);
		}
		if (!response.ok) {
			throw new Error(`Cannot use the image ${path}: dogsled web POST /api/attachments failed: ${response.status} ${(await response.text()).trim()}`);
		}
		const payload = (await response.json()) as { path?: unknown };
		if (typeof payload.path !== "string" || !payload.path) {
			throw new Error(`Cannot use the image ${path}: dogsled web did not return an attachment path`);
		}
		return attachmentUrl(payload.path);
	}

	/** Poll the portal until it has the answers, the form is withdrawn, or the wait ends. */
	private async wait(id: string, file: QuestionsFile, signal: AbortSignal | undefined, timeout: number | undefined): Promise<{ text: string; responses: ResponseItem[] }> {
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
			const responses = parseAnswers(await response.json());
			return { text: formatAnsweredResponses(responses, formQuestions(file)), responses };
		}
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
