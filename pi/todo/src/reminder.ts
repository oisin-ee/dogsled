import type { SessionBoundaryDraft, SessionEntry } from "@earendil-works/pi-coding-agent";
import type { ListFrame } from "./branch-state.ts";
import type { ReminderFact } from "./wasm.ts";
import { branchList } from "./state.ts";
import { completionReminder } from "./wasm.ts";

const REMINDER_ENTRY = "todo-reminder",
  END_TURN = "end_turn",
  absent = -1,
  first = 0,
  step = 1;

/** Retains only the final list and progress since the last reminder. */
class ReminderHistory {
  readonly #facts: ReminderFact[] = [];
  #latest: ListFrame | undefined = undefined;
  #progress = false;
  #lastReminder = absent;

  public boundary(kind: "user" | "reminder"): void {
    this.#facts.push({ kind });
    this.#lastReminder = kind === "user" ? absent : this.#facts.length - step;
    this.#progress = false;
  }

  public snapshot(frame: ListFrame): void {
    this.#latest = frame;
    this.#progress ||= frame.progress;
  }

  public facts(): ReminderFact[] {
    if (this.#latest !== undefined) {
      const snapshot: ReminderFact = { kind: "snapshot", list: this.#latest.list };
      if (this.#lastReminder >= first && !this.#progress) {
        this.#facts.splice(this.#lastReminder, first, snapshot);
      } else {
        this.#facts.push(snapshot);
      }
    }
    return this.#facts;
  }
}

/**
 * Reduces branch history without materializing intermediate delta lists.
 * @param {readonly SessionEntry[]} branch Persisted entries.
 * @param {readonly SessionBoundaryDraft[]} drafts Proposed entries.
 * @returns {ReminderFact[]} Equivalent reminder facts with only the final list.
 */
const reminderFacts = (
    branch: readonly SessionEntry[],
    drafts: readonly SessionBoundaryDraft[],
  ): ReminderFact[] => {
    const history = new ReminderHistory(),
      entries = [...branch, ...drafts],
      frames = branchList(entries);
    let next = frames.next();
    for (const entry of entries) {
      if (entry.type === "message" && entry.message.role === "user") {
        history.boundary("user");
      } else if (next.done !== true && next.value.entry === entry) {
        history.snapshot(next.value);
        next = frames.next();
      } else if (entry.type === "custom_message" && entry.customType === REMINDER_ENTRY) {
        history.boundary("reminder");
      }
    }
    return history.facts();
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
  ): string | undefined => completionReminder({ facts: reminderFacts(branch, drafts) }),
  /**
   * Whether the agent ended this turn with a successful `end_turn` call. Its message is
   * the turn's reply, so a reminder after it would only bury that reply.
   * @param {readonly SessionEntry[]} branch Persisted entries.
   * @returns {boolean} Whether the newest tool result since the owner's message is `end_turn`.
   */
  endedTurn = (branch: readonly SessionEntry[]): boolean => {
    for (const entry of branch.toReversed()) {
      if (entry.type === "message") {
        const { message } = entry;
        if (message.role === "user") {
          return false;
        }
        if (message.role === "toolResult") {
          return message.toolName === END_TURN && !message.isError;
        }
      }
    }
    return false;
  };

export { REMINDER_ENTRY, endedTurn, reminder, reminderFacts };
