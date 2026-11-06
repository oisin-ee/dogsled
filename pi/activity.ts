// Session-scoped telemetry. Hooks reduce compact facts; only the publisher performs I/O.
import { randomUUID } from "node:crypto";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { PortalClient } from "./portal-client.ts";

export type Phase = "idle" | "working" | "thinking" | "responding" | "tool_running";
export interface ActiveTool {
  call_id: string;
  name: string;
  parent_call_id: string | null;
}
export type Payload =
  | { kind: "snapshot"; state: { phase: Phase; active_tools: ActiveTool[] } }
  | { kind: "unavailable"; reason: "resyncing" | "overflow" }
  | { kind: "closed" };
export type ActivityEvent =
  | { type: "agent_start" | "agent_end" | "settled" | "message_end" }
  | { type: "block"; index: number; phase: "thinking" | "responding"; ended: boolean }
  | { type: "tool_start"; tool: ActiveTool }
  | { type: "tool_end"; id: string };

/** Preparation events never enter this reducer. Overflow is sticky until verified settlement. */
export class ActivityReducer {
  private blocks = new Map<number, "thinking" | "responding">();
  private tools = new Map<string, ActiveTool>();
  private gap: "resyncing" | "overflow" | undefined;
  private idle: boolean;

  constructor(idle: boolean) {
    this.idle = idle;
    if (!idle) this.gap = "resyncing";
  }

  reduce(event: ActivityEvent): Payload {
    if (event.type === "settled") {
      this.blocks.clear();
      this.tools.clear();
      this.gap = undefined;
      this.idle = true;
    } else if (!this.gap) {
      if (event.type === "agent_start" || event.type === "agent_end") this.idle = false;
      else if (event.type === "message_end") this.blocks.clear();
      else if (event.type === "block") {
        this.idle = false;
        this.blocks.delete(event.index);
        if (!event.ended) this.blocks.set(event.index, event.phase);
      } else if (event.type === "tool_start") {
        this.idle = false;
        this.tools.set(event.tool.call_id, { ...event.tool });
      } else if (event.type === "tool_end") this.tools.delete(event.id);
      if (this.blocks.size > 64 || this.tools.size > 64) {
        this.gap = "overflow";
        this.blocks.clear();
        this.tools.clear();
      }
    }
    return this.snapshot();
  }

  snapshot(): Payload {
    if (this.gap) return { kind: "unavailable", reason: this.gap };
    const active_tools = [...this.tools.values()].map((tool) => ({ ...tool }));
    const phase = active_tools.length
      ? "tool_running"
      : ([...this.blocks.values()].at(-1) ?? (this.idle ? "idle" : "working"));
    return { kind: "snapshot", state: { phase, active_tools } };
  }
}

interface Evidence {
  session_file: string;
  session_id: string;
}
interface Stamp {
  epoch: string;
  generation: number;
  producer_id: string;
  session: string;
}
interface Ack extends Stamp {
  version: number;
  pane: string;
  sequence: number;
  heartbeat_ms: number;
  ttl_ms: number;
}
interface Request {
  method: string;
  path: string;
  body?: unknown;
}
type Client = Pick<PortalClient, "request">;
const counter = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;
const text = (value: unknown, max: number): value is string =>
  typeof value === "string" &&
  Buffer.byteLength(value) > 0 &&
  Buffer.byteLength(value) <= max &&
  !/[\u0000-\u001f\u007f-\u009f]/u.test(value);

function acknowledgement(value: unknown, pane: string, producer: string): value is Ack {
  return (
    record(value) &&
    value["version"] === 1 &&
    value["pane"] === pane &&
    value["producer_id"] === producer &&
    text(value["epoch"], 256) &&
    text(value["session"], 16384) &&
    counter(value["generation"]) &&
    value["generation"] > 0 &&
    counter(value["sequence"]) &&
    value["heartbeat_ms"] === 5000 &&
    value["ttl_ms"] === 15000
  );
}

/** One immutable request in flight, one replaceable pending snapshot, no hook awaits. */
export class ActivityPublisher {
  readonly reducer: ActivityReducer;
  private readonly path: string;
  private readonly producer = randomUUID();
  private disposed = false;
  private stopped = false;
  private busy = false;
  private pending: Payload | undefined;
  private request: Request | undefined;
  private stamp: Stamp | undefined;
  private sequence = 0;
  private lastAttempt = -Infinity;
  private retryAt = -Infinity;
  private retryDelay = 500;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private heartbeat: ReturnType<typeof setInterval>;
  private controller: AbortController | undefined;
  private flight: Promise<void> | undefined;
  private readonly client: Client;
  private readonly pane: string;
  private readonly evidence: Evidence;

