import { DELTA_ENTRY, STATE_ENTRY, branchSnapshot, latestSnapshot, stateChange } from "./state.ts";
import { changes, sequence } from "./delta-sequence-fixture.ts";
import { deltaCount, entry, initial, statusSizes, taskAt, taskCount } from "./delta-fixture.ts";
import { describe, expect, it } from "vitest";
import { first, second } from "./contract-fixture.ts";
import { referenceLatest, referenceSnapshot } from "./snapshot-reference-fixture.ts";
import { parse } from "./wasm.ts";

const hundred = 100,
  largeCount = 1920,
  snapshotBytes = 105_235,
  deltaBytes = 674,
  version = 1,
  unsupportedVersion = 2;

describe("delta replay equivalence", () => {
  it("matches 60 changes over 192 tasks against full snapshots", { timeout: 120_000 }, () => {
    expect.hasAssertions();
    const replay = sequence(),
      comparison = replay.compare();
    expect(comparison.actual).toStrictEqual(comparison.expected);
    expect(deltaCount(replay.branch)).toBe(changes);
  });
});

describe("branch-local replay", () => {
  it("reads legacy snapshots and keeps tree branches independent", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const legacy = [
        entry("legacy", STATE_ENTRY, { tasks: [{ content: "Old", status: "in_progress" }] }),
      ],
      list = initial(hundred),
      root = entry("root", STATE_ENTRY, list),
      left = structuredClone(list),
      right = structuredClone(list);
    taskAt(left).description = "Left";
    taskAt(right, second).description = "Right";
    const leftChange = stateChange(list, left),
      rightChange = stateChange(list, right),
      leftBranch = [root, entry("left", leftChange.customType, leftChange.data)],
      rightBranch = [root, entry("right", rightChange.customType, rightChange.data)],
      replacement = [root, entry("replace", STATE_ENTRY, initial(second))];
    expect([branchSnapshot(legacy), latestSnapshot(legacy)]).toStrictEqual([
      referenceSnapshot(legacy),
      parse(referenceLatest(legacy)),
    ]);
    expect([
      latestSnapshot(leftBranch).tasks.at(first)?.description,
      latestSnapshot(leftBranch).tasks.at(second)?.description,
      latestSnapshot(rightBranch).tasks.at(first)?.description,
      latestSnapshot(rightBranch).tasks.at(second)?.description,
      latestSnapshot([root]).tasks.at(first)?.description,
    ]).toStrictEqual(["Left", undefined, undefined, "Right", undefined]);
    expect(referenceLatest(leftBranch)).toStrictEqual(list);
    expect(latestSnapshot(replacement)).toStrictEqual(initial(second));
  });
});

describe("bounded entry volume", () => {
  it("saves the same status delta for 192 and 1920 tasks", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const small = statusSizes(taskCount),
      large = statusSizes(largeCount);
    expect(small).toStrictEqual({ delta: deltaBytes, full: snapshotBytes, type: DELTA_ENTRY });
    expect(large.delta).toBe(deltaBytes);
    expect(stateChange({ tasks: [] }, { tasks: [] }).customType).toBe(STATE_ENTRY);
  });
});

describe("delta validation", () => {
  it("rejects corrupt entries without mutating stored snapshots", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const base = entry("base", STATE_ENTRY, initial(hundred)),
      before = JSON.stringify(base),
      bad = { changed: [], orders: [], removed: [], version: unsupportedVersion },
      invalidStatus = {
        changed: [
          {
            id: taskAt(initial(version)).unit?.reference.unitId,
            task: { content: "Bad", status: "lost", unit: taskAt(initial(version)).unit },
          },
        ],
        orders: [],
        removed: [],
        version,
      },
      cases = [
        null,
        bad,
        {
          changed: [],
          orders: [{ after: "absent", id: "u-initial-0", parent: null }],
          removed: [],
          version,
        },
        { changed: [], orders: [], removed: [], unexpected: true, version },
        { changed: [{ id: "wrong", task: {} }], orders: [], removed: [], version },
      ];
    for (const data of cases) {
      expect(() => latestSnapshot([base, entry("bad", DELTA_ENTRY, data)])).toThrow(
        "INVALID_TODO_DELTA",
      );
    }
    expect(() => latestSnapshot([base, entry("bad-status", DELTA_ENTRY, invalidStatus)])).toThrow(
      "unknown variant `lost`",
    );
    expect(() =>
      latestSnapshot([entry("missing-base", DELTA_ENTRY, Object.assign({}, bad, { version }))]),
    ).toThrow("no stable-ID base");
    branchSnapshot([base]);
    expect(JSON.stringify(base)).toBe(before);
  });
});
