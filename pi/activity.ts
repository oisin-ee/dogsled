// Session-scoped telemetry. Hooks reduce compact facts; only the publisher performs I/O.
import { randomUUID } from "node:crypto";
import { existsSync } from "node:fs";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { PortalClient } from "./portal-client.ts";

export type Phase = "idle" | "working" | "thinking" | "responding" | "tool_running";
export interface ActiveTool {
  call_id: string;
  name: string;
  parent_call_id: string | null;
  call?: string;
}
export type Payload =
  | { kind: "snapshot"; state: { phase: Phase; active_tools: ActiveTool[]; reasoning?: string } }
  | { kind: "unavailable"; reason: "resyncing" | "overflow" }
  | { kind: "closed" };
export type ActivityEvent =
  | { type: "agent_start" | "agent_end" | "settled" | "message_end" }
  | {
      type: "block";
      index: number;
      phase: "thinking" | "responding";
      ended: boolean;
      delta?: string;
    }
  | { type: "tool_start"; tool: ActiveTool }
  | { type: "tool_end"; id: string };

/** The same call-line rule is used by the transcript reader in src/activity.rs. */
export function callLine(name: string, arguments_: unknown): string {
  let args = arguments_;
  if (typeof args === "string") {
    try {
      args = JSON.parse(args);
    } catch {
      // Non-JSON strings remain literal arguments in the fallback display.
    }
  }
  if (record(args)) {
    const command = args["command"] ?? args["cmd"];
    if (typeof command === "string") return `$ ${command}`;
    const path = args["path"] ?? args["file_path"];
    if (typeof path === "string") return `${name.toLowerCase()} ${path}`;
  }
  return `${name} ${JSON.stringify(args) ?? "null"}`;
}

/** Newest UTF-8 bytes, without splitting a character. */
function tail(value: string, max: number): string {
  const bytes = Buffer.from(value);
  let start = Math.max(0, bytes.length - max);
  while (start < bytes.length && ((bytes.at(start) ?? 0) & 0xc0) === 0x80) start++;
  return bytes.subarray(start).toString();
}

