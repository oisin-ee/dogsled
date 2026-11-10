import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
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
const block = (
  index: number,
  phase: "thinking" | "responding",
  ended = false,
): Extract<ActivityEvent, { type: "block" }> => ({
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
  assert.equal(
    reasoning(r.reduce({ ...block(0, "thinking"), delta: " thought" })),
    "first thought\n\nsecond",
  );
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
    epoch: call.body.epoch ?? "serve",
    generation: call.method === "POST" ? call.body.expected_generation + 1 : call.body.generation,
    producer_id: call.body.producer_id,
    session: "native-key",
    sequence: call.method === "POST" ? 0 : call.body.sequence,
    heartbeat_ms: 5000,
    ttl_ms: 15000,
  };
}

function rig(
  t: TestContext,
  respond?: (call: Call) => Promise<Response> | Response,
  fileExists = true,
) {
  t.mock.timers.enable({ apis: ["setTimeout", "setInterval"] });
  const consoleError = t.mock.method(console, "error", () => {});
  const stderrWrite = t.mock.method(process.stderr, "write", () => true);
  const reports: string[] = [];
  const exists = t.mock.fn((path: string) => {
    assert.equal(path, evidence.session_file);
    return fileExists;
  });
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
  const publisher = new ActivityPublisher(client, "pane", evidence, true, (message) => {
    reports.push(message);
  }, exists);
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
  t.after(async () => {
    publisher.dispose();
    await step(0);
    assert.equal(consoleError.mock.callCount(), 0);
    assert.equal(stderrWrite.mock.callCount(), 0);
  });
  return {
    client, publisher, calls, reports, exists, step, ready,
    fileExists: (value: boolean) => { fileExists = value; },
  };
}

test("missing evidence waits without polling or requests until a boundary arms registration", async (t) => {
  const r = rig(t, undefined, false);
  r.publisher.event({ type: "agent_start" });
  r.publisher.event({ ...block(0, "thinking"), delta: "before registration" });
  await r.step(30000);
  assert.equal(r.calls.length, 0);
  assert.equal(r.exists.mock.callCount(), 1);
  r.publisher.evidenceReady();
  assert.equal(r.calls.length, 0);
  r.fileExists(true);
  await r.step(5000);
  assert.equal(r.calls.length, 0);
  r.publisher.evidenceReady();
  await r.ready();
  assert.deepEqual(r.calls.map((call) => call.method), ["POST", "PUT"]);
  assert.equal(r.calls[1]?.body.payload.state.phase, "thinking");
  assert.equal(r.calls[1]?.body.payload.state.reasoning, "before registration");
  const checks = r.exists.mock.callCount();
  r.publisher.evidenceReady();
  await r.step(0);
  assert.equal(r.exists.mock.callCount(), checks);
  assert.equal(r.calls.length, 2);
  assert.deepEqual(r.reports, []);
});

test("existing evidence registers at construction, freezes evidence, and accepts an assigned epoch", async (t) => {
  const r = rig(t);
  evidence.session_id = "changed-after-start";
  await r.ready();
  evidence.session_id = "native";
  assert.deepEqual(
    r.calls.map((c) => c.method),
    ["POST", "PUT"],
  );
  assert.equal(r.calls[0]?.path, "/api/agents/local/pane/activity/producer");
  assert.deepEqual(r.calls[0]?.body.evidence, evidence);
  assert.equal(r.calls[0]?.body.expected_generation, 0);
  assert.equal(r.calls[0]?.body.epoch, null);
  assert.equal(r.calls[1]?.body.epoch, "serve");
  assert.equal(r.calls[1]?.body.session, "native-key");
  r.publisher.event(block(1, "thinking"));
  r.publisher.event(block(2, "responding"));
  r.publisher.event({ type: "tool_start", tool: tool("read") });
  await r.step(0);
  assert.deepEqual(r.calls[2]?.body.payload, {
    kind: "snapshot",
    state: { phase: "tool_running", active_tools: [tool("read")] },
  });
  r.publisher.event({ type: "settled" });
  await r.step(249);
  assert.equal(r.calls.length, 3);
  await r.step(1);
  assert.equal(r.calls[3]?.body.payload.state.phase, "idle");
});

