import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import type { State } from "./policy.ts";
import { VIRTUAL_MODEL_STATE_ENTRY } from "@earendil-works/pi-coding-agent";

const ZERO = 0,
  RECOVERY = "pi-tiers.recovery",
  /**
   * Validates branch data before policy uses it.
   * @param {unknown} value Stored state.
   * @returns {boolean} Valid state.
   */
  isState = (value: unknown): value is State =>
    typeof value === "object" &&
    value !== null &&
    "chosen" in value &&
    typeof value.chosen === "string" &&
    "budget" in value &&
    typeof value.budget === "number" &&
    Number.isSafeInteger(value.budget) &&
    value.budget >= ZERO &&
    "attempted" in value &&
    Array.isArray(value.attempted) &&
    value.attempted.every((item: unknown) => typeof item === "string"),
  /**
   * Reads state for one tier, including an unconsumed recovery marker.
   * A subsequent Pi state entry consumes the marker without editing history.
   * @param {readonly SessionEntry[]} branch Active branch.
   * @param {string} tier Virtual model ID.
   * @returns {State | undefined} Latest branch-owned state.
   */
  branchState = (branch: readonly SessionEntry[], tier: string): State | undefined => {
    for (const entry of branch.toReversed()) {
      if (
        entry.type === "custom" &&
        (entry.customType === VIRTUAL_MODEL_STATE_ENTRY || entry.customType === RECOVERY)
      ) {
        const data: unknown = entry.data;
        if (
          typeof data === "object" &&
          data !== null &&
          "provider" in data &&
          data.provider === "tier" &&
          "modelId" in data &&
          data.modelId === tier &&
          "state" in data &&
          isState(data.state)
        ) {
          return data.state;
        }
      }
    }
    return undefined;
  },
  /**
   * Detects a real user entry after the latest route or recovery marker.
   * Pi can label recovery as user after the failed assistant is omitted.
   * @param {readonly SessionEntry[]} branch Active branch.
   * @returns {boolean} Whether a fresh user turn started.
   */
  newUserTurn = (branch: readonly SessionEntry[]): boolean => {
    for (const entry of branch.toReversed()) {
      if (entry.type === "message" && entry.message.role === "user") {
        return true;
      }
      if (
        entry.type === "custom" &&
        (entry.customType === RECOVERY || entry.customType === VIRTUAL_MODEL_STATE_ENTRY)
      ) {
        return false;
      }
    }
    return false;
  };

export { branchState, newUserTurn, RECOVERY };
