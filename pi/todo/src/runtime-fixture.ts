import {
  DefaultResourceLoader,
  ModelRuntime,
  SessionManager,
  SettingsManager,
  createAgentSession,
  createCodemodeExtension,
  createEventBus,
} from "@earendil-works/pi-coding-agent";
import {
  InMemoryCredentialStore,
  InMemoryModelsStore,
  fauxAssistantMessage,
  fauxProvider,
  fauxToolCall,
} from "@earendil-works/pi-ai";
import type { ExtensionFactory } from "@earendil-works/pi-coding-agent";
import type { TaskProviderV1 } from "./provider.ts";
import type { TodoParams } from "./tool.ts";
import { Type } from "typebox";
import { contract } from "./contract-fixture.ts";
import piTodo from "./index.ts";
import { registerSeedSource } from "./seed-fixture.ts";

type Session = Awaited<ReturnType<typeof createAgentSession>>["session"];
interface Startup {
  readonly cwd: string;
  readonly seedPath: string;
}
/** One selectable model in the runtime catalog. */
interface Selection {
  readonly id: string;
  readonly kind?: "physical" | "virtual";
  readonly provider: string;
}
interface RuntimeFixture {
  readonly session: Readonly<Pick<Session, "dispose" | "abort" | "messages">> & {
    readonly extensionRunner: Readonly<Pick<Session["extensionRunner"], "emitToolResult" | "emit">>;
  };
  readonly manager: SessionManager;
  readonly provider: () => TaskProviderV1;
  readonly call: (name: string, args: Readonly<Record<string, string>>) => Promise<void>;
  readonly create: () => Promise<void>;
  readonly select: (provider: string, id: string) => Promise<void>;
  readonly todo: (params: TodoParams) => Promise<unknown>;
}
/** Plain description of one runtime fixture. */
interface FixtureRequest {
  readonly collision: boolean;
  readonly later: ExtensionFactory | undefined;
  readonly selections: readonly Selection[];
  readonly startup: Startup | undefined;
}

/**
 * Creates resources without user configuration or external services.
 * @param {Readonly<FixtureRequest>} request Fixture description.
 * @returns {object} Local resource owners.
 */