for (const method of ["GET", "POST"]) {
  for (const [code, status] of [
    ["agent_missing", 404],
    ["session_unverified", 503],
    ["upstream_unavailable", 503],
  ] as const) {
    test(method + " startup " + code + " recovers through registration", async (t) => {
      let failed = false;
      let posts = 0;
      const r = rig(t, (call) => {
        if (call.method === "POST" && ++posts === 1 && method === "GET")
          return response({ code: "stale_generation", error: "discover" }, 409);
        if (call.method === method && !failed) {
          failed = true;
          return response({ code, error: "not ready" }, status);
        }
        return response(call.method === "GET" ? view : ack(call));
      });
      r.publisher.evidenceReady();
      await r.step(0);
      const original = r.calls[0]?.body;
      await r.step(500);
      if (method === "GET") await r.step(1000);
      await r.step(250);
      assert.equal(r.calls.at(-1)?.method, "PUT");
      const registrations = r.calls.filter((call) => call.method === "POST");
      assert.equal(registrations.length, 2);
      assert.deepEqual(registrations[1]?.body, original);
      assert.deepEqual(r.reports, []);
    });
  }
}

for (const code of ["stale_generation", "session_changed"]) {
  test("definitive POST " + code + " discovers CAS evidence", async (t) => {
    let posts = 0;
    const r = rig(t, (call) => {
      if (call.method === "POST" && ++posts === 1)
        return response({ code, error: "discover" }, 409);
      return response(call.method === "GET" ? view : ack(call));
    });
    await r.step(0);
    await r.step(500);
    await r.step(250);
    await r.step(250);
    assert.deepEqual(
      r.calls.map((call) => call.method),
      ["POST", "GET", "POST", "PUT"],
    );
    assert.equal(r.calls[2]?.body.expected_generation, 2);
    assert.equal(r.calls[2]?.body.epoch, "serve");
  });
}

for (const retired of ["producer_closed", "expired"]) {
  test("retained " + retired + " producer permits CAS registration", async (t) => {
    let posts = 0;
    const r = rig(t, (call) => {
      if (call.method === "POST" && ++posts === 1)
        return response({ code: "stale_generation", error: "discover" }, 409);
      return response(
        call.method === "GET"
          ? {
              ...view,
              producer: { generation: 3, id: "other" },
              detail: { availability: "unavailable", reason: retired },
            }
          : ack(call),
      );
    });
    await r.step(0);
    await r.step(500);
    await r.step(250);
    await r.step(250);
    assert.equal(r.calls[2]?.body.expected_generation, 3);
    assert.equal(r.calls.at(-1)?.method, "PUT");
  });
}

for (const retirement of ["expired", "producer_closed"]) {
  for (const detail of [
    { availability: "available" },
    ...["native_unavailable", "no_snapshot", "resyncing", "overflow"].map((reason) => ({
      availability: "unavailable",
      reason,
    })),
  ]) {
    test(
      "live foreign owner waits for " +
        retirement +
        " and probes without claiming: " +
        JSON.stringify(detail),
      async (t) => {
        let retired = false;
        const r = rig(t, (call) => {
          if (call.method === "GET")
            return response({
              ...view,
              producer: { generation: 3, id: "other" },
              detail: retired ? { availability: "unavailable", reason: retirement } : detail,
            });
          if (call.method === "POST" && call.body.expected_generation === 0)
            return response({ code: "stale_generation", error: "owned" }, 409);
          return response(ack(call));
        });
        await r.step(0);
        await r.step(500);
        r.publisher.event({ type: "settled" });
        await r.step(999);
        assert.equal(r.calls.length, 2);
        await r.step(1);
        assert.deepEqual(r.calls[2]?.body, r.calls[0]?.body);
        assert.equal(r.calls[2]?.body.expected_generation, 0);
        retired = true;
        await r.step(2000);
        assert.equal(r.calls.at(-1)?.method, "GET");
        await r.step(250);
        assert.equal(r.calls.at(-1)?.body.expected_generation, 3);
        await r.step(250);
        assert.equal(r.calls.at(-1)?.method, "PUT");
        assert.deepEqual(
          r.calls.map((call) => call.method),
          ["POST", "GET", "POST", "GET", "POST", "PUT"],
        );
      },
    );
  }
}

