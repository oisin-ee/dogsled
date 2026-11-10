import { Replay, initial, taskCount } from "./delta-fixture.ts";
import { REMINDER_ENTRY } from "./reminder.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { branchSnapshot } from "./state.ts";

const changes = 60,
  rounds = 20,
  first = 0,
  step = 1,
  reminderInterval = 5,
  userInterval = 7,
  boundaryEntries = (index: number): SessionEntry[] => {
    const entries: SessionEntry[] = [];
    if (index % reminderInterval === first) {
      entries.push({
        content: "Earlier reminder",
        customType: REMINDER_ENTRY,
        display: false,
        id: `reminder-${index}`,
        parentId: null,
        timestamp: "2026-11-10T00:00:00Z",
        type: "custom_message",
      });
    }
    if (index % userInterval === first) {
      entries.push({
        id: `user-${index}`,
        message: { content: "Continue", role: "user", timestamp: first },
        parentId: null,
        timestamp: "2026-11-10T00:00:00Z",
        type: "message",
      });
    }
    return entries;
  },
  changeRound = (
    replay: { readonly branch: readonly SessionEntry[]; readonly call: Replay["call"] },
    index: number,
  ): void => {
    const id = branchSnapshot(replay.branch).tasks.at(index)?.unit?.reference.unitId;
    if (id === undefined) {
      throw new Error("Missing scripted task");
    }
    replay.call({ description: `Revision ${index}`, op: "update", task: id });
    replay.call({ op: "start", task: id });
    replay.call({ evidence: "Relevant unit checks pass", op: "done", task: id });
  },
  sequence = (): Replay => {
    const replay = new Replay();
    replay.save(initial(taskCount), true);
    for (let index = first; index < rounds; index += step) {
      changeRound(replay, index);
      replay.branch.push(...boundaryEntries(index));
      replay.full.push(...boundaryEntries(index));
    }
    return replay;
  };

export { changes, sequence };
