// Live model settings use Pi's setters, independently of the activity protocol.
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { sleep, type PortalClient } from "./portal-client.ts";

type Thinking = ReturnType<ExtensionAPI["getThinkingLevel"]>;
interface ModelRef {
  provider: string;
  id: string;
}
interface Evidence {
  session_file: string;
  session_id: string;
}
interface Settings {
  model: ModelRef | null;
  thinking: Thinking;
  models: { model: ModelRef; name: string; thinking: Thinking[] }[];
}
interface Change {
  model?: ModelRef;
  thinking?: Thinking;
}
interface Request {
  id: string;
  evidence: Evidence;
  change: Change;
}
type Client = Pick<PortalClient, "json" | "events">;
const levels = new Set<string>(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;
const text = (value: unknown, max: number): value is string =>
  typeof value === "string" &&
  value.length > 0 &&
  Buffer.byteLength(value) <= max &&
  !/\p{Cc}/u.test(value);
const modelRef = (value: unknown): value is ModelRef =>
  record(value) && text(value["provider"], 256) && text(value["id"], 256);
const thinking = (value: unknown): value is Thinking =>
  typeof value === "string" && levels.has(value);

function request(value: unknown): value is Request {
  if (
    !record(value) ||
    !text(value["id"], 128) ||
    !record(value["evidence"]) ||
    !record(value["change"])
  )
    return false;
  const evidence = value["evidence"],
    change = value["change"];
  return (
    text(evidence["session_file"], 4096) &&
    text(evidence["session_id"], 1024) &&
    (change["model"] === undefined || modelRef(change["model"])) &&
    (change["thinking"] === undefined || thinking(change["thinking"])) &&
    (change["model"] !== undefined || change["thinking"] !== undefined)
  );
}

/** One subscription and publication lifetime per native session. */
export class SessionSettings {
  private readonly controller = new AbortController();
  private publication: Promise<void> = Promise.resolve();
  private watching = false;
  private readonly path: string;

  private readonly pi: Pick<ExtensionAPI, "setModel" | "setThinkingLevel" | "getThinkingLevel">;
  private readonly client: Client;
  private readonly evidence: Evidence;
  private readonly ctx: ExtensionContext;

  constructor(
    pi: Pick<ExtensionAPI, "setModel" | "setThinkingLevel" | "getThinkingLevel">,
    client: Client,
    pane: string,
    evidence: Evidence,
    ctx: ExtensionContext,
  ) {
    this.pi = pi;
    this.client = client;
    this.evidence = evidence;
    this.ctx = ctx;
    this.path = `/api/agents/${encodeURIComponent(pane)}/settings`;
  }

  stop(): void {
    this.controller.abort();
  }

  private current(): Settings {
    const models = this.ctx.scopedModels.length
      ? this.ctx.scopedModels.map((choice) => choice.model)
      : this.ctx.modelRegistry.getAvailable();
    const ref = (model: ModelRef): ModelRef => ({ provider: model.provider, id: model.id });
    return {
      model: this.ctx.model ? ref(this.ctx.model) : null,
      thinking: this.pi.getThinkingLevel(),
      models: models
        .filter(modelRef)
        .slice(0, 1000)
        .map((model) => ({
          model: ref(model),
          name: model.name.slice(0, 256),
          thinking: getSupportedThinkingLevels(model),
        })),
    };
  }

  /** Hook publications stay ordered; failures are retried by the subscription loop. */
  publish(): Promise<void> {
    this.publication = this.publication
      .catch(() => undefined)
      .then(async () => {
        if (this.controller.signal.aborted) return;
        await this.client.json(
          "PUT",
          this.path,
          { evidence: this.evidence, settings: this.current() },
          this.controller.signal,
        );
      });
    return this.publication;
  }

  start(): void {
    if (this.watching) return;
    this.watching = true;
    void this.stream();
  }

  private async stream(): Promise<void> {
    const signal = this.controller.signal;
    let delay = 250;
    const query = new URLSearchParams({ ...this.evidence });
    while (!signal.aborted) {
      try {
        await this.publish();
        const events = await this.client.events(`${this.path}/requests?${query}`, signal);
        for await (const event of events) {
          if (signal.aborted) break;
          if (event.event !== "settings") continue;
          await this.apply(JSON.parse(event.data));
          delay = 250;
        }
      } catch {
        // The portal may be absent at startup. A settings hook never fails the session.
      }
      if (!signal.aborted) await sleep(delay, signal);
      delay = Math.min(delay * 2, 10_000);
    }
  }

  async apply(value: unknown): Promise<void> {
    if (
      this.controller.signal.aborted ||
      !request(value) ||
      value.evidence.session_file !== this.evidence.session_file ||
      value.evidence.session_id !== this.evidence.session_id
    )
      return;
    let error: string | undefined;
    try {
      if (value.change.model) {
        const { provider, id } = value.change.model;
        const model = this.ctx.modelRegistry.find(provider, id);
        if (!model) throw new Error("unknown model");
        if (!(await this.pi.setModel(model))) throw new Error(`no credentials for ${provider}`);
      }
      if (this.controller.signal.aborted) return;
      if (value.change.thinking) this.pi.setThinkingLevel(value.change.thinking);
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    }
    await this.publication.catch(() => undefined);
    if (this.controller.signal.aborted) return;
    await this.client.json(
      "POST",
      `${this.path}/requests/${encodeURIComponent(value.id)}/result`,
      {
        evidence: this.evidence,
        ...(error ? { error } : { settings: this.current() }),
      },
      this.controller.signal,
    );
  }
}

export function registerSettings(pi: ExtensionAPI, client: Client, pane: string | undefined): void {
  let session: SessionSettings | undefined;
  pi.on("session_start", (_event, ctx) => {
    session?.stop();
    session = undefined;
    if (!pane || !text(pane, 128)) return;
    try {
      const session_file = ctx.sessionManager.getSessionFile(),
        session_id = ctx.sessionManager.getSessionId();
      if (!text(session_file, 4096) || !text(session_id, 1024)) return;
      session = new SessionSettings(pi, client, pane, { session_file, session_id }, ctx);
      session.start();
    } catch {
      /* Missing session evidence leaves settings unavailable. */
    }
  });
  pi.on("model_select", () => {
    void session?.publish().catch(() => undefined);
  });
  pi.on("thinking_level_select", () => {
    void session?.publish().catch(() => undefined);
  });
  pi.on("session_shutdown", () => {
    session?.stop();
    session = undefined;
  });
}
