import assert from "node:assert/strict";
import { test, type TestContext } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
  ActivityPublisher,
  ActivityReducer,
  registerActivity,
  type ActiveTool,
  type ActivityEvent,
  type Payload,
} from "./activity.ts";

const tool = (id: string, parent: string | null = null): ActiveTool => ({
  call_id: id,
  name: "read",
  parent_call_id: parent,
});
const phase = (payload: Payload): string =>
  payload.kind === "snapshot" ? payload.state.phase : payload.kind;
const block = (index: number, phase: "thinking" | "responding", ended = false): ActivityEvent => ({
  type: "block",
  index,
  phase,
  ended,
});

test("agent start is working; active blocks use observation order, not content index", () => {
  const r = new ActivityReducer(true);
  assert.equal(phase(r.snapshot()), "idle");
  assert.equal(phase(r.reduce({ type: "agent_start" })), "working");
  assert.equal(phase(r.reduce(block(10, "thinking"))), "thinking");
  assert.equal(phase(r.reduce(block(1, "responding"))), "responding");
  assert.equal(phase(r.reduce(block(10, "thinking"))), "thinking");
  assert.equal(phase(r.reduce(block(10, "thinking", true))), "responding");
  assert.equal(phase(r.reduce({ type: "message_end" })), "working");
});

test("parallel and nested executing tools precede streams, and finish independently", () => {
  const r = new ActivityReducer(true);
  r.reduce(block(0, "thinking"));
  r.reduce({ type: "tool_start", tool: tool("parent") });
  r.reduce({ type: "tool_start", tool: tool("parallel") });
  const current = r.reduce({ type: "tool_start", tool: tool("nested", "parent") });
  assert.deepEqual(current, {
    kind: "snapshot",
    state: {
      phase: "tool_running",
      active_tools: [tool("parent"), tool("parallel"), tool("nested", "parent")],
    },
  });
  r.reduce({ type: "tool_end", id: "parent" });
  r.reduce({ type: "tool_end", id: "nested" });
  assert.equal(phase(r.reduce({ type: "message_end" })), "tool_running");
  assert.equal(phase(r.reduce({ type: "tool_end", id: "parallel" })), "working");
});

test("agent_end is not settlement; interrupted settlement clears tools and blocks to idle", () => {
  const r = new ActivityReducer(true);
  r.reduce({ type: "tool_start", tool: tool("interrupted") });
  r.reduce(block(1, "thinking"));
  assert.equal(phase(r.reduce({ type: "agent_end" })), "tool_running");
  assert.deepEqual(r.reduce({ type: "settled" }), {
    kind: "snapshot",
    state: { phase: "idle", active_tools: [] },
  });
  assert.equal(phase(r.reduce({ type: "agent_start" })), "working");
});

test("startup while busy stays resyncing until verified settlement", () => {
  const r = new ActivityReducer(false);
  for (const event of [
    { type: "agent_start" },
    block(0, "thinking"),
    { type: "message_end" },
    { type: "agent_end" },
  ] satisfies ActivityEvent[]) {
    assert.deepEqual(r.reduce(event), { kind: "unavailable", reason: "resyncing" });
  }
  assert.equal(phase(r.reduce({ type: "settled" })), "idle");
});

test("both collections cap at 64; overflow remains sticky through apparent emptiness", () => {
  for (const collection of ["blocks", "tools"]) {
    const r = new ActivityReducer(true);
    for (let i = 0; i < 64; i++)
      r.reduce(
        collection === "blocks"
          ? block(i, "thinking")
          : { type: "tool_start", tool: tool(String(i)) },
      );
    assert.notEqual(phase(r.snapshot()), "unavailable");
    const overflow = r.reduce(
      collection === "blocks" ? block(64, "responding") : { type: "tool_start", tool: tool("64") },
    );
    assert.deepEqual(overflow, { kind: "unavailable", reason: "overflow" });
    for (let i = 0; i < 65; i++)
      r.reduce(
        collection === "blocks" ? block(i, "thinking", true) : { type: "tool_end", id: String(i) },
      );
    assert.deepEqual(r.reduce({ type: "agent_end" }), overflow);
    assert.equal(phase(r.reduce({ type: "settled" })), "idle");
  }
});

