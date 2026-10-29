// Plan mode: the model explores read-only, writes one markdown plan, and sends
// it to the portal for review. The decision comes back later, as a message.
// While planning, the gate stops the model from changing anything but a
// markdown file. It is a guard against mistakes, not a sandbox: it can be
// wrong, and it does not cover every tool that can change a file.
import { readFile } from "node:fs/promises";
import { extname, isAbsolute, relative, resolve, sep } from "node:path";
import type { ExtensionAPI, ExtensionContext, ToolCallEvent, ToolCallEventResult } from "@earendil-works/pi-coding-agent";
import { Key } from "@earendil-works/pi-tui";
import { openInBrowser, PortalClient, PortalDown, portalUrl, sleep, startOneShot } from "./portal-client.ts";

/** The tool that sends a plan to the portal. */
const SUBMIT_PLAN = "submit_plan";

/** The custom message the framing note joins the conversation as. */
const PLAN_MODE_MESSAGE = "dogsled:plan-mode";

/** The file types the gate lets the model write while planning. */
const PLAN_EXTENSIONS = new Set([".md", ".mdx"]);

/** The message the model reads when planning starts. */
const PLANNING_MESSAGE =
	"Plan mode is on. Explore the repository read-only: read files, search, and run read-only commands. " +
	"Write the plan as one markdown file in the working directory, then call submit_plan with that file. " +
	"Do not change any other file until the plan is approved.";

/** The bash commands the gate blocks while planning. */
const BLOCKED_COMMANDS = ["tee", "rm", "mv", "cp", "mkdir", "touch", "sed -i", "git commit", "git push", "git checkout", "git reset", "npm install"];

/** The portal's decision about one plan. */
export interface Decision {
	review_id: string;
	path: string;
	decision: "approved" | "changes_requested";
	message: string;
	plan_content: string;
}

/** The approved document and reviewer feedback passed to pi-todo. */
export interface PlanApproved {
	cwd: string;
	planFilePath: string;
	planContent: string;
	feedback?: string;
}

/** The parts of the extension API that plan mode uses. */
export interface PlanHost {
	sendUserMessage(content: string, options?: { deliverAs?: "steer" | "followUp" }): void;
	sendMessage(message: { customType: string; content: string; display: boolean }, options?: { triggerTurn?: boolean; deliverAs?: "steer" | "followUp" }): void;
	getActiveTools(): string[];
	setActiveTools(names: string[]): void;
}

/** Whether a path stays inside the working directory and ends in a plan file type. */
export function planPathAllowed(path: string, cwd: string): boolean {
	if (!path) return false;
	const target = isAbsolute(path) ? resolve(path) : resolve(cwd, path);
	const inside = relative(cwd, target);
	if (inside.startsWith("..") || inside.split(sep).includes("..")) return false;
	return PLAN_EXTENSIONS.has(extname(target).toLowerCase());
}

/** The quoted parts of a command, which are text and not shell syntax. */
function withoutQuoted(command: string): string {
	return command.replace(/'[^']*'/g, "''").replace(/"[^"]*"/g, '""');
}

/** Why a bash command is blocked while planning, or undefined when it is allowed. */
export function bashBlocked(command: string): string | undefined {
	const text = withoutQuoted(command);
	// Every write redirect, unless it only throws the output away or duplicates a descriptor.
	for (const match of text.matchAll(/>>?/g)) {
		const rest = text.slice(match.index + match[0].length).trimStart();
		if (rest.startsWith("/dev/null") || rest.startsWith("&")) continue;
		return "a write redirect";
	}
	for (const blocked of BLOCKED_COMMANDS) {
		const pattern = new RegExp(`(?:^|[;&|(\\s])${blocked.replace(/[ -]/g, "\\$&")}(?![\\w-])`, "m");
		if (pattern.test(text)) return blocked;
	}
	return undefined;
}

/** The path a write or edit tool call targets. */
function targetPath(input: unknown): string {
	if (typeof input !== "object" || input === null) return "";
	const path = (input as { path?: unknown }).path;
	return typeof path === "string" ? path : "";
}

/** The command a bash tool call runs. */
function bashCommand(input: unknown): string {
	if (typeof input !== "object" || input === null) return "";
	const command = (input as { command?: unknown }).command;
	return typeof command === "string" ? command : "";
}

/** The reason a tool call is blocked while planning, or undefined when it is allowed. */
function planningBlock(event: ToolCallEvent, cwd: string): string | undefined {
	if (event.toolName === "write" || event.toolName === "edit") {
		const path = targetPath(event.input);
		if (!planPathAllowed(path, cwd)) return `Planning is on: ${event.toolName} may only change a markdown file (.md or .mdx) inside ${cwd}. The target was ${path || "(no path)"}.`;
		return undefined;
	}
	if (event.toolName === "bash") {
		const blocked = bashBlocked(bashCommand(event.input));
		if (blocked) return `Planning is on: this command uses ${blocked}, which changes files. Explore read-only until the plan is approved.`;
	}
	return undefined;
}

