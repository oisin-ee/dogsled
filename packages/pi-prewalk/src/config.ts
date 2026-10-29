import {
  CURRENT_SESSION_VERSION,
  SessionManager,
  getAgentDir,
} from "@earendil-works/pi-coding-agent";
import type { ConfigReport } from "./wasm.ts";
import type { Context } from "./ports.ts";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { configure } from "./wasm.ts";
import path from "node:path";
import { readFileSync } from "node:fs";

/**
 * Reads optional config, preserving permission and I/O errors.
 * @param {string} file Config file path.
 * @returns {string | undefined} Source, or undefined for a missing file.
 */
const source = (file: string): string | undefined => {
    try {
      return readFileSync(file, "utf8");
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "ENOENT") {
        return undefined;
      }
      throw error;
    }
  },
  /**
   * Reads live config. Untrusted project files never reach Rust.
   * @param {Context} ctx Current context.
   * @returns {ConfigReport} Validated config and errors.
   */
  readConfig = (ctx: Context): ConfigReport => {
    try {
      const userPath = path.join(getAgentDir(), "prewalk.json"),
        projectPath = path.join(ctx.cwd, ".pi", "prewalk.json"),
        user = source(userPath),
        project = ctx.isProjectTrusted() ? source(projectPath) : undefined;
      return configure(user, project);
    } catch (error) {
      return {
        config: { enabled: false, target: undefined },
        errors: [`prewalk.json: ${error instanceof Error ? error.message : String(error)}`],
      };
    }
  },
  /**
   * Opens only current-version sessions, preventing migration writes.
   * @param {string} file Outgoing session path supplied by pi.
   * @returns {SessionEntry[]} Previous active branch.
   */
  previousBranch = (file: string): SessionEntry[] => {
    const [line] = readFileSync(file, "utf8").split("\n"),
      first: unknown = JSON.parse(line ?? "null");
    if (
      typeof first !== "object" ||
      first === null ||
      !("version" in first) ||
      first.version !== CURRENT_SESSION_VERSION
    ) {
      throw new Error(
        "Previous session is not a current-version pi session. Refusing a migration during restore.",
      );
    }
    return SessionManager.open(file).getBranch();
  };

export { previousBranch, readConfig };
