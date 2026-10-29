/** A lock generation identifies its owning process and creation time. */
interface Owner {
  readonly created_ms: number;
  readonly pid: number;
}

const ZERO = 0,
  STALE_MS = 30_000,
  RETIRED_MS = 60_000,
  /**
   * Validates lock metadata before process checks or path construction.
   * @param {string} source Owner JSON.
   * @returns {Owner | undefined} Complete metadata, or an interrupted write.
   */
  parseOwner = (source: string): Owner | undefined => {
    try {
      const value: unknown = JSON.parse(source);
      if (
        typeof value !== "object" ||
        value === null ||
        !("pid" in value) ||
        !("created_ms" in value) ||
        typeof value.pid !== "number" ||
        !Number.isSafeInteger(value.pid) ||
        value.pid <= ZERO ||
        typeof value.created_ms !== "number" ||
        !Number.isSafeInteger(value.created_ms) ||
        value.created_ms < ZERO
      ) {
        return undefined;
      }
      return { created_ms: value.created_ms, pid: value.pid };
    } catch {
      return undefined;
    }
  },
  /**
   * Names a retired generation without using untrusted path fragments.
   * @param {Owner} owner Observed generation.
   * @returns {string} Tombstone directory name.
   */
  retiredName = (owner: Owner): string => `retired-${owner.pid}-${owner.created_ms}`,
  /**
   * Compares a fresh observation with a previously observed generation.
   * @param {Owner | undefined} current Fresh metadata.
   * @param {Owner} expected Original metadata.
   * @returns {boolean} Whether retirement still applies.
   */
  sameOwner = (current: Owner | undefined, expected: Owner): boolean =>
    current?.pid === expected.pid && current.created_ms === expected.created_ms;

export { parseOwner, RETIRED_MS, retiredName, sameOwner, STALE_MS };
export type { Owner };
