import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test, type TestContext } from "node:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
  ActivityPublisher,
  ActivityReducer,
  callLine,
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
const block = (index: number, phase: "thinking" | "responding", ended = false): Extract<ActivityEvent, { type: "block" }> => ({
  type: "block",
  index,
  phase,
  ended,
});

test("call lines match the shared transcript and live cases", () => {
  const cases: { name: string; arguments: unknown; line: string }[] = JSON.parse(
    readFileSync(join(import.meta.dirname, "../tests/fixtures/call-lines.json"), "utf8"),
  );
  for (const entry of cases) assert.equal(callLine(entry.name, entry.arguments), entry.line);
});

const reasoning = (payload: Payload): string | undefined =>
  payload.kind === "snapshot" ? payload.state.reasoning : undefined;

test("thinking deltas accumulate per index, keep a UTF-8 tail, and clear at message end or settlement", () => {
  const r = new ActivityReducer(true);
  r.reduce({ ...block(1, "thinking"), delta: "second" });
  r.reduce({ ...block(0, "thinking"), delta: "first" });
  assert.equal(reasoning(r.reduce({ ...block(0, "thinking"), delta: " thought" })), "first thought\n\nsecond");
  assert.equal(reasoning(r.reduce(block(0, "thinking", true))), "first thought\n\nsecond");
  assert.equal(reasoning(r.reduce({ type: "message_end" })), undefined);
  const thought = "界".repeat(6000) + "!";
  const current = reasoning(r.reduce({ ...block(0, "thinking"), delta: thought }));
  assert.equal(current, "界".repeat(5461) + "!");
  assert.ok(Buffer.byteLength(current!) <= 16 * 1024);
  assert.equal(reasoning(r.reduce({ type: "settled" })), undefined);
  assert.equal(reasoning(r.reduce(block(0, "thinking"))), undefined);
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
const view = { version: 1, pane: "pane", epoch: "serve", producer: { generation: 2, id: null } };
const response = (body: unknown, status = 200) => Response.json(body, { status });
function ack(call: Call) {
  return {
    version: 1,
    pane: "pane",
    epoch: call.body.epoch,
    generation: call.method === "POST" ? call.body.expected_generation + 1 : call.body.generation,
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
  assert.deepEqual(
    r.calls.map((call) => call.path),
    [
      "/api/agents/local/pane/activity",
      "/api/agents/local/pane/activity/producer",
      "/api/agents/local/pane/activity",
    ],
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

test("ambiguous registration keeps its CAS request; a replacement stops ownership attempts", async (t) => {
  let posts = 0;
  const r = rig(t, (call) => {
    if (call.method === "POST" && ++posts === 1) throw new Error("registration ack lost");
    if (call.method === "POST")
      return response({ code: "stale_generation", error: "replaced" }, 409);
    return response(posts === 0 ? view : { ...view, producer: { generation: 4, id: "other" } });
  });
  await r.ready();
  assert.deepEqual(r.calls[1]?.body, r.calls[2]?.body);
  await r.step(500);
  assert.equal(r.calls.at(-1)?.method, "GET");
  r.publisher.event({ type: "settled" });
  await r.step(10000);
  assert.equal(r.calls.length, 4);
});

test("registration refreshes binding evidence after session changes with exponential backoff", async (t) => {
  let gets = 0;
  let posts = 0;
  const r = rig(t, (call) => {
    if (call.method === "GET")
      return response({
        ...view,
        epoch: `serve-${++gets}`,
        producer: { generation: gets, id: null },
      });
    if (call.method === "POST" && ++posts <= 2)
      return response(
        { code: posts === 1 ? "session_changed" : "session_unverified", error: "not yet bound" },
        posts === 1 ? 409 : 503,
      );
    return response(ack(call));
  });
  await r.ready();
  assert.equal(r.calls.length, 2);
  r.publisher.event(block(0, "thinking"));
  await r.step(249);
  assert.equal(r.calls.length, 2);
  await r.step(1);
  assert.equal(r.calls.at(-1)?.method, "GET");
  await r.step();
  assert.equal(posts, 2);
  await r.step(999);
  assert.equal(r.calls.length, 4);
  await r.step(1);
  assert.equal(r.calls.at(-1)?.method, "GET");
  await r.step();
  await r.step();
  assert.deepEqual(
    r.calls.map((call) => call.method),
    ["GET", "POST", "GET", "POST", "GET", "POST", "PUT"],
  );
  assert.deepEqual(
    r.calls
      .filter((call) => call.method === "POST")
      .map((call) => [call.body.epoch, call.body.expected_generation]),
    [
      ["serve-1", 1],
      ["serve-2", 2],
      ["serve-3", 3],
    ],
  );
  assert.equal(r.calls.at(-1)?.body.payload.state.phase, "thinking");
});

test("registration backoff doubles to five seconds and cannot be bypassed by events or heartbeats", async (t) => {
  let posts = 0;
  const codes = ["upstream_unavailable", "stale_generation"];
  const r = rig(t, (call) => {
    if (call.method === "POST")
      return response(
        { code: codes[posts++ % codes.length], error: "not yet bound" },
        posts % 2 ? 503 : 409,
      );
    return response(view);
  });
  await r.step(0);
  await r.step();
  for (const delay of [500, 1000, 2000, 4000, 5000, 5000]) {
    const count = r.calls.length;
    r.publisher.event({ type: "settled" });
    await r.step(delay - 1);
    assert.equal(r.calls.length, count);
    await r.step(1);
    assert.equal(r.calls.at(-1)?.method, "GET");
    await r.step();
    assert.equal(r.calls.at(-1)?.method, "POST");
  }
});

test("an activity GET with an existing producer never registers", async (t) => {
  const r = rig(t, () => response({ ...view, producer: { generation: 3, id: "other" } }));
  await r.ready();
  r.publisher.event({ type: "settled" });
  await r.step(10000);
  assert.deepEqual(
    r.calls.map((call) => call.method),
    ["GET"],
  );
});

for (const code of ["stale_generation", "session_changed", "producer_closed"]) {
  test(`PUT ${code} refreshes registration and republishes the latest snapshot`, async (t) => {
    let gets = 0;
    let puts = 0;
    const r = rig(t, (call) => {
      if (call.method === "GET")
        return response({
          ...view,
          epoch: `serve-${++gets}`,
          producer: { generation: gets * 2, id: null },
        });
      if (call.method === "PUT" && ++puts === 1)
        return response({ code, error: "registration cleared" }, 409);
      return response({ ...ack(call), session: gets === 1 ? "native-key" : "new-native-key" });
    });
    await r.ready();
    r.publisher.event(block(0, "responding"));
    await r.step(499);
    assert.equal(r.calls.length, 3);
    await r.step(1);
    await r.step();
    await r.step();
    assert.deepEqual(
      r.calls.map((call) => call.method),
      ["GET", "POST", "PUT", "GET", "POST", "PUT"],
    );
    assert.equal(r.calls.at(-1)?.body.epoch, "serve-2");
    assert.equal(r.calls.at(-1)?.body.generation, 5);
    assert.equal(r.calls.at(-1)?.body.session, "new-native-key");
    assert.equal(r.calls.at(-1)?.body.sequence, 1);
    assert.equal(r.calls.at(-1)?.body.payload.state.phase, "responding");
  });
}

for (const method of ["POST", "PUT"]) {
  test(`${method} with an unparseable 5xx body retries the exact request`, async (t) => {
    let attempts = 0;
    const r = rig(t, (call) => {
      if (call.method === method && ++attempts === 1)
        return new Response("unavailable", { status: 503 });
      return response(call.method === "GET" ? view : ack(call));
    });
    await r.ready();
    await r.step();
    const retries = r.calls.filter((call) => call.method === method);
    assert.equal(retries.length, 2);
    assert.deepEqual(retries[0]?.body, retries[1]?.body);
  });

  test(`${method} stops on non-recoverable 4xx errors`, async (t) => {
    const r = rig(t, (call) =>
      call.method === method
        ? response({ code: "sequence_conflict", error: "conflict" }, 409)
        : response(call.method === "GET" ? view : ack(call)),
    );
    await r.ready();
    const count = r.calls.length;
    r.publisher.event({ type: "settled" });
    await r.step(10000);
    assert.equal(r.calls.length, count);
  });

  test(`dispose during ${method} recovery backoff sends nothing further`, async (t) => {
    const r = rig(t, (call) =>
      call.method === method
        ? response({ code: "session_changed", error: "not bound" }, 409)
        : response(call.method === "GET" ? view : ack(call)),
    );
    await r.ready();
    const count = r.calls.length;
    r.publisher.dispose();
    r.publisher.event({ type: "settled" });
    await r.step(20000);
    assert.equal(r.calls.length, count);
  });
}

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
