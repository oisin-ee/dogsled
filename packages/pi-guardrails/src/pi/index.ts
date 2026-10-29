/*
 * The pi adapter: the same shell and edit policies as the Claude Code and
 * Codex hooks, plus pi's timeout and sandbox-wait gates, on every tool call.
 */
import {
  EDIT_TOOL_NAMES,
  SHELL_TOOL_NAMES,
  canonicalToolName,
  commandsFromToolInput,
} from "./tool-inputs.ts";
import { codeWaitViolation, enforceTimeout } from "./tool-timeouts.ts";
import { classifyEdits, editInputsFromPayload } from "../policies/edit.ts";
import { SHELL_CLASSIFIERS } from "../policies/shell.ts";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { isRecord } from "../common.ts";
import type { Command } from "../common.ts";

type GuardrailsAPI = Readonly<Pick<ExtensionAPI, "on">>;

/** The parts of a tool call the gates read; `input` is mutable, as pi allows. */
interface ToolCall {
  readonly toolName: string;
  readonly input: Record<string, unknown>;
}

// A codemode script's nested calls nest at most this deep before the gate stops looking.
const MAX_NESTING = 8,
  /**
   * The block message of the first shell policy that refuses one of `commands`.
   * @param {readonly Command[]} commands The commands a tool call runs.
   * @param {string} cwd The working directory.
   * @returns {string | null} The block message.
   */
  shellPolicyViolation = (commands: readonly Command[], cwd: string): string | null => {
    for (const command of commands) {
      for (const classify of SHELL_CLASSIFIERS) {
        const message = classify(command, cwd);
        if (message !== null) {
          return message;
        }
      }
    }
    return null;
  },
  shellViolation = (call: ToolCall, cwd: string): string | null =>
    shellPolicyViolation(
      commandsFromToolInput(call.toolName, call.input).filter((command) => command.length > 0),
      cwd,
    ),
  editViolation = (call: ToolCall, cwd: string): string | null =>
    classifyEdits(editInputsFromPayload({ tool_input: call.input, tool_name: call.toolName }), cwd),
  /**
   * The nested calls of a codemode input.
   * @param {Readonly<Record<string, unknown>>} input The tool input.
   * @returns {readonly ToolCall[]} The calls of the envelope.
   */
  nestedCalls = (input: Readonly<Record<string, unknown>>): readonly ToolCall[] => {
    const uses = input["tool_uses"];
    if (!Array.isArray(uses)) {
      return [];
    }
    const calls: ToolCall[] = [];
    for (const use of uses) {
      if (!isRecord(use)) {
        continue;
      }
      const toolName = use["recipient_name"];
      const parameters = use["parameters"];
      calls.push({
        input: isRecord(parameters) ? parameters : {},
        toolName: typeof toolName === "string" ? toolName : "",
      });
    }
    return calls;
  },
  toolViolation = (event: ToolCall, cwd: string, depth = 0): string | null => {
    if (depth > MAX_NESTING) {
      return null;
    }
    const call = { input: event.input, toolName: canonicalToolName(event.toolName) },
      own =
        enforceTimeout(call.toolName, call.input) ??
        codeWaitViolation(call.toolName, call.input) ??
        (SHELL_TOOL_NAMES.has(call.toolName) ? shellViolation(call, cwd) : null) ??
        (EDIT_TOOL_NAMES.has(call.toolName) ? editViolation(call, cwd) : null);
    if (own !== null) {
      return own;
    }
    for (const child of nestedCalls(call.input)) {
      const message = toolViolation(child, cwd, depth + 1);
      if (message !== null) {
        return message;
      }
    }
    return null;
  };

/**
 * Registers the guardrails on pi's `tool_call` event. A policy that throws,
 * such as an unparsable command, blocks the call with its message.
 * @param {GuardrailsAPI} pi The extension API methods used here.
 */
export default function piGuardrails(pi: GuardrailsAPI): void {
  pi.on("tool_call", (event, ctx) => {
    try {
      const message = toolViolation(event, ctx.cwd);
      return message === null ? undefined : { block: true, reason: message };
    } catch (error) {
      return { block: true, reason: error instanceof Error ? error.message : String(error) };
    }
  });
}

export { shellPolicyViolation };
