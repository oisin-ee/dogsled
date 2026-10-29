import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { createBashTool } from "@earendil-works/pi-coding-agent";
import { tmpdir } from "node:os";
import { worker } from "./provider-fixture.ts";

/**
 * Runs Git with argument boundaries and explicit exit failure.
 * @param {string} cwd Temporary owned checkout.
 * @returns {Promise<void>} Command completion.
 */
const repository = async (cwd: string): Promise<void> => {
    await createBashTool(cwd).execute("repository", {
      command: "git init && git remote add origin git@github.com:oisin-ee/oisinpi.git",
    });
  },
  /**
   * Makes an owned repository and a seed from the real provider, without network I/O.
   * @param {string} repositoryId Logical identity authored by the lead.
   * @returns {Promise<object>} Temporary checkout and exact claim.
   */
  importFixture = async (
    repositoryId = "github.com/oisin-ee/oisinpi",
  ): Promise<{
    cwd: string;
    seedPath: string;
    origin: Awaited<ReturnType<typeof worker>>;
    removeSeed: () => Promise<void>;
    close: () => Promise<void>;
  }> => {
    const cwd = await mkdtemp(`${tmpdir()}/todo-import-`),
      seedPath = `${cwd}/seed.json`,
      origin = await worker(repositoryId);
    try {
      await repository(cwd);
      await writeFile(
        seedPath,
        JSON.stringify({
          binding: origin.claimed.binding,
          execution: origin.claimed.binding.execution,
          exported: origin.claimed.exported,
          version: 1,
        }),
      );
      return {
        close: async (): Promise<void> => {
          origin.test.session.dispose();
          await rm(cwd, { force: true, recursive: true });
        },
        cwd,
        origin,
        removeSeed: async (): Promise<void> => {
          await rm(seedPath);
        },
        seedPath,
      };
    } catch (error: unknown) {
      origin.test.session.dispose();
      await rm(cwd, { force: true, recursive: true });
      throw error;
    }
  };

export { importFixture };
