/** One validated account window from Dogsled. */
interface Window {
  readonly account: string;
  readonly pool: string;
  readonly resets_ms: number | null;
  readonly used_pct: number;
}

const ZERO = 0,
  VERSION = 1,
  FULL = 100,
  /**
   * Validates the fields used by quota policy.
   * @param {unknown} value External window.
   * @returns {boolean} Whether the window is usable.
   */
  isWindow = (value: unknown): value is Window =>
    typeof value === "object" &&
    value !== null &&
    "account" in value &&
    typeof value.account === "string" &&
    "pool" in value &&
    typeof value.pool === "string" &&
    "used_pct" in value &&
    typeof value.used_pct === "number" &&
    Number.isFinite(value.used_pct) &&
    value.used_pct >= ZERO &&
    value.used_pct <= FULL &&
    "resets_ms" in value &&
    (value.resets_ms === null ||
      (typeof value.resets_ms === "number" &&
        Number.isSafeInteger(value.resets_ms) &&
        value.resets_ms >= ZERO)),
  /**
   * Rejects malformed or unsupported snapshots instead of filtering from partial data.
   * @param {string} source JSON snapshot.
   * @returns {readonly Window[]} Validated windows, or no filtering.
   */
  parseQuota = (source: string): readonly Window[] => {
    try {
      const value: unknown = JSON.parse(source);
      if (
        typeof value !== "object" ||
        value === null ||
        !("version" in value) ||
        value.version !== VERSION ||
        !("pools" in value) ||
        !Array.isArray(value.pools) ||
        !value.pools.every(isWindow)
      ) {
        return [];
      }
      return value.pools;
    } catch {
      return [];
    }
  },
  /**
   * Finds when any account in an exhausted pool first becomes available.
   * @param {readonly Window[]} windows Snapshot windows.
   * @param {string} pool Pool name.
   * @param {number} now Current time.
   * @returns {number | undefined} Pool reset, only when every account is exhausted.
   */
  exhaustedUntil = (windows: readonly Window[], pool: string, now: number): number | undefined => {
    const accounts = new Map<string, number>();
    for (const window of windows.filter((entry) => entry.pool === pool)) {
      const reset =
        window.used_pct >= FULL && window.resets_ms !== null && window.resets_ms > now
          ? window.resets_ms
          : ZERO;
      accounts.set(window.account, Math.max(accounts.get(window.account) ?? ZERO, reset));
    }
    const resets = [...accounts.values()];
    return resets.length > ZERO && resets.every((reset) => reset > now)
      ? Math.min(...resets)
      : undefined;
  };

export { exhaustedUntil, parseQuota };
export type { Window };