interface Call {
  method: string;
  path: string;
  body: any;
  signal: AbortSignal | undefined;
}
const evidence = { session_file: "/sessions/native.jsonl", session_id: "native" };
const view = { version: 1, pane: "pane", epoch: "serve", producer: { generation: 2 } };
const response = (body: unknown, status = 200) => Response.json(body, { status });
function ack(call: Call) {
  return {
    version: 1,
    pane: "pane",
    epoch: "serve",
    generation: 3,
    producer_id: call.body.producer_id,
    session: "native-key",
    sequence: call.method === "POST" ? 0 : call.body.sequence,
    heartbeat_ms: 5000,
    ttl_ms: 15000,
  };
}

function rig(t: TestContext, respond?: (call: Call) => Promise<Response> | Response) {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  let now = 0;
  t.mock.method(performance, "now", () => now);
  const calls: Call[] = [];
  const client = {
    async request(
      method: string,
      path: string,
      options: { body?: unknown; signal?: AbortSignal | undefined } = {},
    ) {
      const call = { method, path, body: options.body, signal: options.signal };
      calls.push(call);
      return respond ? respond(call) : response(method === "GET" ? view : ack(call));
    },
  };
  const publisher = new ActivityPublisher(client, "pane", evidence, true);
  t.after(() => publisher.dispose());
  const step = async (milliseconds = 250) => {
    now += milliseconds;
    t.mock.timers.tick(milliseconds);
    await new Promise<void>((resolve) => setImmediate(resolve));
  };
  const ready = async () => {
    await step(0);
    await step();
    await step();
  };
  return { client, publisher, calls, step, ready };
}

test("registration freezes evidence and publishes only compact facts, coalesced at 250ms", async (t) => {
  const r = rig(t);
  await r.ready();
  assert.deepEqual(
    r.calls.map((c) => c.method),
    ["GET", "POST", "PUT"],
  );
  assert.deepEqual(r.calls[1]?.body.evidence, evidence);
  assert.equal(r.calls[1]?.body.expected_generation, 2);
  assert.equal(r.calls[1]?.body.epoch, "serve");
  assert.equal(r.calls[2]?.body.session, "native-key");
  r.publisher.event(block(1, "thinking"));
  r.publisher.event(block(2, "responding"));
  r.publisher.event({ type: "tool_start", tool: tool("read") });
  await r.step(249);
  assert.equal(r.calls.length, 3);
  await r.step(1);
  assert.deepEqual(r.calls[3]?.body.payload, {
    kind: "snapshot",
    state: { phase: "tool_running", active_tools: [tool("read")] },
  });
  assert.deepEqual(Object.keys(r.calls[3]?.body.payload.state).sort(), ["active_tools", "phase"]);
});

test("ambiguous publication retries the exact request before a replaceable pending snapshot", async (t) => {
  let attempts = 0;
  const r = rig(t, (call) => {
    if (call.method === "PUT" && ++attempts === 1) throw new Error("connection lost after commit");
    return response(call.method === "GET" ? view : ack(call));
  });
  await r.ready();
  const ambiguous = r.calls[2]?.body;
  r.publisher.event(block(0, "thinking"));
  r.publisher.event(block(1, "responding"));
  await r.step();
  assert.deepEqual(r.calls[3]?.body, ambiguous);
  await r.step();
  assert.equal(r.calls[4]?.body.sequence, 2);
  assert.equal(r.calls[4]?.body.payload.state.phase, "responding");
});

test("ambiguous registration keeps its CAS request; a conflict stops ownership attempts", async (t) => {
  let posts = 0;
  const r = rig(t, (call) => {
    if (call.method === "POST" && ++posts === 1) throw new Error("registration ack lost");
    if (call.method === "POST")
      return response({ code: "stale_generation", error: "replaced" }, 409);
    return response(view);
  });
  await r.ready();
  assert.deepEqual(r.calls[1]?.body, r.calls[2]?.body);
  r.publisher.event({ type: "settled" });
  await r.step(10000);
  assert.equal(r.calls.length, 3);
});