interface ReviewCreated { id: string; revision: number }

/** The plan-mode state of one Pi session. */
export class PlanMode {
	private readonly pi: PlanHost;
	private readonly client: PortalClient;
	private readonly open: (url: string) => void;
	private readonly approved: (plan: PlanApproved, ctx: ExtensionContext) => void;
	private planning = false;
	private framed = false;
	private watching: AbortController | undefined;
	private readonly pending = new Set<AbortController>();
	/** The reviews this session already delivered, so a reconnect never delivers one twice. */
	private readonly delivered = new Set<string>();
	/** The reviews the portal acknowledged. A review that was delivered but not acked is acked again, not delivered again. */
	private readonly acked = new Set<string>();

	constructor(pi: PlanHost, approved: (plan: PlanApproved, ctx: ExtensionContext) => void, client: PortalClient = new PortalClient(), open: (url: string) => void = openInBrowser) {
		this.pi = pi;
		this.approved = approved;
		this.client = client;
		this.open = open;
	}

	/** Whether the model is planning. */
	get active(): boolean { return this.planning; }

	/** Enter or leave planning. Entering frames the task once and shows the submit tool. */
	toggle(): void {
		if (this.planning) this.stop();
		else this.start();
	}

	/** Enter planning: frame the task, and make the submit tool the only plan tool. */
	start(): void {
		this.planning = true;
		this.activate();
		if (this.framed) return;
		this.framed = true;
		// A conversation message, not a system prompt change: the plan mode frames the task
		// for this turn only, and the system prompt stays as the session wrote it.
		this.pi.sendMessage({ customType: PLAN_MODE_MESSAGE, content: PLANNING_MESSAGE, display: true }, { triggerTurn: true });
	}

	/** Leave planning: hide the submit tool. */
	stop(): void {
		this.planning = false;
		this.deactivate();
	}

	/** The tool call gate. It blocks changes while planning and lets everything else through. */
	gate(event: ToolCallEvent, ctx: ExtensionContext): ToolCallEventResult | undefined {
		if (!this.planning) return undefined;
		const reason = planningBlock(event, ctx.cwd);
		if (!reason) return undefined;
		return { block: true, reason };
	}

	/** Send a plan to the portal and start waiting for its decision. */
	async submit(filePath: string, ctx: ExtensionContext): Promise<(ReviewCreated & { url: string }) | { fallback: true }> {
		const path = isAbsolute(filePath) ? filePath : resolve(ctx.cwd, filePath);
		const content = await readFile(path, "utf8");
		let created: ReviewCreated;
		try {
			created = await this.client.json<ReviewCreated>("POST", "/api/reviews", {
				path, content, pane: process.env["HERDR_PANE_ID"] ?? "",
				session: ctx.sessionManager.getSessionId(), cwd: ctx.cwd,
			});
		} catch (error) {
			if (!(error instanceof PortalDown)) throw error;
			await this.fallback(path, ctx);
			return { fallback: true };
		}
		const id = typeof created?.id === "string" ? created.id : "";
		const revision = Number(created?.revision);
		if (!id || !Number.isFinite(revision)) throw new Error("dogsled web did not return a review id and revision");
		const url = portalUrl(`/?review=${encodeURIComponent(id)}`, this.client.env);
		if (revision === 1) this.open(url);
		this.watch(ctx);
		return { id, revision, url };
	}

	/** Start a temporary review server without holding the tool call open. */
	private async fallback(path: string, ctx: ExtensionContext): Promise<void> {
		const controller = new AbortController();
		this.pending.add(controller);
		const command = startOneShot([
			"review", "--file", path, "--pane", process.env["HERDR_PANE_ID"] ?? "",
			"--session", ctx.sessionManager.getSessionId(), "--cwd", ctx.cwd,
		], this.client.env, controller.signal);
		try { await command.started; }
		catch (error) { this.pending.delete(controller); throw error; }
		void command.result.then(stdout => this.decision(stdout, ctx, false))
			.catch(error => {
				if (!controller.signal.aborted) {
					const message = `Plan review did not finish: ${error instanceof Error ? error.message : String(error)}`;
					this.pi.sendUserMessage(message, ctx.isIdle() ? undefined : { deliverAs: "steer" });
				}
			})
			.finally(() => { this.pending.delete(controller); });
	}

	/** Hold one decisions stream open until the session ends. */
	private watch(ctx: ExtensionContext): void {
		if (this.watching) return;
		const controller = new AbortController();
		this.watching = controller;
		const session = ctx.sessionManager.getSessionId();
		void this.stream(session, controller.signal, ctx);
	}

	/** Close the decisions stream. */
	stopWatching(): void {
		this.watching?.abort();
		this.watching = undefined;
		for (const controller of this.pending) controller.abort();
		this.pending.clear();
	}