const resources = (
    request: Readonly<FixtureRequest>,
  ): {
    events: ReturnType<typeof createEventBus>;
    manager: SessionManager;
    settings: SettingsManager;
    loader: DefaultResourceLoader;
  } => {
    const { collision, later, startup } = request,
      events = createEventBus(),
      manager = SessionManager.inMemory(),
      settings = SettingsManager.inMemory({
        compaction: { enabled: false },
        retry: { enabled: false },
      }),
      loader = new DefaultResourceLoader({
        additionalExtensionPaths: collision ? [] : [new URL("index.ts", import.meta.url).pathname],
        agentDir: "/nonexistent/pi-todo-test",
        cwd: startup?.cwd ?? "/tmp",
        eventBus: events,
        extensionFactories: [
          createCodemodeExtension(),
          ...(later === undefined ? [] : [later]),
          ...(collision ? [piTodo] : []),
        ],
        noContextFiles: true,
        noExtensions: true,
        noPromptTemplates: true,
        noSkills: true,
        noThemes: true,
        settingsManager: settings,
      });
    if (startup !== undefined) {
      registerSeedSource(events, { path: startup.seedPath, version: 1 });
    }
    return { events, loader, manager, settings };
  },
  /**
   * Requires one synchronous response from the real event bus.
   * @param {Readonly<object>} events Runtime event bus.
   * @returns {TaskProviderV1} Acquired extension capability.
   */
  acquire = (events: Readonly<ReturnType<typeof createEventBus>>): TaskProviderV1 => {
    const replies: TaskProviderV1[] = [],
      version = 1;
    events.emit("oisin.tasks.acquire.v1", {
      reply: (value: TaskProviderV1): void => {
        replies.push(value);
      },
      version,
    });
    const [provider] = replies;
    if (provider === undefined || replies.length !== version) {
      throw new Error("Expected exactly one synchronous provider");
    }
    return provider;
  },
  /**
   * Executes the registered todo entry point, including its persistence checks.
   * @param {Session} session Real SDK session.
   * @param {TodoParams} params Requested mutation.
   * @returns {Promise<unknown>} Actual tool result.
   */
  todoCall = async (
    session: {
      readonly extensionRunner: Readonly<
        Pick<Session["extensionRunner"], "getToolDefinition" | "createToolContext">
      >;
    },
    params: TodoParams,
  ): Promise<unknown> => {
    const tool = session.extensionRunner.getToolDefinition("todo");
    if (tool === undefined) {
      throw new Error("Missing todo registration");
    }
    return tool.execute(
      "review",
      params,
      undefined,
      undefined,
      session.extensionRunner.createToolContext("review", new AbortController().signal),
    );
  },
  /**
   * Creates the model catalog with the requested physical and virtual entries.
   * @param {Readonly<FixtureRequest>} request Fixture description.
   * @returns {object} Scripted provider and model catalog owner.
   */
  catalog = async (
    request: Readonly<FixtureRequest>,
  ): Promise<{
    faux: ReturnType<typeof fauxProvider>;
    modelRuntime: ModelRuntime;
  }> => {
    const modelRuntime = await ModelRuntime.create({
        credentials: new InMemoryCredentialStore(),
        modelsPath: null,
        modelsStore: new InMemoryModelsStore(),
        refreshOnCreate: false,
      }),
      faux = fauxProvider({
        models: [
          ...request.selections
            .filter((selection) => selection.kind === "physical")
            .map((selection) => ({ id: selection.id })),
          { id: "faux-1" },
        ],
        tokensPerSecond: 0,
      });
    modelRuntime.registerNativeProvider(faux.provider);
    for (const selection of request.selections.filter((entry) => entry.kind !== "physical")) {
      modelRuntime.registerVirtualModel({
        id: selection.id,
        name: selection.id,
        provider: selection.provider,
        route: () => ({ model: faux.getModel(), thinkingLevel: "off" }),
        thinkingLevels: ["off"],
      });
    }
    await modelRuntime.refresh({ allowNetwork: false });
    return { faux, modelRuntime };
  },
  /**
   * Builds the real session and its callers over prepared resources.
   * @param {Readonly<FixtureRequest>} request Fixture description.
   * @returns {Promise<RuntimeFixture>} Isolated runtime and callers.
   */
  buildFixture = async (request: Readonly<FixtureRequest>): Promise<RuntimeFixture> => {
    const setup = resources(request),
      { faux, modelRuntime } = await catalog(request);
    await setup.loader.reload();
    const { session } = await createAgentSession({
      cwd: request.startup?.cwd ?? "/tmp",
      model: faux.getModel(),
      modelRuntime,
      resourceLoader: setup.loader,
      sessionManager: setup.manager,
      settingsManager: setup.settings,
      thinkingLevel: "off",
    });
    await session.bindExtensions({});
    session.setActiveToolsByName([...session.getActiveToolNames(), "codemode"]);
    return {
      call: async (name, args): Promise<void> => {
        faux.setResponses([
          fauxAssistantMessage(fauxToolCall(name, args)),
          fauxAssistantMessage("Done."),
        ]);
        await session.prompt("Exercise one tool.");
      },
      create: async (): Promise<void> => {
        await todoCall(session, {
          items: [{ content: "Build", contract: contract(), key: "build" }],
          op: "init",
          start: "build",
        });
      },
      manager: setup.manager,
      provider: () => acquire(setup.events),
      select: async (provider, id): Promise<void> => {
        const model = modelRuntime.getModel(provider, id);
        if (model === undefined) {
          throw new Error(`Unknown selection: ${provider}/${id}`);
        }
        await session.setModel(model);
      },
      session,
      todo: async (params): Promise<unknown> => todoCall(session, params),
    };
  },
  /**
   * Loads the real runner and codemode with a scripted model.
   * @param {ExtensionFactory} later Optional additional guard or test tool.
   * @param {boolean} collision Whether the other todo owner loads first.
   * @param {Startup | undefined} startup Optional worker startup configuration.
   * @returns {Promise<RuntimeFixture>} Isolated runtime and callers.
   */
  runtimeFixture = async (
    later?: ExtensionFactory,
    collision = false,
    startup?: Startup,
  ): Promise<RuntimeFixture> => buildFixture({ collision, later, selections: [], startup }),
  /**
   * Loads the real runtime with selectable virtual and physical models.
   * @param {readonly Selection[]} selections Selectable catalog entries.
   * @returns {Promise<RuntimeFixture>} Runtime with the given selections.
   */
  selectableFixture = async (selections: readonly Selection[]): Promise<RuntimeFixture> =>
    buildFixture({ collision: false, later: undefined, selections, startup: undefined }),
  /**
   * Produces a deterministic tool result.
   * @returns {object} Successful content.
   */
  emptyResult = (): { content: [{ type: "text"; text: string }]; details: undefined } => ({
    content: [{ text: "ok", type: "text" }],
    details: undefined,
  }),
  emptyParameters = Type.Object({});

export { emptyParameters, emptyResult, runtimeFixture, selectableFixture };
export type { RuntimeFixture, Selection };
