import {
  ExtensionRunner,
  ModelRegistry,
  ModelRuntime,
  SessionManager,
  createExtensionRuntime,
} from "@earendil-works/pi-coding-agent";
import { InMemoryCredentialStore, InMemoryModelsStore } from "@earendil-works/pi-ai";
import { contract, first, firstId, third } from "./contract-fixture.ts";
import { describe, expect, it, vi } from "vitest";
import { TaskProvider } from "./provider.ts";
import { WorkerImport } from "./handoff.ts";
import { branchSnapshot } from "./state.ts";
import piTodo from "./index.ts";
import { registration } from "./registration-fixture.ts";

/**
 * Loads index against a real in-memory Pi session and context.
 * @returns {Promise<object>} Registered execution with native branch persistence.
 */
const caller = async (): Promise<{
  session: SessionManager;
  call: ReturnType<typeof registration>["tools"][number]["execute"];
  runner: ExtensionRunner;
  api: ReturnType<typeof registration>["api"];
}> => {
  const session = SessionManager.inMemory(),
    registered = registration(session),
    runtime = await ModelRuntime.create({
      credentials: new InMemoryCredentialStore(),
      modelsPath: null,
      modelsStore: new InMemoryModelsStore(),
      refreshOnCreate: false,
    }),
    runner = new ExtensionRunner(
      [],
      createExtensionRuntime(),
      ".",
      session,
      new ModelRegistry(runtime),
    );
  piTodo(registered.api);
  const tool = registered.tools.at(first);
  if (tool === undefined) {
    throw new Error("Expected registered todo tool");
  }
  return { api: registered.api, call: tool.execute, runner, session };
};

describe("worker seed path", () => {
  it("does not import an ordinary launch", { timeout: 10_000 }, async () => {
    expect.hasAssertions();
    const test = await caller(),
      ctx = test.runner.createToolContext("startup", new AbortController().signal),
      abort = vi.fn<() => void>(),
      notify = vi.fn<() => void>(),
      importer = new WorkerImport(test.api, new TaskProvider(test.api));
    ctx.abort = abort;
    ctx.ui.notify = notify;
    await importer.start({ reason: "startup", type: "session_start" }, ctx);
    expect(abort).not.toHaveBeenCalled();
    expect(notify).not.toHaveBeenCalled();
    expect(test.session.getBranch()).toStrictEqual([]);
  });

  it.each([null, false, Number.NaN, "", " "])(
    "rejects an invalid seed path: %j",
    { timeout: 10_000 },
    async (path) => {
      expect.hasAssertions();
      const test = await caller(),
        ctx = test.runner.createToolContext("startup", new AbortController().signal),
        abort = vi.fn<() => void>(),
        notify = vi.fn<() => void>(),
        importer = new WorkerImport(test.api, new TaskProvider(test.api), (): unknown => path);
      ctx.abort = abort;
      ctx.ui.notify = notify;
      await importer.start({ reason: "startup", type: "session_start" }, ctx);
      expect(abort).toHaveBeenCalledExactlyOnceWith();
      expect(notify).toHaveBeenCalledExactlyOnceWith("IMPORT_FAILED: invalid seed path", "error");
      expect(test.session.getBranch()).toStrictEqual([]);
    },
  );
});

describe("registered todo execution", () => {
  it(
    "uses real Pi session IDs and append provenance through index",
    { timeout: 10_000 },
    async () => {
      expect.hasAssertions();
      const { call, runner, session } = await caller(),
        context = runner.createToolContext("todo", new AbortController().signal);
      await call(
        {
          items: [{ content: "Build", contract: contract(), key: "build" }],
          op: "init",
          start: "build",
        },
        context,
      );
      const list = branchSnapshot(session.getBranch()),
        unit = list.tasks.at(first)?.unit;
      expect(unit?.reference).toMatchObject({
        contractEntryId: session.getLeafId(),
        revision: 1,
        sessionId: session.getSessionId(),
      });
      expect(unit?.localSessionId).toBe(session.getSessionId());
      await call({ evidence: "tests passed", op: "done", task: firstId(list) }, context);
      expect(branchSnapshot(session.getBranch()).tasks.at(first)?.status).toBe("completed");
      expect(session.getBranch()).toHaveLength(third);
    },
  );
});
