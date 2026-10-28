// Interviews live in the portal inbox. The owner opens the portal; asking a
// question never opens a browser or starts a temporary form server.
import { readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import type { AgentToolResult, AgentToolUpdateCallback, ExtensionAPI, ExtensionToolContext } from "@earendil-works/pi-coding-agent";
import { PortalClient, PortalDown, portalUrl, sleep } from "./portal-client.ts";

/** How long the tool waits between two polls for the answers. */
const POLL_MS = 500;

/** The parameter schema of pi-interview 0.13.0, so the model calls it the same way. */
const interviewParameters = {
	type: "object",
	required: ["questions"],
	properties: {
		questions: { type: "string", description: "Inline JSON string with questions, or path to a questions JSON file" },
		timeout: { type: "number", description: "Optional timeout in seconds. Without it, wait until the user answers or rejects the interview.", minimum: 1 },
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

/** The interview description with the portal's browser transport. */
const interviewDescription =
	"Present an interactive form to gather user responses. " +
	"Adds the form to the Dogsled portal inbox without opening a browser tab. Assume the user can open the portal. " +
	"The user can answer or reject the interview with a reason. No timeout applies unless you explicitly set one. " +
	"Use proactively when: choosing between multiple approaches, gathering requirements before implementation, " +
	"exploring design tradeoffs, or when decisions have multiple dimensions worth discussing. " +
	"Provides better UX than back-and-forth chat for structured input. " +
	"Image responses and attachments are returned as file paths - use read tool directly to display them. " +
	"Pass questions as inline JSON string directly (preferred) or as a path to a JSON file. " +
	'Questions JSON format: { "title": "...", "description": "...", "questions": [{ "id": "q1", "type": "single|multi|text|image|info", "question": "...", "options": ["A", "B"], "content": { "source": "...", "lang": "ts" }, "media": { "type": "image|chart|mermaid|table|html", ... } }] }. ' +
	"Options can be strings or objects: { label: string, content?: { source, lang?, file?, lines?, highlights?, title?, showSource? } }. " +
	"Every single and multi question automatically offers Other with a free-text answer; selected options also accept extra context. " +
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
	value: string | { option: string; note?: string } | (string | { option: string; note?: string })[];
	attachments?: string[];
}

/** The result shape of pi-interview 0.13.0. */
export interface InterviewDetails {
	status: "completed" | "rejected" | "cancelled" | "timeout" | "aborted" | "queued" | "started";
	responses: ResponseItem[];
	reason?: string;
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
	if (Array.isArray(value)) return value.map(formatResponseValue).join(", ");
	if (isChoiceValue(value)) return value.note ? `${value.option} (${value.note})` : value.option;
	return value;
}

function hasAnswerValue(value: ResponseItem["value"]): boolean {
	if (Array.isArray(value)) return value.some(hasAnswerValue);
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
function parseAnswers(value: unknown): { responses: ResponseItem[]; rejection?: { reason: string } } {
	const payload = value as { id?: unknown; responses?: unknown; rejection?: unknown } | null;
	if (!payload || typeof payload !== "object" || Array.isArray(payload) || typeof payload.id !== "string" || !Array.isArray(payload.responses)) {
		throw new Error('dogsled web returned an unexpected answer shape: expected {"id": string, "responses": ResponseItem[]}');
	}
	if (!payload.responses.every(isResponseItem)) throw new Error("dogsled web returned an invalid response item");
	if (payload.rejection !== undefined) {
		const rejection = payload.rejection as { reason?: unknown } | null;
		if (!rejection || typeof rejection !== "object" || typeof rejection.reason !== "string" || payload.responses.length !== 0) {
			throw new Error("dogsled web returned an invalid rejection");
		}
		return { responses: [], rejection: { reason: rejection.reason } };
	}
	return { responses: payload.responses };
}

/** One answer of the portal, in the shape the contract names. */
function isResponseItem(value: unknown): value is ResponseItem {
	if (!value || typeof value !== "object" || Array.isArray(value)) return false;
	const item = value as { id?: unknown; value?: unknown; attachments?: unknown };
	if (typeof item.id !== "string" || item.value === undefined) return false;
	return item.attachments === undefined || Array.isArray(item.attachments);
}

interface Outcome {
	status: "completed" | "rejected" | "cancelled" | "timeout" | "aborted";
	text: string;
	responses: ResponseItem[];
	reason?: string;
}

/** The `interview` tool: ask the user a form through the portal and wait for the answers. */
export class InterviewTool {
	private readonly client: PortalClient;
	private readonly deliver: (content: string) => void;
	/** The background waits of the async interviews, so the session can close them. */
	private readonly pending = new Map<AbortController, Promise<Outcome>>();
	constructor(client: PortalClient = new PortalClient(), deliver: (content: string) => void = () => undefined) {
		this.client = client;
		this.deliver = deliver;
	}

	/** Run one interview. The tool's `signal` withdraws the form. */
	async run(params: { questions: string; timeout?: number; verbose?: boolean; async?: boolean }, ctx: ExtensionToolContext, signal: AbortSignal | undefined, onUpdate?: AgentToolUpdateCallback<InterviewDetails>): Promise<AgentToolResult<InterviewDetails>> {
		const cwd = ctx.cwd;
		const session = ctx.sessionManager.getSessionId();
		const pane = process.env["HERDR_PANE_ID"] ?? "";
		// The questions are parsed before the call, so a broken file or bad JSON
		// is a tool error and never reaches the portal.
		const file = loadQuestions(params.questions, cwd);
		if (params.timeout !== undefined && (!Number.isFinite(params.timeout) || params.timeout < 1 || params.timeout > 2_147_483)) {
			throw new Error("Interview timeout must be between 1 and 2147483 seconds.");
		}
		const uploaded = await this.uploadMedia(file, cwd, signal);
		const created = await this.client.json<{ id?: unknown }>("POST", "/api/questions", {
			questions: uploaded, pane, session, cwd,
		}, signal);
		const id = typeof created?.id === "string" ? created.id : "";
		if (!id) throw new Error("dogsled web did not return an interview id");
		const url = portalUrl(`/?question=${encodeURIComponent(id)}&pane=${encodeURIComponent(pane)}`, this.client.env);
		onUpdate?.({
			content: [{ type: "text", text: `Interview ${id} is in the Dogsled inbox: ${url}. Waiting for answers or rejection.` }],
			details: { status: "started", responses: [], url },
		});

		const own = new AbortController();
		const operation = params.async || !signal ? own.signal : AbortSignal.any([own.signal, signal]);
		if (signal?.aborted) own.abort();
		const answer = this.wait(id, file, operation, params.timeout)
			.finally(() => { this.pending.delete(own); });
		this.pending.set(own, answer);
		if (params.async) {
			void answer.then(outcome => {
				if (!own.signal.aborted) this.deliver(`Interview ${id} ${outcome.status}.\n\n${outcome.text}`);
			}).catch(error => {
				if (!own.signal.aborted) this.deliver(`Interview ${id} did not finish: ${error instanceof Error ? error.message : String(error)}`);
			});
			return {
				content: [{ type: "text", text: `Interview ${id} is in the Dogsled inbox: ${url}. The outcome arrives later as a message; keep working and do not poll.` }],
				details: { status: "started", responses: [], url },
			};
		}
		const { text, ...details } = await answer;
		return { content: [{ type: "text", text: `${text}\n\n${url}` }], details: { ...details, url } };
	}

	/** Finish withdrawal before the extension runtime exits or is replaced. */
	async close(): Promise<void> {
		for (const controller of this.pending.keys()) controller.abort();
		const results = await Promise.allSettled(this.pending.values());
		this.pending.clear();
		for (const result of results) {
			if (result.status === "rejected") throw result.reason;
		}
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
	 * read. If it is down, report that directly without starting another server.
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
	 * A portal that cannot be reached is a `PortalDown` error.
	 * A portal that refuses the image is an error that names the file.
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

	/** Resolve saved outcomes consistently in sync, async, and cancellation races. */
	private outcome(payload: unknown, file: QuestionsFile): Outcome {
		const { responses, rejection } = parseAnswers(payload);
		if (rejection) return {
			status: "rejected", responses: [], reason: rejection.reason,
			text: `User rejected the interview. Do not treat this as approval.\n\nReason: ${rejection.reason || "No reason given."}`,
		};
		return { status: "completed", responses, text: `User completed the interview form.\n\nAnswered responses:\n${formatAnsweredResponses(responses, formQuestions(file))}` };
	}

	/** Cancel abandoned forms, but recover a terminal outcome that won the race. */
	private async withdraw(id: string, file: QuestionsFile): Promise<Outcome | undefined> {
		const signal = AbortSignal.timeout(5_000);
		const path = `/api/questions/${encodeURIComponent(id)}`;
		const response = await this.client.request("DELETE", path, { signal });
		if (response.status === 409) {
			const saved = await this.client.json<unknown>("GET", `${path}/answer?wait=0`, undefined, signal);
			return this.outcome(saved, file);
		}
		if (!response.ok && response.status !== 410) throw new Error(`Could not withdraw interview ${id}: HTTP ${response.status}`);
		return undefined;
	}

	/** No implicit deadline: the owner can leave the form in the inbox until ready. */
	private async wait(id: string, file: QuestionsFile, signal: AbortSignal, timeout: number | undefined): Promise<Outcome> {
		const deadline = new AbortController();
		const timer = timeout === undefined ? undefined : setTimeout(() => deadline.abort(), timeout * 1000);
		timer?.unref();
		const operation = AbortSignal.any([signal, deadline.signal]);
		try {
			for (;;) {
				operation.throwIfAborted();
				const response = await this.client.request("GET", `/api/questions/${encodeURIComponent(id)}/answer`, { signal: operation });
				if (response.status === 204) {
					await sleep(POLL_MS, operation);
					continue;
				}
				if (response.status === 410) return { status: "cancelled", responses: [], text: "The interview was withdrawn." };
				if (!response.ok) throw new Error(`dogsled web GET /api/questions/${id}/answer failed: ${response.status} ${(await response.text()).trim()}`);
				return this.outcome(await response.json(), file);
			}
		} catch (error) {
			const saved = await this.withdraw(id, file);
			if (saved) return saved;
			if (signal.aborted) return { status: "aborted", responses: [], text: "Interview was aborted." };
			if (deadline.signal.aborted) return { status: "timeout", responses: [], text: `Interview timed out after ${timeout} seconds and was removed from the inbox.` };
			throw error;
		} finally {
			if (timer) clearTimeout(timer);
		}
	}
}

/** Register the `interview` tool. */
export function registerInterview(pi: ExtensionAPI, client: PortalClient = new PortalClient()): InterviewTool {
	const tool = new InterviewTool(client, content => { pi.sendUserMessage(content, { deliverAs: "steer" }); });
	pi.registerTool({
		name: "interview",
		label: "Interview",
		description: interviewDescription,
		promptSnippet: "Gather structured user input through an interactive form for requirements, tradeoffs, or multi-dimensional decisions.",
		parameters: interviewParameters,
		async execute(_toolCallId, params, signal, onUpdate, ctx) {
			const question = params as { questions?: unknown };
			if (typeof question.questions !== "string" || !question.questions) throw new Error("interview needs the questions, as inline JSON or a path to a questions file");
			return await tool.run(question as { questions: string }, ctx, signal, onUpdate);
		},
	});
	return tool;
}