  constructor(client: Client, pane: string, evidence: Evidence, idle: boolean) {
    this.client = client;
    this.pane = pane;
    this.evidence = { ...evidence };
    this.path = `/api/agents/local/${encodeURIComponent(pane)}/activity`;
    this.reducer = new ActivityReducer(idle);
    this.pending = this.reducer.snapshot();
    this.request = { method: "GET", path: this.path };
    this.heartbeat = setInterval(() => {
      if (this.disposed || this.stopped) return;
      this.pending = this.reducer.snapshot();
      this.schedule();
    }, 5000);
    this.heartbeat.unref();
    this.schedule();
  }

  event(event: ActivityEvent): void {
    if (this.disposed || this.stopped) return;
    this.pending = this.reducer.reduce(event);
    this.schedule();
  }

  private schedule(): void {
    if (this.disposed || this.stopped || this.busy || this.timer) return;
    this.timer = setTimeout(
      () => {
        if (this.disposed) return;
        this.timer = undefined;
        this.flight = this.pump();
      },
      Math.max(0, this.retryAt - performance.now(), 250 - (performance.now() - this.lastAttempt)),
    );
    this.timer.unref();
  }

  private async send(
    request: Request,
    controller: AbortController,
  ): Promise<{ status: number; body: unknown }> {
    let deadline: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        (async () => {
          const response = await this.client.request(request.method, request.path, {
            body: request.body,
            signal: controller.signal,
          });
          return {
            status: response.status,
            body: response.ok
              ? ((await response.json()) as unknown)
              : ((await response.json().catch(() => undefined)) as unknown),
          };
        })(),
        new Promise<never>((_resolve, reject) => {
          deadline = setTimeout(() => {
            controller.abort();
            reject(new Error("activity request deadline"));
          }, 1000);
          deadline.unref();
        }),
      ]);
    } finally {
      if (deadline) clearTimeout(deadline);
    }
  }

  private async pump(): Promise<void> {
    if (this.disposed || this.stopped || this.busy) return;
    if (!this.request && this.stamp && this.pending) {
      this.request = {
        method: "PUT",
        path: this.path,
        body: {
          version: 1,
          ...this.stamp,
          sequence: ++this.sequence,
          payload: this.pending,
        },
      };
      this.pending = undefined;
    }
    const request = this.request;
    if (!request) return;
    this.busy = true;
    this.lastAttempt = performance.now();
    const controller = new AbortController();
    this.controller = controller;
    try {
      const { status, body } = await this.send(request, controller);
      if (this.disposed) return;
      if (status < 200 || status >= 300) {
        const code = record(body) && typeof body["error"] === "string" ? body["code"] : undefined;
        if (
          (request.method === "POST" &&
            (code === "session_changed" ||
              code === "session_unverified" ||
              code === "upstream_unavailable" ||
              code === "stale_generation")) ||
          (request.method === "PUT" &&
            (code === "stale_generation" ||
              code === "session_changed" ||
              code === "producer_closed"))
        ) {
          this.stamp = undefined;
          this.sequence = 0;
          this.pending = this.reducer.snapshot();
          this.request = { method: "GET", path: this.path };
          this.retryAt = performance.now() + this.retryDelay;
          this.retryDelay = Math.min(5000, this.retryDelay * 2);
          return;
        }
        // A server failure may follow a committed mutation. Retry this exact request.
        if (status >= 500) return;
        this.stop();
        return;
      }
      if (request.method === "GET") {
        if (
          !record(body) ||
          body["version"] !== 1 ||
          body["pane"] !== this.pane ||
          !text(body["epoch"], 256) ||
          !record(body["producer"]) ||
          !counter(body["producer"]["generation"]) ||
          (body["producer"]["id"] !== null && body["producer"]["id"] !== undefined)
        ) {
          this.stop();
          return;
        }
        // Freeze fresh compare-and-swap evidence only while the pane is unowned.
        this.request = {
          method: "POST",
          path: `${this.path}/producer`,
          body: {
            version: 1,
            epoch: body["epoch"],
            expected_generation: body["producer"]["generation"],
            producer_id: this.producer,
            evidence: { ...this.evidence },
          },
        };
      } else {
        if (
          !acknowledgement(body, this.pane, this.producer) ||
          body.sequence !== (request.method === "POST" ? 0 : this.sequence) ||
          (request.method === "POST" &&
            record(request.body) &&
            (body.epoch !== request.body["epoch"] ||
              !counter(request.body["expected_generation"]) ||
              body.generation <= request.body["expected_generation"])) ||
          (this.stamp &&
            (body.epoch !== this.stamp.epoch ||
              body.generation !== this.stamp.generation ||
              body.session !== this.stamp.session))
        ) {
          this.stop();
          return;
        }
        this.stamp = {
          epoch: body.epoch,
          generation: body.generation,
          producer_id: body.producer_id,
          session: body.session,
        };
        this.request = undefined;
        this.retryDelay = 500;
        this.retryAt = -Infinity;
      }
    } catch {
      // Offline and ambiguous outcomes are telemetry gaps, never hook failures.
      if (this.disposed) return;
    } finally {
      if (!this.disposed) {
        this.busy = false;
        this.controller = undefined;
        if (this.request || this.pending) this.schedule();
      }
    }
  }

  private stop(): void {
    this.stopped = true;
    clearInterval(this.heartbeat);
    if (this.timer) clearTimeout(this.timer);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    clearInterval(this.heartbeat);
    if (this.timer) clearTimeout(this.timer);
    this.controller?.abort();
    // A close can only terminalize this acknowledged registration. Never wait in shutdown.
    if (this.stamp) {
      const request = {
        method: "PUT",
        path: this.path,
        body: {
          version: 1,
          ...this.stamp,
          sequence: this.sequence + 1,
          payload: { kind: "closed" },
        },
      };
      const close = async () => {
        await this.flight;
        // Disposed is required here: closure is the only permitted post-disposal I/O.
        if (!this.disposed) return;
        await this.send(request, new AbortController());
      };
      void close().catch(() => undefined);
    }
  }
}

