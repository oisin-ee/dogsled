/** A value at one replay revision. */
interface Version<Value> {
  readonly revision: number;
  readonly value: Value;
}

const first = 0,
  step = 1,
  halves = 2,
  last = -1;

/** Retains only changed values, with stable reads of earlier frames. */
class History<Key, Value> {
  readonly #values = new Map<Key, Version<Value>[]>();

  /**
   * Records one changed value.
   * @param {Key} key Value identity.
   * @param {number} revision Replay revision.
   * @param {Value} value New value.
   */
  public set(key: Key, revision: number, value: Value): void {
    const versions = this.#values.get(key) ?? [];
    versions.push({ revision, value });
    this.#values.set(key, versions);
  }

  /**
   * Finds the value at or before one revision.
   * @param {Key} key Value identity.
   * @param {number} revision Replay revision.
   * @returns {Value | undefined} Recorded value.
   */
  public get(key: Key, revision: number): Value | undefined {
    const versions = this.#values.get(key) ?? [],
      latest = versions.at(last);
    if (latest !== undefined && latest.revision <= revision) {
      return latest.value;
    }
    let lower = first,
      upper = versions.length;
    while (lower < upper) {
      const middle = Math.floor((lower + upper) / halves);
      if ((versions.at(middle)?.revision ?? Infinity) <= revision) {
        lower = middle + step;
      } else {
        upper = middle;
      }
    }
    return lower === first ? undefined : versions.at(lower - step)?.value;
  }
}

export { History };
