import type {
  AgentBeforeSettleEvent,
  ContextEvent,
  ExtensionAPI,
  SessionStartEvent,
  TurnEndEvent,
} from "@earendil-works/pi-coding-agent";
import { previousBranch, readConfig } from "./config.ts";
import { Adapter } from "./adapter.ts";
import { projectContext } from "./projection.ts";

/** The adapter methods the boundaries call. */
type BoundaryAdapter = Readonly<Pick<Adapter, "run" | "turn">>;

/**
 * Registers actionable boundaries and context projection.
 * @param {ExtensionAPI} pi Host API.
 * @param {BoundaryAdapter} adapter Protocol adapter.
 */
const boundaries = (pi: ExtensionAPI, adapter: BoundaryAdapter): void => {
  pi.on("turn_end", async (event: TurnEndEvent, ctx) =>
    event.outcome === "completed" && event.message.role === "assistant"
      ? adapter.turn(event, ctx)
      : undefined,
  );
  pi.on("agent_before_settle", async (event: AgentBeforeSettleEvent, ctx) =>
    event.outcome === "completed" ? adapter.run({ kind: "settle" }, ctx, event.entries) : undefined,
  );
  pi.on("context", (event: ContextEvent, ctx) =>
    projectContext(ctx.sessionManager.getBranch(), event.messages),
  );
};

/**
 * Registers prewalk flags, lifecycle, commands, and boundaries.
 * @param {ExtensionAPI} pi Host API.
 */
export default function prewalk(pi: ExtensionAPI): void {
  pi.registerFlag("prewalk", {
    description: "Arm a one-shot prewalk for the startup session.",
    type: "boolean",
  });
  pi.registerFlag("no-prewalk", {
    description: "Keep the startup session unarmed.",
    type: "boolean",
  });
  pi.registerFlag("prewalk-into", {
    description: "Arm prewalk into provider/model.",
    type: "string",
  });
  const adapter = new Adapter(pi, { config: readConfig, previous: previousBranch });
  boundaries(pi, adapter);
  pi.on("session_start", async (event: SessionStartEvent, ctx) => {
    if (event.reason === "new") {
      await adapter.newSession(event.previousSessionFile, ctx);
    } else {
      const into = pi.getFlag("prewalk-into");
      await adapter.run(
        {
          flags: {
            into: typeof into === "string" ? into : undefined,
            no_prewalk: pi.getFlag("no-prewalk") === true,
            prewalk: pi.getFlag("prewalk") === true,
          },
          kind: "start",
          reason: event.reason,
        },
        ctx,
      );
    }
  });
  pi.on("input", async (_event, ctx) => {
    await adapter.run({ kind: "input" }, ctx);
    return { action: "continue" };
  });
  pi.registerCommand("prewalk", {
    description: "Arm a handoff (no argument or on), or use off, restart, status.",
    async handler(command, ctx) {
      await adapter.run({ command: command.trim(), kind: "command" }, ctx);
    },
  });
}
