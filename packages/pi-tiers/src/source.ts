import type { Change, Health } from "./health.ts";
import { RETIRED_MS, STALE_MS, parseOwner, retiredName, sameOwner } from "./lock.ts";
import { emptyHealth, parseHealth, updateHealth } from "./health.ts";
import { mkdir, open, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import type { Owner } from "./lock.ts";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import path from "node:path";
import process from "node:process";
import { setTimeout } from "node:timers/promises";

const ZERO = 0,
  LOCK_WAIT = 25,
  IO_TIMEOUT = 2000,
  /**
   * Recognizes a specific filesystem error without suppressing other failures.
   * @param {unknown} error Failure.
   * @param {string} code Expected code.
   * @returns {boolean} Match.
   */
  hasCode = (error: unknown, code: string): boolean =>
    error instanceof Error && "code" in error && error.code === code,
  /**
   * Reads optional JSON source.
   * @param {string} file Path.
   * @returns {Promise<string | undefined>} Source, absent only for missing files.
   */
  readOptional = async (file: string): Promise<string | undefined> => {
    try {
      return await readFile(file, "utf8");
    } catch (error) {
      if (hasCode(error, "ENOENT")) {
        return undefined;
      }
      throw error;
    }
  },
  /**
   * Sleeps without retaining an abort listener after completion.
   * @param {number} delay Milliseconds.
   * @param {AbortSignal | undefined} signal Cancellation.
   * @returns {Promise<void>} Completion.
   */
  wait = async (delay: number, signal?: AbortSignal): Promise<void> => {
    await setTimeout(Math.max(ZERO, delay), undefined, { signal });
  },
  /**
   * Probes process existence without treating permission failures as death.
   * @param {number} pid Recorded process.
   * @returns {boolean} Whether the process is gone.
   */
  dead = (pid: number): boolean => {
    try {
      process.kill(pid, ZERO);
      return false;
    } catch (error) {
      return hasCode(error, "ESRCH");
    }
  },
  /**
   * Reads a complete generation, ignoring missing metadata during creation.
   * @param {string} directory Lock directory.
   * @returns {Promise<Owner | undefined>} Owner.
   */
  ownerAt = async (directory: string): Promise<Owner | undefined> => {
    const source = await readOptional(path.join(directory, "owner.json"));
    return source === undefined ? undefined : parseOwner(source);
  },
  /**
   * Gives an abandoned empty directory stable metadata and a nonempty tombstone.
   * @param {string} directory Lock directory.
   * @returns {Promise<void>} Completion.
   */
  identifyAbandoned = async (directory: string): Promise<void> => {
    try {
      const details = await stat(directory);
      if (Date.now() - details.mtimeMs > STALE_MS) {
        await writeFile(
          path.join(directory, "owner.json"),
          JSON.stringify({ created_ms: Math.floor(details.mtimeMs), pid: process.pid }),
          { flag: "wx" },
        );
      }
    } catch (error) {
      if (!hasCode(error, "ENOENT") && !hasCode(error, "EEXIST")) {
        throw error;
      }
    }
  },
  /**
   * Finds a stale generation, including interrupted directory creation.
   * @param {string} directory Lock directory.
   * @returns {Promise<Owner | undefined>} Stale owner.
   */
  staleOwner = async (directory: string): Promise<Owner | undefined> => {
    const owner = await ownerAt(directory);
    if (owner === undefined) {
      await identifyAbandoned(directory);
      return undefined;
    }
    return Date.now() - owner.created_ms > STALE_MS || dead(owner.pid) ? owner : undefined;
  },
  /**
   * Retires only a stale, unchanged generation to its unique nonempty directory.
   * @param {string} directory Lock directory.
   * @returns {Promise<boolean>} Whether this waiter won retirement.
   */
  retire = async (directory: string): Promise<boolean> => {
    const owner = await staleOwner(directory);
    if (owner === undefined || !sameOwner(await ownerAt(directory), owner)) {
      return false;
    }
    try {
      await rename(directory, path.join(path.dirname(directory), retiredName(owner)));
      return true;
    } catch (error) {
      if (hasCode(error, "ENOENT") || hasCode(error, "ENOTEMPTY") || hasCode(error, "EEXIST")) {
        return false;
      }
      throw error;
    }
  },
  /**
   * Removes only retired directories outside the contention retention window.
   * @param {string} directory State directory.
   * @returns {Promise<void>} Completion.
   */
  cleanRetired = async (directory: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true });
    await Promise.all(
      entries
        .filter(
          (entry: Readonly<{ name: string; isDirectory: () => boolean }>) =>
            entry.isDirectory() && /^retired-\d+-\d+$/u.test(entry.name),
        )
        .map(async (entry: Readonly<{ name: string }>) => {
          const retired = path.join(directory, entry.name);
          try {
            const details = await stat(retired);
            if (Date.now() - details.ctimeMs > RETIRED_MS) {
              await rm(retired, { force: true, recursive: true });
            }
          } catch (error) {
            if (!hasCode(error, "ENOENT")) {
              throw error;
            }
          }
        }),
    );
  },
  /**
   * Publishes complete owner metadata after exclusive directory creation.
   * @param {string} directory Acquired directory.
   * @returns {Promise<Owner>} Published generation.
   */
  publishOwner = async (directory: string): Promise<Owner> => {
    const owner = { created_ms: Date.now(), pid: process.pid };
    await writeFile(path.join(directory, "owner.tmp"), JSON.stringify(owner), { flag: "wx" });
    await rename(path.join(directory, "owner.tmp"), path.join(directory, "owner.json"));
    await cleanRetired(path.dirname(directory));
    return owner;
  },
  /**
   * Acquires a recoverable directory lock with a bounded wait.
   * @param {string} directory Lock directory.
   * @param {number} deadline Deadline.
   * @returns {Promise<Owner>} Acquired generation.
   */
  acquire = async (directory: string, deadline: number): Promise<Owner> => {
    try {
      await mkdir(directory);
    } catch (error) {
      if (!hasCode(error, "EEXIST") || Date.now() >= deadline) {
        throw error;
      }
      await retire(directory);
      await wait(LOCK_WAIT);
      return acquire(directory, deadline);
    }
    return publishOwner(directory);
  },
  /**
   * Releases only the generation this transaction acquired.
   * @param {string} directory Lock directory.
   * @param {Owner} owner Acquired generation.
   * @returns {Promise<void>} Completion.
   */
  release = async (directory: string, owner: Owner): Promise<void> => {
    if (sameOwner(await ownerAt(directory), owner)) {
      await rm(directory, { recursive: true });
    }
  };