test("one-second deadline aborts one flight; offline telemetry never blocks reduction", async (t) => {
  const r = rig(t, () => new Promise<Response>(() => {}));
  await r.step(0);
  for (let index = 0; index < 100; index++) r.publisher.event(block(index % 2, "thinking"));
  assert.equal(r.calls.length, 1);
  await r.step(999);
  assert.equal(r.calls.length, 1);
  await r.step(1);
  assert.equal(r.calls[0]?.signal?.aborted, true);
  await r.step(0);
  assert.equal(r.calls.length, 2);
});

test("heartbeat increases sequence and sends the current state every five seconds", async (t) => {
  const r = rig(t);
  await r.ready();
  await r.step(4500);
  await r.step(0);
  assert.equal(r.calls.at(-1)?.body.sequence, 2);
  assert.equal(r.calls.at(-1)?.body.payload.state.phase, "idle");
});

test("reload rejects late registration callbacks; only the replacement runtime publishes", async (t) => {
  let resolve!: (response: Response) => void;
  const r = rig(t, (call) =>
    call.method === "POST"
      ? new Promise<Response>((done) => {
          resolve = done;
        })
      : response(call.method === "GET" ? view : ack(call)),
  );
  await r.step(0);
  await r.step();
  const old = r.calls[1];
  assert.ok(old);
  r.publisher.dispose();
  const replacement = new ActivityPublisher(
    {
      request: async (method, path, options = {}) => {
        const call = { method, path, body: options.body, signal: options.signal };
        r.calls.push(call);
        return response(method === "GET" ? view : ack(call));
      },
    },
    "pane",
    evidence,
    false,
  );
  t.after(() => replacement.dispose());
  resolve(response(ack(old)));
  await r.step(0);
  await r.step();
  await r.step();
  assert.equal(r.calls.filter((call) => call.method === "PUT").length, 1);
  assert.deepEqual(r.calls.at(-1)?.body.payload, { kind: "unavailable", reason: "resyncing" });
  assert.notEqual(r.calls.at(-1)?.body.producer_id, old.body.producer_id);
});

test("shutdown aborts a pending publication, then attempts only its own terminal closure", async (t) => {
  let resolve!: (response: Response) => void;
  const r = rig(t, (call) =>
    call.method === "PUT" && call.body.sequence === 2
      ? new Promise<Response>((done) => {
          resolve = done;
        })
      : response(call.method === "GET" ? view : ack(call)),
  );
  await r.ready();
  r.publisher.event(block(0, "thinking"));
  await r.step();
  const pending = r.calls.at(-1);
  assert.ok(pending);
  r.publisher.dispose();
  assert.equal(pending.signal?.aborted, true);
  assert.equal(r.calls.length, 4);
  resolve(response(ack(pending)));
  await r.step(0);
  assert.deepEqual(r.calls.at(-1)?.body.payload, { kind: "closed" });
  assert.equal(r.calls.at(-1)?.body.sequence, 3);
  assert.equal(r.calls.at(-1)?.body.producer_id, pending.body.producer_id);
  r.publisher.event({ type: "settled" });
  await r.step(10000);
  assert.equal(r.calls.length, 5);
});

test("notification hooks are synchronous offline and preparation never claims execution", async (t) => {
  const r = rig(t, async () => {
    throw new Error("offline");
  });
  r.publisher.dispose();
  const handlers = new Map<string, (event: any, ctx: any) => unknown>();
  registerActivity(
    {
      on: (type: string, fn: (event: any, ctx: any) => unknown) => {
        handlers.set(type, fn);
      },
    } as unknown as ExtensionAPI,
    r.client,
    "pane",
  );
  const invoke = (type: string, event = {}) => {
    const fn = handlers.get(type);
    assert.ok(fn);
    assert.equal(
      fn(event, {
        sessionManager: {
          getSessionFile: () => evidence.session_file,
          getSessionId: () => evidence.session_id,
        },
        isIdle: () => true,
      }),
      undefined,
    );
  };
  invoke("session_start");
  await r.step(0);
  invoke("agent_start");
  invoke("message_update", {
    message: { role: "assistant" },
    assistantMessageEvent: { type: "toolcall_delta", contentIndex: 0, delta: "private arguments" },
  });
  invoke("agent_end");
  invoke("agent_settled");
  invoke("session_shutdown");
  const count = r.calls.length;
  await r.step(10000);
  assert.equal(r.calls.length, count);
});
