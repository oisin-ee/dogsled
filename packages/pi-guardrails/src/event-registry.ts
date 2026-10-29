/* Routes a Claude Code or Codex hook event to the policies. */
import { EDIT_TOOLS, SHELL_TOOLS, block, commandFromPayload, ok } from "./common.ts";
import { SHELL_CLASSIFIERS } from "./policies/shell.ts";
import { classifyEdits, editInputsFromPayload } from "./policies/edit.ts";
import type { HookIO, HookPayload, HookResult } from "./common.ts";

/** What a hook run can reach. */
interface Runtime extends HookIO {
  readonly cwd: string;
}

const preShell = (runtime: Runtime, payload: HookPayload): string | null => {
    const command = commandFromPayload(payload),
      cwd = payload.cwd ?? runtime.cwd;
    for (const classify of SHELL_CLASSIFIERS) {
      const message = classify(command, cwd);
      if (message !== null) {
        return message;
      }
    }
    return null;
  },
  /**
   * The verdict for one hook event. Only `PreToolUse` on a shell or edit tool
   * can block; a policy that throws, such as an unparsable command, blocks.
   * @param {Runtime} runtime The working directory and stderr.
   * @param {HookPayload} payload The hook payload.
   * @returns {HookResult} Exit code 2 to block, otherwise 0.
   */
  runEvent = (runtime: Runtime, payload: HookPayload): HookResult => {
    if (payload.hook_event_name !== "PreToolUse") {
      return ok();
    }
    const tool = payload.tool_name ?? "";
    if (SHELL_TOOLS.has(tool)) {
      try {
        const message = preShell(runtime, payload);
        return message === null ? ok() : block(message, runtime);
      } catch (error) {
        return block(error instanceof Error ? error.message : String(error), runtime);
      }
    }
    if (EDIT_TOOLS.has(tool)) {
      const message = classifyEdits(editInputsFromPayload(payload), payload.cwd ?? runtime.cwd);
      return message === null ? ok() : block(message, runtime);
    }
    return ok();
  };

export { runEvent };
export type { Runtime };
