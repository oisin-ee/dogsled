const ZERO = 0,
  ONE = 1,
  LIMIT = 128;

/** Correlates HTTP status only when the error identifies the same response. */
class Responses {
  readonly #statuses = new Map<string, number>();

  /**
   * Records bounded response metadata, never headers or credentials.
   * @param {Readonly<Record<string, string>>} headers Response headers.
   * @param {number} status HTTP status.
   */
  public observe(headers: Readonly<Record<string, string>>, status: number): void {
    const id = headers["x-request-id"] ?? headers["request-id"];
    if (this.#statuses.size >= LIMIT) {
      this.#statuses.clear();
    }
    if (id !== undefined && id.length > ZERO) {
      this.#statuses.set(id, status);
    }
  }

  /**
   * Uses status only when a provider error explicitly includes its request ID.
   * @param {string} message Error text.
   * @returns {number | undefined} Correlated status.
   */
  public status(message: string): number | undefined {
    const tokens: readonly string[] = message.match(/[a-z0-9_-]+/giu) ?? [],
      matches = [...this.#statuses].filter(([id]: readonly [string, number]) =>
        tokens.includes(id),
      ),
      [match] = matches;
    return matches.length === ONE ? match?.[ONE] : undefined;
  }

  /** Removes metadata after settlement or session navigation. */
  public clear(): void {
    this.#statuses.clear();
  }
}

export { Responses };
