import assert from "node:assert/strict";
import { test } from "node:test";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { fauxProvider } from "@earendil-works/pi-ai";
import { registerSettings, SessionSettings } from "./settings.ts";

const evidence = { session_file: "/tmp/session.jsonl", session_id: "session" };
const first = { ...fauxProvider().getModel(), provider: "first", id: "one", reasoning: true };
const second = { ...first, provider: "second", id: "two" };
function fixture() {
  let selected = first;
  let level: ReturnType<ExtensionAPI["getThinkingLevel"]> = "high";
  let allowed = true;
  const calls: { method: string; path: string; body: unknown }[] = [];
  const setters: string[] = [];
  const handlers = new Map<string, (event: unknown, ctx: ExtensionContext) => void>();
  const pi = {
    on(name: string, handler: (event: unknown, ctx: ExtensionContext) => void) {
      handlers.set(name, handler);
    },
    async setModel(model: typeof first) {
      setters.push(`model:${model.id}`);
      if (allowed) selected = model;
      return allowed;
    },
    setThinkingLevel(value: typeof level) {
      setters.push(`thinking:${value}`);
      level = value;
    },
    getThinkingLevel: () => level,
  };
  const ctx = {
    get model() {
      return selected;
    },
    scopedModels: [],
    modelRegistry: {
      getAvailable: () => [first, second],
      find: (provider: string, id: string) =>
        [first, second].find((model) => model.provider === provider && model.id === id),
    },
    sessionManager: {
      getSessionFile: () => evidence.session_file,
      getSessionId: () => evidence.session_id,
    },
  } as unknown as ExtensionContext;
  const client = {
    async json<T>(method: string, path: string, body?: unknown): Promise<T> {
      calls.push({ method, path, body });
      return undefined as T;
    },
    async events(_path: string, signal: AbortSignal) {
      async function* stream() {
        await new Promise<void>((resolve) => {
          if (signal.aborted) resolve();
          else signal.addEventListener("abort", () => resolve(), { once: true });
        });
      }
      return stream();
    },
  };
  return {
    pi,
    ctx,
    client,
    calls,
    setters,
    handlers,
    refuse: () => {
      allowed = false;
    },
  };
}
const change = (model = second) => ({
  id: "request",
  evidence,
  change: { model: { provider: model.provider, id: model.id }, thinking: "low" },
});

test("settings publish on session, model, and thinking events and stop at shutdown", async () => {
  const f = fixture();
  registerSettings(f.pi as unknown as ExtensionAPI, f.client, "w1:p1");
  f.handlers.get("session_start")!({}, f.ctx);
  await new Promise((resolve) => setImmediate(resolve));
  f.handlers.get("model_select")!({}, f.ctx);
  f.handlers.get("thinking_level_select")!({}, f.ctx);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(f.calls.length, 3);
  for (const call of f.calls) {
    assert.equal(call.method, "PUT");
    assert.equal(call.path, "/api/agents/w1%3Ap1/settings");
    const body = call.body as {
      evidence: typeof evidence;
      settings: { model: unknown; thinking: string; models: unknown[] };
    };
    assert.deepEqual(body.evidence, evidence);
    assert.equal(body.settings.thinking, "high");
    assert.equal(body.settings.models.length, 2);
  }
  f.handlers.get("session_shutdown")!({}, f.ctx);
  f.handlers.get("model_select")!({}, f.ctx);
  assert.equal(f.calls.length, 3);
});

test("live settings apply model before thinking and return the actual settings", async () => {
  const f = fixture();
  const session = new SessionSettings(f.pi, f.client, "w1:p1", evidence, f.ctx);
  await session.apply(change());
  assert.deepEqual(f.setters, [`model:${second.id}`, "thinking:low"]);
  const body = f.calls[0]!.body as { settings: { model: unknown; thinking: string } };
  assert.deepEqual(body.settings.model, { provider: second.provider, id: second.id });
  assert.equal(body.settings.thinking, "low");
  session.stop();
});

test("false setModel reports credentials refusal without changing thinking", async () => {
  const f = fixture();
  f.refuse();
  const session = new SessionSettings(f.pi, f.client, "w1:p1", evidence, f.ctx);
  await session.apply(change());
  assert.deepEqual(f.setters, [`model:${second.id}`]);
  assert.equal(
    (f.calls[0]!.body as { error: string }).error,
    `no credentials for ${second.provider}`,
  );
  session.stop();
});

test("unknown models report an error and stale or stopped sessions are ignored", async () => {
  const f = fixture();
  const session = new SessionSettings(f.pi, f.client, "w1:p1", evidence, f.ctx);
  await session.apply({ ...change(), evidence: { ...evidence, session_id: "old" } });
  assert.equal(f.calls.length, 0);
  await session.apply({ ...change(), change: { model: { provider: "unknown", id: "missing" } } });
  assert.equal((f.calls[0]!.body as { error: string }).error, "unknown model");
  assert.equal(f.setters.length, 0);
  session.stop();
  await session.apply(change());
  assert.equal(f.calls.length, 1);
});

test("scoped model choices replace the available catalogue", async () => {
  const f = fixture();
  const scoped = { ...f.ctx, scopedModels: [{ model: second, thinkingLevel: "low" as const }] };
  const session = new SessionSettings(f.pi, f.client, "w1:p1", evidence, scoped);
  await session.publish();
  const settings = (
    f.calls[0]!.body as { settings: { models: { model: unknown; thinking: string[] }[] } }
  ).settings;
  assert.equal(settings.models.length, 1);
  assert.deepEqual(settings.models[0]!.model, { provider: second.provider, id: second.id });
  assert.ok(settings.models[0]!.thinking.includes("low"));
  session.stop();
});

test("publication limits model count and UTF-8 identity bytes", async () => {
  const f = fixture();
  const scoped = {
    ...f.ctx,
    scopedModels: Array.from({ length: 1002 }, (_, index) => ({
      model: { ...first, id: index === 0 ? "界".repeat(100) : `choice-${index}` },
      thinkingLevel: "high" as const,
    })),
  };
  const session = new SessionSettings(f.pi, f.client, "w1:p1", evidence, scoped);
  await session.publish();
  const body = f.calls[0]!.body as { settings: { models: { model: { id: string } }[] } };
  assert.equal(body.settings.models.length, 1000);
  assert.equal(body.settings.models[0]!.model.id, "choice-1");
  assert.ok(body.settings.models.every((choice) => Buffer.byteLength(choice.model.id) <= 256));
  session.stop();
});