for (const method of ["POST", "PUT"]) {
  for (const failure of ["transport", "abort", "parsed503", "unparseable503"]) {
    test(
      method + " " + failure + " replays its identical request before pending data",
      async (t) => {
        let attempts = 0;
        const r = rig(t, (call) => {
          if (call.method === method && ++attempts === 1) {
            if (failure === "transport") throw new Error("connection lost after commit");
            if (failure === "abort") throw new DOMException("aborted", "AbortError");
            if (failure === "parsed503")
              return response({ code: "upstream_unavailable", error: "queued" }, 503);
            return new Response("unavailable", { status: 503 });
          }
          return response(call.method === "GET" ? view : ack(call));
        });
        await r.ready();
        r.publisher.event(block(0, "responding"));
        await r.step(500);
        const retries = r.calls.filter((call) => call.method === method);
        assert.equal(retries.length, 2);
        assert.strictEqual(retries[0]?.body, retries[1]?.body);
        assert.equal(retries[0]?.path, retries[1]?.path);
        await r.step(250);
        assert.equal(r.calls.at(-1)?.body.payload.state.phase, "responding");
      },
    );
  }

  test(method + " delayed ACK beyond one second still publishes", async (t) => {
    let resolve!: (value: Response) => void;
    let delayed: Call | undefined;
    const r = rig(t, (call) => {
      if (call.method === method && !delayed) {
        delayed = call;
        return new Promise<Response>((done) => {
          resolve = done;
        });
      }
      return response(ack(call));
    });
    await r.ready();
    await r.step(2400);
    assert.ok(delayed);
    assert.equal(delayed.signal?.aborted, false);
    resolve(response(ack(delayed)));
    await r.step(0);
    await r.step(250);
    assert.equal(r.calls.filter((call) => call.method === method).length, 1);
    assert.equal(r.calls.at(-1)?.method, "PUT");
  });

  test(method + " late ACK after deadline cannot replace an identical retry", async (t) => {
    let resolve!: (value: Response) => void;
    let delayed: Call | undefined;
    const r = rig(t, (call) => {
      if (call.method === method && !delayed) {
        delayed = call;
        return new Promise<Response>((done) => {
          resolve = done;
        });
      }
      return response(ack(call));
    });
    await r.ready();
    await r.step(6000);
    assert.ok(delayed);
    assert.equal(delayed.signal?.aborted, true);
    await r.step(500);
    const retries = r.calls.filter((call) => call.method === method);
    assert.equal(retries.length, 2);
    assert.strictEqual(retries[0]?.body, retries[1]?.body);
    resolve(response(ack(delayed)));
    await r.step(0);
    r.publisher.event(block(0, "thinking"));
    await r.step(250);
    assert.equal(r.calls.at(-1)?.method, "PUT");
    assert.equal(r.calls.at(-1)?.body.producer_id, delayed.body.producer_id);
    assert.equal(r.calls.at(-1)?.body.payload.state.phase, "thinking");
  });

  for (const code of ["stale_generation", "session_changed", "producer_closed"]) {
    if (method !== "PUT") continue;
    test("PUT " + code + " refreshes registration and latest snapshot", async (t) => {
      let puts = 0;
      const r = rig(t, (call) => {
        if (call.method === "PUT" && ++puts === 1) return response({ code, error: "revoked" }, 409);
        return response(call.method === "GET" ? view : ack(call));
      });
      await r.ready();
      r.publisher.event(block(0, "responding"));
      await r.step(250);
      await r.step(250);
      await r.step(250);
      assert.deepEqual(
        r.calls.map((call) => call.method),
        ["POST", "PUT", "GET", "POST", "PUT"],
      );
      assert.equal(r.calls.at(-1)?.body.sequence, 1);
      assert.equal(r.calls.at(-1)?.body.payload.state.phase, "responding");
    });
  }

  test("dispose during " + method + " recovery cancels retries", async (t) => {
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

test("six-second deadline aborts and replays identical registration with backoff", async (t) => {
  const r = rig(t, () => new Promise<Response>(() => {}));
  await r.step(0);
  for (let index = 0; index < 100; index++) r.publisher.event(block(index % 2, "thinking"));
  await r.step(5999);
  assert.equal(r.calls.length, 1);
  assert.equal(r.calls[0]?.signal?.aborted, false);
  await r.step(1);
  assert.equal(r.calls[0]?.signal?.aborted, true);
  await r.step(499);
  assert.equal(r.calls.length, 1);
  await r.step(1);
  assert.equal(r.calls.length, 2);
  assert.strictEqual(r.calls[0]?.body, r.calls[1]?.body);
  r.publisher.dispose();
  assert.equal(r.calls[1]?.signal?.aborted, true);
  await r.step(20000);
  assert.equal(r.calls.length, 2);
});

test("identical and changed recovery codes stay silent", async (t) => {
  let code = "upstream_unavailable";
  let status = 503;
  const r = rig(t, () => response({ code, error: "not ready" }, status));
  await r.step(0);
  await r.step(500);
  status = 502;
  await r.step(1000);
  assert.equal(r.calls.length, 3);
  assert.deepEqual(r.reports, []);
  code = "session_unverified";
  status = 503;
  await r.step(2000);
  assert.equal(r.calls.length, 4);
  r.publisher.dispose();
  assert.deepEqual(r.reports, []);
});

test("successful ACK resets backoff delay without reporting recovery", async (t) => {
  let fail = true;
  const r = rig(t, (call) => {
    if (call.method === "PUT" && fail)
      return response({ code: "upstream_unavailable", error: "not ready" }, 503);
    return response(ack(call));
  });
  await r.ready();
  assert.equal(r.calls.length, 2);
  await r.step(250);
  assert.equal(r.calls.length, 3);
  fail = false;
  await r.step(1000);
  assert.equal(r.calls.length, 4);
  fail = true;
  r.publisher.event({ type: "settled" });
  await r.step(250);
  assert.equal(r.calls.length, 5);
  await r.step(499);
  assert.equal(r.calls.length, 5);
  await r.step(1);
  assert.equal(r.calls.length, 6);
  assert.deepEqual(r.reports, []);
});

test("registration backoff doubles to five seconds despite hooks and heartbeats", async (t) => {
  const r = rig(t, () => response({ code: "upstream_unavailable", error: "offline" }, 503));
  await r.step(0);
  for (const delay of [500, 1000, 2000, 4000, 5000, 5000]) {
    const count = r.calls.length;
    r.publisher.event({ type: "settled" });
    await r.step(delay - 1);
    assert.equal(r.calls.length, count);
    await r.step(1);
    assert.equal(r.calls.length, count + 1);
    assert.strictEqual(r.calls.at(-1)?.body, r.calls[0]?.body);
  }
});

for (const method of ["GET", "POST", "PUT"]) {
  test(method + " invalid acknowledgement reports once and stops", async (t) => {
    const r = rig(t, (call) => {
      if (call.method === method) return response({ version: 999 });
      if (method === "GET" && call.method === "POST")
        return response({ code: "stale_generation", error: "discover" }, 409);
      return response(ack(call));
    });
    await r.ready();
    const count = r.calls.length;
    r.publisher.event({ type: "settled" });
    r.publisher.evidenceReady();
    await r.step(30000);
    assert.equal(r.calls.length, count);
    assert.deepEqual(r.reports, ["Dogsled live activity stopped: invalid_response (HTTP 200)"]);
    r.publisher.dispose();
    assert.equal(r.reports.length, 1);
  });

  for (const [code, status] of [
    ["invalid_request", 400],
    ["unsupported_version", 400],
    ["unauthorized", 403],
    ["sequence_conflict", 409],
    ["unknown", 404],
  ] as const) {
    test(method + " terminal " + code + " stops", async (t) => {
      const r = rig(t, (call) => {
        if (call.method === method) return response({ code, error: "terminal" }, status);
        if (method === "GET" && call.method === "POST")
          return response({ code: "stale_generation", error: "discover" }, 409);
        return response(ack(call));
      });
      await r.ready();
      const count = r.calls.length;
      r.publisher.event({ type: "settled" });
      await r.step(20000);
      assert.equal(r.calls.length, count);
      assert.deepEqual(r.reports, [`Dogsled live activity stopped: ${code} (HTTP ${status})`]);
      r.publisher.evidenceReady();
      r.publisher.dispose();
      assert.equal(r.reports.length, 1);
    });
  }
}

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
  const old = r.calls[0];
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
    (message) => r.reports.push(message),
    () => true,
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
  assert.equal(r.calls.length, 3);
  resolve(response(ack(pending)));
  await r.step(0);
  assert.deepEqual(r.calls.at(-1)?.body.payload, { kind: "closed" });
  assert.equal(r.calls.at(-1)?.body.sequence, 3);
  assert.equal(r.calls.at(-1)?.body.producer_id, pending.body.producer_id);
  r.publisher.event({ type: "settled" });
  await r.step(10000);
  assert.equal(r.calls.length, 4);
});

function sessionHooks(
  t: TestContext,
  r: ReturnType<typeof rig>,
  notify: (message: string, level: string) => void = () => {},
) {
  r.publisher.dispose();
  const directory = mkdtempSync(join(tmpdir(), "dogsled-activity-"));
  const sessionFile = join(directory, "session.jsonl");
  const handlers = new Map<string, (event: any, ctx: any) => unknown>();
  const ctx = {
    sessionManager: {
      getSessionFile: () => sessionFile,
      getSessionId: () => "native",
    },
    isIdle: () => true,
    ui: { notify },
  };
  registerActivity({
    on: (type: string, fn: (event: any, ctx: any) => unknown) => {
      handlers.set(type, fn);
    },
  } as unknown as ExtensionAPI, r.client, "pane");
  const invoke = (type: string, event = {}, context = ctx) => {
    const handler = handlers.get(type);
    assert.ok(handler, type);
    assert.equal(handler(event, context), undefined);
  };
  t.after(() => {
    invoke("session_shutdown");
    rmSync(directory, { recursive: true, force: true });
  });
  return {
    ctx, invoke,
    persist: () => writeFileSync(sessionFile, '{"type":"session","id":"native"}\n'),
  };
}

for (const [boundary, role] of [
  ["message_start", "user"],
  ["message_start", "assistant"],
  ["message_start", "toolResult"],
  ["message_end", "user"],
  ["message_end", "assistant"],
  ["message_end", "toolResult"],
  ["turn_end", "assistant"],
  ["agent_settled", "assistant"],
]) {
  test(boundary + " " + role + " arms evidence but streaming deltas do not", async (t) => {
    const r = rig(t);
    const hooks = sessionHooks(t, r);
    hooks.invoke("session_start");
    hooks.invoke("message_end", { message: { role: "user" } });
    await r.step(30000);
    assert.equal(r.calls.length, 0);
    hooks.persist();
    hooks.invoke("agent_start");
    hooks.invoke("message_update", {
      message: { role: "assistant" },
      assistantMessageEvent: { type: "thinking_delta", contentIndex: 0, delta: "observed" },
    });
    await r.step(5000);
    assert.equal(r.calls.length, 0);
    hooks.invoke(boundary!, { message: { role } });
    await r.ready();
    assert.deepEqual(r.calls.map((call) => call.method), ["POST", "PUT"]);
    assert.equal(r.calls[1]?.body.payload.state.phase,
      boundary === "agent_settled" ? "idle" :
        boundary === "message_end" && role === "assistant" ? "working" : "thinking");
    hooks.invoke("session_shutdown");
    assert.deepEqual(r.reports, []);
  });
}

for (const throws of [false, true]) {
  test("terminal notification uses the starting context and stays safe when notify throws: " + throws, async (t) => {
    const r = rig(t, () => response({ code: "unauthorized", error: "private response" }, 403));
    const notices: [string, string][] = [];
    const hooks = sessionHooks(t, r, (message, level) => {
      notices.push([message, level]);
      if (throws) throw new Error("notification unavailable");
    });
    hooks.persist();
    hooks.invoke("session_start");
    hooks.invoke("message_start", { message: { role: "assistant" } }, {
      ...hooks.ctx,
      ui: { notify: () => assert.fail("must use the session_start context") },
    });
    await r.ready();
    hooks.invoke("agent_settled");
    await r.step(30000);
    assert.equal(r.calls.length, 1);
    hooks.invoke("session_shutdown");
    assert.deepEqual(notices, [["Dogsled live activity stopped: unauthorized (HTTP 403)", "warning"]]);
  });
}

test("notification hooks are synchronous offline and preparation never claims execution", async (t) => {
  const r = rig(t, async () => {
    throw new Error("offline");
  });
  const hooks = sessionHooks(t, r);
  const { invoke } = hooks;
  hooks.persist();
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
  assert.equal(r.calls.length, 1);
  assert.deepEqual(r.reports, []);
  const count = r.calls.length;
  await r.step(10000);
  assert.equal(r.calls.length, count);
});