/** Atomic host-local circuit and cooldown persistence. */
class Store {
  readonly #directory: string;
  readonly #file: string;

  /**
   * Selects the agent state directory.
   * @param {string} directory State root.
   */
  public constructor(directory = path.join(getAgentDir(), "state", "pi-tiers")) {
    this.#directory = directory;
    this.#file = path.join(directory, "state.json");
  }

  /**
   * Reads an atomic state snapshot.
   * @returns {Promise<Health>} Current shared health.
   */
  public async read(): Promise<Health> {
    const source = await readOptional(this.#file);
    return source === undefined ? emptyHealth() : parseHealth(source);
  }

  /**
   * Writes and syncs a temporary snapshot before atomic replacement.
   * @param {Health} health New state.
   * @returns {Promise<void>} Durable replacement.
   */
  async #write(health: Health): Promise<void> {
    const temporary = path.join(this.#directory, `${crypto.randomUUID()}.tmp`),
      file = await open(temporary, "wx");
    try {
      await file.writeFile(JSON.stringify(health));
      await file.sync();
      await rename(temporary, this.#file);
    } finally {
      await file.close();
      await rm(temporary, { force: true });
    }
  }

  /**
   * Holds the lock across the entire read-modify-rename transaction.
   * @param {Change} change Failure to record.
   * @returns {Promise<void>} Durable completion.
   */
  public async change(change: Change): Promise<void> {
    await mkdir(this.#directory, { recursive: true });
    const lockPath = path.join(this.#directory, "lock"),
      lock = await acquire(lockPath, Date.now() + IO_TIMEOUT);
    try {
      await this.#write(updateHealth(await this.read(), change));
    } finally {
      await release(lockPath, lock);
    }
  }
}

/**
 * Reads the user-owned tier map.
 * @returns {Promise<string | undefined>} Config source.
 */
const readConfig = async (): Promise<string | undefined> =>
  readOptional(path.join(getAgentDir(), "tiers.json"));

export { acquire, readConfig, release, retire, Store, wait };
