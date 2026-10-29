import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { TaskProvider } from "./provider.ts";
import { ownsTodo } from "./lifecycle.ts";

/** Planning selections that skip the activation check, exact provider and model IDs. */
const planningSelections = new Set(["plan-claude", "plan-gpt"]),
  /**
   * Whether the selected model is a planning selection.
   * @param {Readonly<object>} model Selected model, when the session has one.
   * @returns {boolean} Whether the activation check does not apply.
   */
  isPlanning = (model: { readonly provider?: string; readonly id?: string } | undefined): boolean =>
    model?.provider === "tier" && planningSelections.has(model.id ?? ""),
  /** Exact structured reads and workflow controls, never command-text inference. */
  controls = new Set([
    "ask",
    "codemode",
    "interview",
    "todo",
    "tool_search",
    "read",
    "ls",
    "find",
    "grep",
    "hypa_read",
    "hypa_ls",
    "hypa_find",
    "hypa_grep",
    "web_search",
    "web_fetch",
    "lsp_diagnostics",
    "ctx_search",
    "ctx_stats",
    "mcp__codebase_memory__search_graph",
    "mcp__codebase_memory__get_code_snippet",
    "mcp__codebase_memory__trace_path",
    "mcp__codebase_memory__detect_changes",
    "hindsight_search_knowledge_pages",
    "hindsight_list_knowledge_pages",
    "hindsight_read_knowledge_page",
    "hindsight_reflect",
    "hindsight_sync_status",
    "hindsight_diagnose",
    "hindsight_capture_initiative",
    "mcp__dogsled__spawn",
    "mcp__dogsled__send",
    "mcp__dogsled__cancel",
    "mcp__dogsled__close",
    "mcp__dogsled__done",
    "mcp__dogsled__stop",
    "mcp__dogsled__read",
    "mcp__dogsled__status",
    "mcp__dogsled__inbox",
  ]),
  /**
   * Checks actual calls and leases execution-bearing calls.
   * @param {Readonly<ExtensionAPI>} pi Runtime hooks and metadata.
   * @param {Readonly<TaskProvider>} provider Task state and leases.
   */
  registerGate = (
    pi: Readonly<Pick<ExtensionAPI, "on" | "getAllTools">>,
    provider: Readonly<TaskProvider>,
  ): void => {
    pi.on("tool_call", (event: Readonly<{ toolName: string; toolCallId: string }>, ctx) => {
      if (!ownsTodo(pi)) {
        return { block: false };
      }
      provider.context(ctx);
      if (controls.has(event.toolName) || isPlanning(ctx.model)) {
        return { block: false };
      }
      const active = provider.capability().active();
      if (!active.ok || active.value === null) {
        return {
          block: true,
          reason: active.ok
            ? "Start a complete self-assigned leaf with todo before shell, code, mutation, or unknown tools."
            : active.error.message,
        };
      }
      provider.lease(event.toolCallId, active.value.ref);
      return { block: false };
    });
    pi.on("tool_execution_end", (event: Readonly<{ toolCallId: string }>) => {
      provider.release(event.toolCallId);
    });
  };

export { registerGate };
