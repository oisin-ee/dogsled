/** Failure classes control recovery, not provider retry settings. */
type Failure = "gateway" | "quota" | "transient" | "request";
/** Durable router state on one session branch. */
interface State {
  readonly attempted: readonly string[];
  readonly budget: number;
  readonly chosen: string;
}
/** Parsed physical model reference. */
interface Target {
  readonly id: string;
  readonly pool: string;
  readonly provider: string;
}

const ZERO = 0,
  ONE = 1,
  INTERNAL = 500,
  BAD_GATEWAY = 502,
  GATEWAY_TIMEOUT = 504,
  UNAVAILABLE = 503,
  BAD_REQUEST = 400,
  UNAUTHORIZED = 401,
  FORBIDDEN = 403,
  BUDGET = 2,
  QUOTA_STATUS = 429,
  GATEWAY_STATUSES = new Set([BAD_GATEWAY, UNAVAILABLE]),
  REQUEST_STATUSES = new Set([BAD_REQUEST, UNAUTHORIZED, FORBIDDEN]),
  /**
   * Classifies exactly once, with permanent failures taking precedence.
   * @param {string} message Provider error text.
   * @param {number | undefined} status Correlated HTTP status.
   * @returns {Failure} Recovery class.
   */
  classify = (message: string, status?: number): Failure => {
    if (
      REQUEST_STATUSES.has(status ?? ZERO) ||
      /unknown model|\b(?:status|code|HTTP)\s*:?\s*(?:400|401|403)\b|unauthorized|forbidden/iu.test(
        message,
      )
    ) {
      return "request";
    }
    if (
      GATEWAY_STATUSES.has(status ?? ZERO) ||
      /\b(?:status|code|HTTP)\s*:?\s*(?:502|503)\b|no available server|bad gateway|unable to connect/iu.test(
        message,
      )
    ) {
      return "gateway";
    }
    if (
      status === QUOTA_STATUS ||
      /\b(?:status|code|HTTP)\s*:?\s*429\b|usage.limit|rate_limit|insufficient_quota|quota exceeded/iu.test(
        message,
      )
    ) {
      return "quota";
    }
    // A provider-side failure of one target (HTTP 500/504, an empty upstream stream) is not
    // It is not the request's fault, so the next target in the tier may succeed.
    if (
      status === INTERNAL ||
      status === GATEWAY_TIMEOUT ||
      /\b(?:status|code|HTTP)\s*:?\s*(?:500|504)\b|internal_server_error|server_error|empty_stream/iu.test(
        message,
      )
    ) {
      return "transient";
    }
    return /overloaded|timeout|timed out|websocket.*1006|server_busy|retry after/iu.test(message)
      ? "transient"
      : "request";
  },
  /**
   * Parses a provider-qualified model and assigns its quota pool.
   * @param {string} reference Model reference.
   * @returns {Target} Physical target.
   */
  target = (reference: string): Target => {
    const separator = reference.indexOf("/"),
      provider = reference.slice(ZERO, separator),
      id = reference.slice(separator + ONE);
    if (separator < ONE || id.length === ZERO) {
      throw new Error(`Invalid tier target: ${reference}`);
    }
    const pools = [
        ["claude-", "claude"],
        ["gpt-", "codex"],
        ["minimax-", "minimax"],
      ],
      pool =
        pools.find(([prefix]: readonly string[]) => id.toLowerCase().startsWith(prefix ?? ""))?.[
          ONE
        ] ?? provider;
    return { id, pool, provider };
  };

export { BUDGET, classify, target };
export type { Failure, State, Target };
