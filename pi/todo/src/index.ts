import { ownsTodo, settle } from "./lifecycle.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { PlanApprovedHandoff } from "./plan-approved.ts";
import { TaskProvider } from "./provider.ts";
import { TodoPresentation } from "./presentation.ts";
import { TodoWork } from "./work.ts";
import type { WorkHost } from "./work.ts";
import { WorkerImport } from "./handoff.ts";
import { parameters } from "./schema.ts";
import { registerGate } from "./gate.ts";
import { workUnitGuidance } from "./guidance.ts";

type TodoAPI = WorkHost &
  Readonly<
    Pick<
      ExtensionAPI,
      | "appendEntry"
      | "getAllTools"
      | "exec"
      | "on"
      | "registerCommand"
      | "registerShortcut"
      | "registerTool"
      | "sendMessage"
    >
  >;

/**
 * Registers the branch-backed `todo` tool and its completion reminder.
 * @param {TodoAPI} pi The extension API methods used here.
 */
export default function piTodo(pi: TodoAPI): void {
  const handoff = new PlanApprovedHandoff(pi),
    presentation = new TodoPresentation(),
    work = new TodoWork(pi, presentation),
    provider = new TaskProvider(pi),
    workerImport = new WorkerImport(pi, provider);
  work.register(pi);
  provider.register();
  registerGate(pi, provider);
  pi.registerTool({
    description: `Keep branch-local outcomes and executable contracts. Use returned IDs, not labels. Draft creation is pending. Start one complete self-assigned leaf; workers require their assigned profile through the executor. Update replaces pending contract fields and may set or clear an open item's description. Done takes a short note. Parents require explicit integration review after children close. ${workUnitGuidance}`,
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const result = provider.run(params, ctx);
      work.publish(ctx);
      presentation.refresh(ctx);
      return result;
    },
    executionMode: "sequential",
    label: "Todo",
    name: "todo",
    parameters,
  });
  pi.on("session_start", async (event, ctx) => {
    provider.context(ctx);
    await workerImport.start(event, ctx);
    presentation.start(pi, ctx);
    work.start(ctx);
    handoff.start(ctx);
  });
  pi.on("session_shutdown", () => {
    provider.stop();
    handoff.stop();
    work.stop();
  });
  pi.on("session_tree", (_event, ctx) => {
    provider.context(ctx);
    if (ownsTodo(pi)) {
      work.publish(ctx);
      presentation.reset(ctx);
    }
  });
  pi.on("agent_before_settle", (event, ctx) => settle(pi, event, ctx));
}