/** Register synchronous notification hooks before any session resources start. */
export function registerActivity(pi: ExtensionAPI, client: Client, pane: string | undefined): void {
  let runtime: ActivityPublisher | undefined;
  pi.on("session_start", (_event, ctx) => {
    runtime?.dispose();
    runtime = undefined;
    if (!pane) return;
    try {
      const session_file = ctx.sessionManager.getSessionFile();
      const session_id = ctx.sessionManager.getSessionId();
      if (!session_file || !text(session_file, 4096) || !text(session_id, 1024) || !text(pane, 128))
        return;
      runtime = new ActivityPublisher(client, pane, { session_file, session_id }, ctx.isIdle());
    } catch {
      /* Missing native evidence leaves detail unavailable. */
    }
  });
  pi.on("session_shutdown", () => {
    runtime?.dispose();
    runtime = undefined;
  });
  pi.on("agent_start", () => {
    runtime?.event({ type: "agent_start" });
  });
  pi.on("agent_end", () => {
    runtime?.event({ type: "agent_end" });
  });
  pi.on("agent_settled", () => {
    runtime?.event({ type: "settled" });
  });
  pi.on("message_update", (event) => {
    if (event.message.role !== "assistant") return;
    const update = event.assistantMessageEvent;
    switch (update.type) {
      case "thinking_start":
      case "thinking_delta":
      case "thinking_end":
      case "text_start":
      case "text_delta":
      case "text_end":
        runtime?.event({
          type: "block",
          index: update.contentIndex,
          phase: update.type.startsWith("thinking") ? "thinking" : "responding",
          ended: update.type.endsWith("_end"),
        });
        break;
    }
  });
  pi.on("message_end", (event) => {
    if (event.message.role === "assistant") runtime?.event({ type: "message_end" });
  });
  pi.on("tool_execution_start", (event) => {
    runtime?.event({
      type: "tool_start",
      tool: {
        call_id: event.toolCallId,
        name: event.toolName,
        parent_call_id: event.parentToolCallId ?? null,
      },
    });
  });
  pi.on("tool_execution_end", (event) => {
    runtime?.event({ type: "tool_end", id: event.toolCallId });
  });
}
