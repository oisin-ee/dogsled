/** Backoff record shared by all sessions on one host. */
interface Backoff {
  readonly failures: number;
  readonly until: number;
}
/** Provider circuits and pool cooldowns have separate namespaces. */
interface Health {
  readonly circuits: Readonly<Record<string, Backoff>>;
  readonly pools: Readonly<Record<string, Backoff>>;
}
/** One atomic update to shared health. */
interface Change {
  readonly kind: "gateway" | "quota";
  readonly key: string;
  readonly now: number;
  readonly reset: number | undefined;
}

const ZERO = 0,
  ONE = 1,
  TWO = 2,
  QUOTA_DELAY = 300_000,
  QUOTA_MAX = 3_600_000,
  GATEWAY_DELAY = 2000,
  GATEWAY_MAX = 60_000,
  /**
   * Creates an empty health store.
   * @returns {Health} Healthy endpoints and pools.
   */
  emptyHealth = (): Health => ({ circuits: {}, pools: {} }),
  /**
   * Computes exponential backoff while preserving distinct failure domains.
   * @param {Health} health Current store.
   * @param {Change} change Failure observation.
   * @returns {Health} Updated store.
   */
  updateHealth = (health: Health, change: Change): Health => {
    const namespace = change.kind === "gateway" ? "circuits" : "pools",
      previous = health[namespace][change.key],
      base = change.kind === "gateway" ? GATEWAY_DELAY : QUOTA_DELAY,
      maximum = change.kind === "gateway" ? GATEWAY_MAX : QUOTA_MAX,
      failures = (previous?.failures ?? ZERO) + ONE,
      delay = Math.min(maximum, base * TWO ** (failures - ONE)),
      record = { failures, until: change.reset ?? change.now + delay };
    return Object.assign({}, health, {
      [namespace]: Object.assign({}, health[namespace], { [change.key]: record }),
    });
  },
  /**
   * Validates persisted backoff records.
   * @param {unknown} value External map.
   * @returns {boolean} Whether all records are valid.
   */
  isBackoffs = (value: unknown): value is Record<string, Backoff> =>
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Object.values(value).every(
      (entry: unknown) =>
        typeof entry === "object" &&
        entry !== null &&
        "failures" in entry &&
        typeof entry.failures === "number" &&
        Number.isSafeInteger(entry.failures) &&
        entry.failures >= ZERO &&
        "until" in entry &&
        typeof entry.until === "number" &&
        Number.isSafeInteger(entry.until) &&
        entry.until >= ZERO,
    ),
  /**
   * Rejects corrupt stores rather than silently discarding cooldowns.
   * @param {string} source Persisted JSON.
   * @returns {Health} Validated health state.
   */
  parseHealth = (source: string): Health => {
    const value: unknown = JSON.parse(source);
    if (
      typeof value !== "object" ||
      value === null ||
      !("circuits" in value) ||
      !("pools" in value) ||
      !isBackoffs(value.circuits) ||
      !isBackoffs(value.pools)
    ) {
      throw new Error("Invalid pi-tiers health state.");
    }
    return { circuits: value.circuits, pools: value.pools };
  };

export { emptyHealth, parseHealth, updateHealth };
export type { Change, Health };