	private async stream(session: string, signal: AbortSignal, ctx: ExtensionContext): Promise<void> {
		let backoff = 500;
		while (!signal.aborted) {
			try {
				const events = await this.client.events(`/api/sessions/${encodeURIComponent(session)}/decisions`, signal);
				for await (const event of events) {
					if (signal.aborted) break;
					if (event.event !== "decision") continue;
					await this.decision(event.data, ctx);
				}
				backoff = 500;
			} catch {
				// The portal may be restarting. Reconnect with a longer wait.
			}
			if (signal.aborted) break;
			await sleep(backoff, signal);
			backoff = Math.min(backoff * 2, 30_000);
		}
	}

	/** Deliver one decision, acknowledge it, and hand the approved plan on. */
	async decision(payload: string, ctx: ExtensionContext, acknowledge = true): Promise<void> {
		let decision: Partial<Decision> | null;
		try { decision = JSON.parse(payload) as Partial<Decision> | null; }
		catch {
			if (!acknowledge) throw new Error("dogsled review returned invalid decision JSON");
			return;
		}
		if (!decision || typeof decision.review_id !== "string" || !decision.review_id
			|| (decision.decision !== "approved" && decision.decision !== "changes_requested")
			|| typeof decision.path !== "string" || typeof decision.message !== "string" || typeof decision.plan_content !== "string") {
			if (!acknowledge) throw new Error("dogsled review returned an unexpected decision shape");
			return;
		}
		const review = decision.review_id;
		// A review the portal did not acknowledge is acked again, never delivered again.
		if (!this.delivered.has(review)) {
			this.delivered.add(review);
			const message = typeof decision.message === "string" ? decision.message : "";
			if (message) {
				// A running agent is steered, so the message joins the turn that is running.
				if (ctx.isIdle()) this.pi.sendUserMessage(message);
				else this.pi.sendUserMessage(message, { deliverAs: "steer" });
			}
			if (decision.decision === "approved") {
				const approved: PlanApproved = {
					cwd: ctx.cwd,
					planFilePath: typeof decision.path === "string" ? decision.path : "",
					planContent: typeof decision.plan_content === "string" ? decision.plan_content : "",
					...(message ? { feedback: message } : {}),
				};
				this.approved(approved, ctx);
				this.stop();
			}
		}
		// The one-shot command acknowledges its own decision before exiting.
		if (!acknowledge || this.acked.has(review)) return;
		await this.client.json<unknown>("POST", `/api/decisions/${encodeURIComponent(review)}/ack`).catch(() => undefined);
		this.acked.add(review);
	}

	/** Show the submit tool while planning. */
	private activate(): void {
		const active = this.pi.getActiveTools();
		if (!active.includes(SUBMIT_PLAN)) this.pi.setActiveTools([...active, SUBMIT_PLAN]);
	}

	/** Hide the submit tool again. */
	private deactivate(): void {
		const active = this.pi.getActiveTools();
		if (active.includes(SUBMIT_PLAN)) this.pi.setActiveTools(active.filter(name => name !== SUBMIT_PLAN));
	}
}

/** Register the plan mode: the submit tool, the command, and the shortcut. */
export function registerPlanMode(pi: ExtensionAPI, approved: (plan: PlanApproved, ctx: ExtensionContext) => void, client: PortalClient = new PortalClient(), open: (url: string) => void = openInBrowser): PlanMode {
	const host: PlanHost = {
		sendUserMessage: (content, options) => { pi.sendUserMessage(content, options); },
		sendMessage: (message, options) => { pi.sendMessage(message, options); },
		getActiveTools: () => pi.getActiveTools(),
		setActiveTools: names => { pi.setActiveTools(names); },
	};
	const mode = new PlanMode(host, approved, client, open);
	pi.registerTool({
		name: SUBMIT_PLAN,
		label: "Submit plan",
		description: "Send the plan file to the portal for review. Write the plan as one markdown file in the working directory, then call this with its path. The decision arrives later as a message.",
		promptSnippet: "Send a written plan for review and wait for the decision.",
		parameters: {
			type: "object",
			required: ["filePath"],
			properties: { filePath: { type: "string", description: "Path of the markdown plan file, absolute or relative to the working directory" } },
		} as const,
		// The tool belongs to planning only, so it is declared to the model while planning runs.
		defaultActive: false,
		executionMode: "sequential",
		async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
			const filePath = (params as { filePath?: unknown }).filePath;
			if (typeof filePath !== "string" || !filePath) throw new Error("submit_plan needs the path of the plan file");
			const review = await mode.submit(filePath, ctx);
			return {
				content: [{ type: "text", text: `${"fallback" in review ? "Started a review in a browser tab" : `Pushed as revision ${review.revision}: ${review.url}`}; the decision arrives later as a message; end your turn.` }],
				details: review,
			};
		},
	});
	pi.registerCommand("plan", {
		description: "Toggle plan mode (Ctrl+Alt+P)",
		handler: async () => { mode.toggle(); },
	});
	pi.registerShortcut(Key.ctrlAlt("p"), {
		description: "Toggle plan mode",
		handler: async () => { mode.toggle(); },
	});
	return mode;
}
