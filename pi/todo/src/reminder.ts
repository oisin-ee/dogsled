import type { SessionBoundaryDraft, SessionEntry } from "@earendil-works/pi-coding-agent";
import type { ReminderFact } from "./wasm.ts";
import { branchList } from "./state.ts";
import { completionReminder } from "./wasm.ts";

const REMINDER_ENTRY = "todo-reminder",
  /**
   * Projects pi's branch and boundary drafts into core-owned reminder facts.
   * @param {readonly SessionEntry[]} branch Persisted branch entries.
   * @param {readonly SessionBoundaryDraft[]} drafts Proposed boundary entries.
   * @returns {ReminderFact[]} The ordered reminder history.
   */
  reminderFacts = (
    branch: readonly SessionEntry[],
    drafts: readonly SessionBoundaryDraft[],
  ): ReminderFact[] => {
    const facts: ReminderFact[] = [],
      entries = [...branch, ...drafts],
      frames = branchList(entries);
    let next = frames.next();
    for (const entry of entries) {
      if (entry.type === "message" && entry.message.role === "user") {
        facts.push({ kind: "user" });
      } else if (next.done !== true && next.value.entry === entry) {
        facts.push({ kind: "snapshot", list: next.value.list });
        next = frames.next();
      } else if (entry.type === "custom_message" && entry.customType === REMINDER_ENTRY) {
        facts.push({ kind: "reminder" });
      }
    }
    return facts;
  },
  /**
   * Requests the next reminder from the core.
   * @param {readonly SessionEntry[]} branch Persisted branch entries.
   * @param {readonly SessionBoundaryDraft[]} drafts Proposed boundary entries.
   * @returns {string | undefined} The next reminder, when permitted.
   */
  reminder = (
    branch: readonly SessionEntry[],
    drafts: readonly SessionBoundaryDraft[],
  ): string | undefined => completionReminder({ facts: reminderFacts(branch, drafts) });

export { REMINDER_ENTRY, reminder, reminderFacts };
