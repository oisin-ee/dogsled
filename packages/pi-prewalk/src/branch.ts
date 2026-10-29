import type { Record as BranchRecord, Call } from "./wasm.ts";
import type { SessionEntry, SessionMessageEntry } from "@earendil-works/pi-coding-agent";
import { parse_state } from "./wasm.ts";

/** Only the fields the protocol reads from tool results. */
interface CallEvidence {
  readonly toolName: string;
  readonly isError: boolean;
  readonly nestedCalls?: {
    readonly calls: readonly { readonly name: string; readonly status: string }[];
  };
}

/**
 * Extracts call evidence, including calls without transcript entries.
 * @param {ToolResultMessage} result A completed outer tool result.
 * @returns {Call[]} Nested calls followed by the outer result.
 */
const calls = (result: CallEvidence): Call[] => [
    ...(result.nestedCalls?.calls.map((call) => ({
      name: call.name,
      success: call.status === "ok",
    })) ?? []),
    { name: result.toolName, success: !result.isError },
  ],
  /**
   * Translates message evidence.
   * @param {SessionMessageEntry} entry A transcript entry.
   * @returns {BranchRecord[]} Relevant messages.
   */
  messageRecords = (entry: SessionMessageEntry): BranchRecord[] => {
    if (entry.message.role === "toolResult") {
      return [{ calls: calls(entry.message), kind: "calls" }];
    }
    return entry.message.role === "user" ? [{ kind: "user" }] : [];
  },
  /**
   * Translates a pi entry. Rust validates protocol payloads.
   * @param {SessionEntry} entry One branch entry.
   * @returns {BranchRecord[]} Protocol evidence.
   */
  entryRecords = (entry: SessionEntry): BranchRecord[] => {
    if (
      entry.type === "custom" &&
      ["prewalk-arm", "prewalk-handoff", "prewalk-off"].includes(entry.customType)
    ) {
      return [{ kind: "state", state: parse_state(entry.data) }];
    }
    if (entry.type === "custom_message" && entry.customType.startsWith("prewalk-")) {
      return [{ id: entry.id, kind: "prompt", name: entry.customType }];
    }
    if (entry.type === "context_edit" && entry.replacement === null) {
      return [{ id: entry.targetId, kind: "scrub" }];
    }
    if (entry.type === "model_change" || entry.type === "thinking_level_change") {
      return [{ kind: "selection" }];
    }
    return entry.type === "message" ? messageRecords(entry) : [];
  },
  /**
   * Translates the active branch in transcript order.
   * @param {readonly SessionEntry[]} entries Current branch, oldest first.
   * @returns {BranchRecord[]} Protocol evidence.
   */
  branchRecords = (entries: readonly SessionEntry[]): BranchRecord[] =>
    entries.flatMap((entry) => entryRecords(entry));

export { branchRecords, calls };
export type { CallEvidence };
