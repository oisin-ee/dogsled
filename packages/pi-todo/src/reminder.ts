import type { SessionBoundaryDraft, SessionEntry } from "@earendil-works/pi-coding-agent";
import { completionReminder, parse } from "./wasm.ts";
import type { ReminderFact } from "./wasm.ts";
import { STATE_ENTRY } from "./state.ts";

const REMINDER_ENTRY = "todo-reminder",
  /**
   * Projects pi's branch and boundary drafts into core-owned reminder facts.
   * @param {readonly SessionEntry[]} branch Persisted branch entries.
   * @param {readonly SessionBoundaryDraft[]} drafts Proposed boundary entries.
   * @returns {string | undefined} The next reminder, when permitted.
   */
  reminder = (
    branch: readonly SessionEntry[],
    drafts: readonly SessionBoundaryDraft[],
  ): string | undefined => {
    const facts: ReminderFact[] = [];
    for (const entry of [...branch, ...drafts]) {
      if (entry.type === "message" && entry.message.role === "user") {
        facts.push({ kind: "user" });
      } else if (entry.type === "custom" && entry.customType === STATE_ENTRY) {
        facts.push({ kind: "snapshot", list: parse(entry.data) });
      } else if (entry.type === "custom_message" && entry.customType === REMINDER_ENTRY) {
        facts.push({ kind: "reminder" });
      }
    }
    return completionReminder({ facts });
  };

export { REMINDER_ENTRY, reminder };
