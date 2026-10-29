/*
 * The Claude Code and Codex hook command. It reads one hook payload on stdin,
 * prints the verdict's extra keys as JSON, and exits with the verdict's code:
 * 2 blocks the tool call. An internal failure is recorded and allows the call,
 * so a broken install never stops every tool.
 */
import { mkdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { isRecord } from "./common.ts";
import { runEvent } from "./event-registry.ts";
import type { HookResult } from "./common.ts";

/** A hook run's inputs; tests replace the process's. */
interface HookRun {
  readonly payload: string | Readonly<Record<string, unknown>>;
  readonly cwd?: string;
  readonly stderr?: (message: string) => void;
}

/**
 * The verdict for one raw hook payload.
 * @param {HookRun} run The payload and the process context.
 * @returns {HookResult} The verdict; a payload that is not an object allows.
 */
const runLifecycleHook = (run: HookRun): HookResult => {
    const parsed: unknown = typeof run.payload === "string" ? JSON.parse(run.payload) : run.payload;
    if (!isRecord(parsed)) {
      return { code: 0 };
    }
    return runEvent(
      {
        cwd: run.cwd ?? process.cwd(),
        stderr:
          run.stderr ??
          ((message: string): void => {
            process.stderr.write(message);
          }),
      },
      parsed,
    );
  },
  errorRoot = (): string =>
    join(
      process.env["XDG_STATE_HOME"] ?? join(process.env["HOME"] ?? homedir(), ".local", "state"),
      "agent-hooks",
      "runtime",
    ),
  readStdin = async (): Promise<string> => {
    const chunks: Buffer[] = [];
    for await (const chunk of process.stdin) {
      chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk)));
    }
    return Buffer.concat(chunks).toString("utf8");
  },
  /**
   * Runs the hook on this process's stdin and returns the exit code.
   * @returns {Promise<number>} The process exit code.
   */
  main = async (): Promise<number> => {
    const raw = await readStdin();
    if (raw.trim().length === 0) {
      return 0;
    }
    try {
      const { code, ...output } = runLifecycleHook({ payload: raw });
      if (Object.keys(output).length > 0) {
        process.stdout.write(`${JSON.stringify(output)}\n`);
      }
      return code;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error),
        root = errorRoot();
      await mkdir(root, { recursive: true });
      await writeFile(join(root, "last-error.txt"), `${message}\n`);
      return 0;
    }
  };

export { main, runLifecycleHook };
export type { HookRun };
