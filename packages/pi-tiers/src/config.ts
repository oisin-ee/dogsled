import { target } from "./policy.ts";

/** Validated tier names and ordered physical targets. */
type Tiers = ReadonlyMap<string, readonly string[]>;

const ZERO = 0,
  /**
   * Validates a tier's ordered model references.
   * @param {string} name Tier name.
   * @param {unknown} models External list.
   * @returns {readonly string[]} Unique physical targets.
   */
  parseTargets = (name: string, models: unknown): readonly string[] => {
    if (
      name.length === ZERO ||
      !Array.isArray(models) ||
      models.length === ZERO ||
      !models.every((model: unknown): model is string => typeof model === "string")
    ) {
      throw new Error(`Invalid tier: ${name}`);
    }
    for (const model of models) {
      target(model);
    }
    return [...new Set(models)];
  },
  /**
   * Validates user config before registration.
   * @param {string | undefined} source User JSON.
   * @returns {Tiers} Tier definitions.
   */
  parseConfig = (source?: string): Tiers => {
    if (source === undefined) {
      return new Map();
    }
    const value: unknown = JSON.parse(source);
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw new Error("tiers.json must contain a tier-to-model map.");
    }
    const tiers = new Map<string, readonly string[]>();
    for (const [name, models] of Object.entries(value)) {
      tiers.set(name, parseTargets(name, models));
    }
    return tiers;
  };

export { parseConfig };
export type { Tiers };
