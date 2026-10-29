import type {
  ExtensionAPI,
  ExtensionToolContext,
  ToolDefinition,
  ToolInfo,
} from "@earendil-works/pi-coding-agent";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import type { TSchema } from "typebox";
import type { TodoParams } from "./tool.ts";
import { Parse as parse } from "typebox/value";

/** The API subset the extension uses, fully typed without loading the SDK. */
type TodoAPI = Readonly<
  Pick<
    ExtensionAPI,
    | "appendEntry"
    | "events"
    | "getAllTools"
    | "exec"
    | "on"
    | "registerCommand"
    | "registerShortcut"
    | "registerTool"
    | "sendMessage"
  >
>;
interface Registered {
  readonly name: string;
  readonly parameters: TSchema;
  readonly execute: (params: TodoParams, ctx: ExtensionToolContext) => Promise<unknown>;
}

interface Registration {
  readonly api: TodoAPI;
  readonly commands: readonly string[];
  readonly events: readonly string[];
  readonly shortcuts: readonly string[];
  readonly tools: readonly Registered[];
}

/**
 * Captures pi registration facts without loading the SDK.
 * @param {Readonly<Pick<SessionManager, "appendCustomEntry">>} session Pi session append owner.
 * @returns {Registration} Typed registration methods and captured facts.
 */
const silent = (): void => {
    /* Registration has no transport side effects. */
  },
  /**
   * Projects registered test tools to real ownership metadata.
   * @param {readonly Registered[]} tools Captured definitions.
   * @returns {ToolInfo[]} Runtime metadata.
   */
  metadata = (tools: readonly Registered[]): ToolInfo[] =>
    tools.map((tool) => ({
      description: "Registered test tool",
      exposure: "direct",
      name: tool.name,
      parameters: tool.parameters,
      sourceInfo: {
        origin: "top-level",
        path: new URL("index.ts", import.meta.url).pathname,
        scope: "project",
        source: "extension",
      },
    })),
  /**
   * Converts the registered execution surface to a typed test caller.
   * @param {Readonly<object>} tool Registered tool definition.
   * @returns {Registered} Captured registration.
   */
  registeredTool = <Params extends TSchema, Details, State>(
    tool: Readonly<Pick<ToolDefinition<Params, Details, State>, "execute" | "name" | "parameters">>,
  ): Registered => ({
    execute: async (params, ctx) =>
      tool.execute("test", parse(tool.parameters, params), undefined, undefined, ctx),
    name: tool.name,
    parameters: tool.parameters,
  }),
  registration = (
    session: Readonly<Pick<SessionManager, "appendCustomEntry">> = SessionManager.inMemory(),
  ): Registration => {
    const commands: string[] = [],
      events: string[] = [],
      shortcuts: string[] = [],
      tools: Registered[] = [];
    return {
      api: {
        appendEntry: (customType, data): void => {
          session.appendCustomEntry(customType, data);
        },
        events: {
          emit: silent,
          on: (): (() => void) => silent,
        },
        exec: async () => {
          throw new Error("Unexpected startup execution");
        },
        getAllTools: () => metadata(tools),
        on: (event): (() => void) => {
          events.push(event);
          return silent;
        },
        registerCommand: (name): void => {
          commands.push(name);
        },
        registerShortcut: (key): void => {
          shortcuts.push(key);
        },
        registerTool: <Params extends TSchema, Details, State>(
          tool: Readonly<
            Pick<ToolDefinition<Params, Details, State>, "execute" | "name" | "parameters">
          >,
        ): void => {
          tools.push(registeredTool(tool));
        },
        sendMessage: silent,
      },
      commands,
      events,
      shortcuts,
      tools,
    };
  };

export { registration };
