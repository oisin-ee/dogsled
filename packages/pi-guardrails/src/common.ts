/* Shared shapes and helpers for the hook payloads and the policies. */
import { shellCallGuards, shellCalls } from "./shell-parser.ts";

/** Shell text, or an argument list from an argv-style spawn. */
type Command = string | readonly string[];

/** A hook's verdict: exit code 2 blocks the tool call; other keys are printed as JSON. */
interface HookResult {
  readonly code: number;
  readonly [key: string]: unknown;
}

/** Where a hook writes its block message. */
interface HookIO {
  readonly stderr: (message: string) => void;
}

/** A harness hook payload, as Claude Code and Codex send it. */
interface HookPayload {
  readonly hook_event_name?: string;
  readonly tool_name?: string;
  readonly tool_input?: Readonly<Record<string, unknown>>;
  readonly cwd?: string;
  // Other event fields, such as Stop's `last_assistant_message`, which no policy reads.
  readonly [field: string]: unknown;
}

const BLOCK_CODE = 2,
  SHELL_TOOLS: ReadonlySet<string> = new Set(["Bash", "bash", "shell"]),
  EDIT_TOOLS: ReadonlySet<string> = new Set([
    "Edit",
    "Write",
    "MultiEdit",
    "apply_patch",
    "write",
    "edit",
    "patch",
  ]),
  isRecord = (value: unknown): value is Record<string, unknown> =>
    typeof value === "object" && value !== null && !Array.isArray(value),
  text = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined),
  /**
   * The shell text of a hook payload's tool input.
   * @param {HookPayload} payload A hook payload.
   * @returns {string} The command, or "".
   */
  commandFromPayload = (payload: HookPayload): string => {
    const input = payload.tool_input ?? {};
    return text(input["command"]) ?? text(input["cmd"]) ?? text(input["commandText"]) ?? "";
  },
  /**
   * The calls in `command`. An argument list from an argv-style spawn is
   * already one call; only shell text needs parsing.
   * @param {Command} command Shell text or an argument list.
   * @returns {string[][]} One word list per call.
   */
  splitShell = (command: Command): string[][] =>
    typeof command === "string" ? shellCalls(command) : [[...command]],
  /**
   * The `&&` guard pipeline of each splitShell piece. An argv-style call has no
   * shell operators, so no guard.
   * @param {Command} command Shell text or an argument list.
   * @returns {(string[][] | null)[]} One guard per call.
   */
  splitShellGuards = (command: Command): (string[][] | null)[] =>
    typeof command === "string" ? shellCallGuards(command) : [null],
  /**
   * The words of the first call in `command`.
   * @param {Command} command Shell text or an argument list.
   * @returns {string[]} The words, or none.
   */
  tokens = (command: Command): string[] =>
    typeof command === "string" ? (shellCalls(command)[0] ?? []) : [...command],
  /**
   * A command word without quotes, directory, or `@version`.
   * @param {string} token A command word.
   * @returns {string} The bare command name.
   */
  commandName = (token: string): string =>
    (
      token
        .replaceAll(/^["']|["']$/gu, "")
        .split("/")
        .pop() ?? ""
    ).split("@")[0] ?? "",
  /**
   * Blocks a tool call: writes `message` and returns exit code 2.
   * @param {string} message The reason shown to the agent.
   * @param {HookIO} io Where the message goes.
   * @returns {HookResult} The blocking result.
   */
  block = (message: string, io: HookIO): HookResult => {
    io.stderr(`${message}\n`);
    return { code: BLOCK_CODE };
  },
  /**
   * Allows a tool call, with optional output for the harness.
   * @param {Readonly<Record<string, unknown>>} extra Keys printed as JSON.
   * @returns {HookResult} The allowing result.
   */
  ok = (extra: Readonly<Record<string, unknown>> = {}): HookResult => ({ code: 0, ...extra });

export {
  BLOCK_CODE,
  EDIT_TOOLS,
  SHELL_TOOLS,
  block,
  commandFromPayload,
  commandName,
  isRecord,
  ok,
  splitShell,
  splitShellGuards,
  tokens,
};
export type { Command, HookIO, HookPayload, HookResult };
