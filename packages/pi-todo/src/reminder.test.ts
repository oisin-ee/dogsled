import { REMINDER_ENTRY, reminder } from "./reminder.ts";
import { describe, expect, it } from "vitest";
import { STATE_ENTRY } from "./state.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";

const draft = {
    content: "earlier reminder",
    customType: REMINDER_ENTRY,
    display: false,
    type: "custom_message" as const,
  },
  open = { tasks: [{ content: "a", status: "in_progress" }] },
  /**
   * Builds a persisted branch entry from a snapshot or reminder.
   * @param {unknown} data Untrusted snapshot data.
   * @returns {SessionEntry} A persisted snapshot.
   */
  snapshot = (data: unknown): SessionEntry => ({
    customType: STATE_ENTRY,
    data,
    id: crypto.randomUUID(),
    parentId: null,
    timestamp: "2026-10-16T00:00:00Z",
    type: "custom",
  }),
  storedReminder: SessionEntry = Object.assign({}, draft, {
    id: "reminder",
    parentId: null,
    timestamp: "2026-10-16T00:00:00Z",
  });

describe("reminder projection", () => {
  it("includes pending boundary entries", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const branch = [snapshot(open)];
    expect(reminder(branch, [])).toContain("Reminder 1/3");
    expect(reminder(branch, [draft])).toBeUndefined();
    expect(
      reminder(branch, [draft, { customType: STATE_ENTRY, data: open, type: "custom" }]),
    ).toBeUndefined();
  });

  it("uses changed snapshots from prior boundary handlers", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const changed = { tasks: [{ content: "b", status: "in_progress" }] };
    expect(
      reminder(
        [snapshot(open)],
        [draft, { customType: STATE_ENTRY, data: changed, type: "custom" }],
      ),
    ).toContain("Reminder 2/3");
  });
});

describe("hierarchical reminders", () => {
  it("names open children and explicit parent review", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const branch = [
      snapshot({
        tasks: [
          {
            content: "Outcome",
            status: "in_progress",
            subtasks: [{ content: "Build", status: "pending" }],
          },
        ],
      }),
    ];
    expect(reminder(branch, [])).toContain("Outcome → Build");
    branch.push(
      snapshot({
        tasks: [
          {
            content: "Outcome",
            status: "in_progress",
            subtasks: [{ content: "Build", status: "completed" }],
          },
        ],
      }),
    );
    expect(reminder(branch, [])).toContain("Ready for review");
    expect(reminder(branch, [])).toContain("Reminder 1/3");
  });
});

describe("reminder replay", () => {
  it("replays the budget and resets on the latest user message", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const branch: SessionEntry[] = [
      snapshot(open),
      storedReminder,
      snapshot({ tasks: [] }),
      storedReminder,
      snapshot(open),
      storedReminder,
      snapshot({ tasks: [{ content: "b", status: "in_progress" }] }),
    ];
    expect(reminder(branch, [])).toBeUndefined();
    branch.push({
      id: "prompt",
      message: { content: "Continue", role: "user", timestamp: 0 },
      parentId: null,
      timestamp: "2026-10-16T00:00:00Z",
      type: "message",
    });
    expect(reminder(branch, [])).toContain("Reminder 1/3");
  });

  it("fails explicitly on corrupt snapshots", { timeout: 5000 }, () => {
    expect.hasAssertions();
    expect(reminder([], [])).toBeUndefined();
    expect(() =>
      reminder([snapshot({ tasks: [{ content: "a", status: "invalid" }] })], []),
    ).toThrow("unknown variant");
  });
});