/** Preparation events never enter this reducer. Overflow is sticky until verified settlement. */
export class ActivityReducer {
  private blocks = new Map<number, "thinking" | "responding">();
  private thoughts = new Map<number, string>();
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
      this.thoughts.clear();
      this.tools.clear();
      this.gap = undefined;
      this.idle = true;
    } else if (!this.gap) {
      if (event.type === "agent_start" || event.type === "agent_end") this.idle = false;
      else if (event.type === "message_end") {
        this.blocks.clear();
        this.thoughts.clear();
      } else if (event.type === "block") {
        this.idle = false;
        this.blocks.delete(event.index);
        if (!event.ended) this.blocks.set(event.index, event.phase);
        if (event.phase === "thinking") {
          this.thoughts.set(
            event.index,
            (this.thoughts.get(event.index) ?? "") + (event.delta ?? ""),
          );
        }
      } else if (event.type === "tool_start") {
        this.idle = false;
        this.tools.set(event.tool.call_id, { ...event.tool });
      } else if (event.type === "tool_end") this.tools.delete(event.id);
      if (this.blocks.size > 64 || this.thoughts.size > 64 || this.tools.size > 64) {
        this.gap = "overflow";
        this.blocks.clear();
        this.thoughts.clear();
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
    const reasoning = tail(
      [...this.thoughts.entries()]
        .sort(([a], [b]) => a - b)
        .map(([, thought]) => thought)
        .filter((thought) => thought.length > 0)
        .join("\n\n"),
      16 * 1024,
    );
    return {
      kind: "snapshot",
      state: { phase, active_tools, ...(reasoning ? { reasoning } : {}) },
    };
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
  private heartbeat: ReturnType<typeof setInterval> | undefined;
  private controller: AbortController | undefined;
  private flight: Promise<void> | undefined;
  private readonly client: Client;
  private readonly pane: string;
  private readonly evidence: Evidence;
  private readonly exists: (path: string) => boolean;
  private readonly report: (message: string) => void;

  constructor(
    client: Client,
    pane: string,
    evidence: Evidence,
    idle: boolean,
    report: (message: string) => void,
    exists: (path: string) => boolean = existsSync,
  ) {
    this.client = client;
    this.pane = pane;
    this.evidence = { ...evidence };
    this.exists = exists;
    this.report = report;
    this.path = `/api/agents/local/${encodeURIComponent(pane)}/activity`;
    this.reducer = new ActivityReducer(idle);
    this.pending = this.reducer.snapshot();
    this.evidenceReady();
  }

  evidenceReady(): void {
    if (
      this.disposed ||
      this.stopped ||
      this.request ||
      this.stamp ||
      !this.exists(this.evidence.session_file)
    )
      return;
    this.request = this.registration(null, 0);
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
    if (
      this.disposed ||
      this.stopped ||
      this.busy ||
      this.timer ||
      (!this.request && !this.stamp)
    )
      return;
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
          }, 6000);
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
        const receivedCode =
          record(body) && typeof body["error"] === "string" ? body["code"] : undefined;
        const code =
          [
            "invalid_request",
            "unsupported_version",
            "unauthorized",
            "sequence_conflict",
            "agent_missing",
            "session_unverified",
            "upstream_unavailable",
            "session_changed",
            "stale_generation",
            "producer_closed",
            "capacity",
            "upstream_invalid",
          ].find((known) => known === receivedCode) ?? "unknown";
        if (
          code === "invalid_request" ||
          code === "unsupported_version" ||
          code === "unauthorized" ||
          code === "sequence_conflict" ||
          (status === 404 && code !== "agent_missing")
        ) {
          this.stop(status, code);
          return;
        }
        // A 5xx may follow a committed mutation, even with a typed error body.
        // Discovery failures return to POST so startup keeps cheap sampling alive.
        if (
          status >= 500 ||
          code === "agent_missing" ||
          code === "session_unverified" ||
          code === "upstream_unavailable"
        ) {
          if (
            request.method === "GET" &&
            (code === "agent_missing" ||
              code === "session_unverified" ||
              code === "upstream_unavailable")
          )
            this.request = this.registration(null, 0);
          this.backoff();
          return;
        }
        if (
          (request.method === "POST" &&
            (code === "session_changed" || code === "stale_generation")) ||
          (request.method === "PUT" &&
            (code === "stale_generation" ||
              code === "session_changed" ||
              code === "producer_closed"))
        ) {
          this.stamp = undefined;
          this.sequence = 0;
          this.pending = this.reducer.snapshot();
          this.request = { method: "GET", path: this.path };
          this.backoff();
          return;
        }
        this.stop(status, code);
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
          (body["producer"]["id"] !== null && !text(body["producer"]["id"], 256))
        ) {
          this.stop(status);
          return;
        }
        const detail = body["detail"];
        const retired =
          record(detail) &&
          detail["availability"] === "unavailable" &&
          (detail["reason"] === "producer_closed" || detail["reason"] === "expired");
        if (body["producer"]["id"] !== null && !retired) {
          // Generation-0 probes renew sampling interest by design without claiming the live generation.
          this.request = this.registration(null, 0);
          this.backoff();
          return;
        }
        this.request = this.registration(body["epoch"], body["producer"]["generation"]);
      } else {
        if (
          !acknowledgement(body, this.pane, this.producer) ||
          body.sequence !== (request.method === "POST" ? 0 : this.sequence) ||
          (request.method === "POST" &&
            record(request.body) &&
            ((request.body["epoch"] !== null && body.epoch !== request.body["epoch"]) ||
              !counter(request.body["expected_generation"]) ||
              body.generation <= request.body["expected_generation"])) ||
          (this.stamp &&
            (body.epoch !== this.stamp.epoch ||
              body.generation !== this.stamp.generation ||
              body.session !== this.stamp.session))
        ) {
          this.stop(status);
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
      this.backoff();
    } finally {
      if (!this.disposed) {
        this.busy = false;
        this.controller = undefined;
        if (this.request || this.pending) this.schedule();
      }
    }
  }

  private registration(epoch: string | null, generation: number): Request {
    return {
      method: "POST",
      path: `${this.path}/producer`,
      body: {
        version: 1,
        epoch,
        expected_generation: generation,
        producer_id: this.producer,
        evidence: { ...this.evidence },
      },
    };
  }

  private backoff(): void {
    this.retryAt = performance.now() + this.retryDelay;
    this.retryDelay = Math.min(5000, this.retryDelay * 2);
  }

  private stop(status: number, code = "invalid_response"): void {
    if (this.stopped) return;
    this.stopped = true;
    clearInterval(this.heartbeat);
    if (this.timer) clearTimeout(this.timer);
    this.report(`Dogsled live activity stopped: ${code} (HTTP ${status})`);
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
      runtime = new ActivityPublisher(
        client,
        pane,
        { session_file, session_id },
        ctx.isIdle(),
        (message) => {
          try {
            ctx.ui.notify(message, "warning");
          } catch {
            /* A notification failure must not escape telemetry. */
          }
        },
      );
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
    runtime?.evidenceReady();
  });
  pi.on("turn_end", () => {
    runtime?.evidenceReady();
  });
  pi.on("message_start", () => {
    runtime?.evidenceReady();
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
          ...(update.type === "thinking_delta" ? { delta: update.delta } : {}),
        });
        break;
    }
  });
  pi.on("message_end", (event) => {
    if (event.message.role === "assistant") runtime?.event({ type: "message_end" });
    runtime?.evidenceReady();
  });
  pi.on("tool_execution_start", (event) => {
    runtime?.event({
      type: "tool_start",
      tool: {
        call_id: event.toolCallId,
        name: event.toolName,
        parent_call_id: event.parentToolCallId ?? null,
        call: tail(callLine(event.toolName, event.args), 512),
      },
    });
  });
  pi.on("tool_execution_end", (event) => {
    runtime?.event({ type: "tool_end", id: event.toolCallId });
  });
}
