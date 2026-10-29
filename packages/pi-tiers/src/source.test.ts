import { acquire, release } from "./source.ts";
import { describe, expect, it } from "vitest";
import { mkdir, mkdtemp, readFile, readdir, rm, utimes, writeFile } from "node:fs/promises";
import type { Owner } from "./lock.ts";
import { once } from "node:events";
import path from "node:path";
import process from "node:process";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";

const DEADLINE = 100,
  STALE_AGE = 31_000,
  ONE = 1,
  /**
   * Gets a real dead PID from a completed child process.
   * @returns {Promise<number>} Exited process ID.
   */
  deadPid = async (): Promise<number> => {
    const child = spawn(process.execPath, ["-e", ""], { stdio: "ignore" }),
      { pid } = child;
    await once(child, "exit");
    if (pid === undefined) {
      throw new Error("Child did not start.");
    }
    return pid;
  },
  /**
   * Creates an isolated directory lock with explicit owner metadata.
   * @param {Owner} owner Initial generation.
   * @returns {Promise<string>} State directory.
   */
  fixture = async (owner: Owner): Promise<string> => {
    const directory = await mkdtemp(path.join(tmpdir(), "pi-tiers-lock-")),
      lock = path.join(directory, "lock");
    await mkdir(lock);
    await writeFile(path.join(lock, "owner.json"), JSON.stringify(owner));
    return directory;
  };

describe("directory lock takeover", () => {
  it("takes over a fresh lock whose process exited", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const directory = await fixture({ created_ms: Date.now(), pid: await deadPid() }),
      lock = path.join(directory, "lock");
    try {
      const owner = await acquire(lock, Date.now() + DEADLINE);
      expect(owner.pid).toBe(process.pid);
      await expect(readFile(path.join(lock, "owner.json"), "utf8")).resolves.toBe(
        JSON.stringify(owner),
      );
      const entries = await readdir(directory);
      expect(entries.filter((entry) => entry.startsWith("retired-"))).toHaveLength(ONE);
      await release(lock, owner);
    } finally {
      await rm(directory, { force: true, recursive: true });
    }
  });

  it(
    "allows exactly one concurrent acquirer to retain the stale lock",
    { timeout: 5000 },
    async () => {
      expect.hasAssertions();
      const directory = await fixture({ created_ms: Date.now(), pid: await deadPid() }),
        lock = path.join(directory, "lock");
      try {
        const deadline = Date.now() + DEADLINE,
          results = await Promise.allSettled([acquire(lock, deadline), acquire(lock, deadline)]);
        expect(
          results.filter((result: Readonly<{ status: string }>) => result.status === "fulfilled"),
        ).toHaveLength(ONE);
        expect(
          results.filter((result: Readonly<{ status: string }>) => result.status === "rejected"),
        ).toHaveLength(ONE);
        const entries = await readdir(directory);
        expect(entries.filter((entry) => entry.startsWith("retired-"))).toHaveLength(ONE);
      } finally {
        await rm(directory, { force: true, recursive: true });
      }
    },
  );
});

describe("directory lock expiry", () => {
  it("keeps a fresh live owner until the deadline", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const owner = { created_ms: Date.now(), pid: process.pid },
      directory = await fixture(owner),
      lock = path.join(directory, "lock");
    try {
      await expect(acquire(lock, Date.now() + DEADLINE)).rejects.toMatchObject({ code: "EEXIST" });
      await expect(readFile(path.join(lock, "owner.json"), "utf8")).resolves.toBe(
        JSON.stringify(owner),
      );
    } finally {
      await rm(directory, { force: true, recursive: true });
    }
  });

  it("takes over an expired ownerless directory", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const directory = await mkdtemp(path.join(tmpdir(), "pi-tiers-empty-")),
      lock = path.join(directory, "lock"),
      old = new Date(Date.now() - STALE_AGE);
    try {
      await mkdir(lock);
      await utimes(lock, old, old);
      await expect(acquire(lock, Date.now() + DEADLINE)).resolves.toMatchObject({
        pid: process.pid,
      });
      const entries = await readdir(directory);
      expect(entries.filter((entry) => entry.startsWith("retired-"))).toHaveLength(ONE);
    } finally {
      await rm(directory, { force: true, recursive: true });
    }
  });
});
