/* The generated-path gate: lockfiles and generated files change only through their tools. */
import { basename, dirname, resolve } from "node:path";
import type { HookPayload } from "../common.ts";

/** One file an edit touches. */
interface EditInput {
  readonly filePath: string;
  readonly operation: string;
}

const LOCKFILES: ReadonlySet<string> = new Set([
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
    "Cargo.lock",
    "composer.lock",
    "Gemfile.lock",
    "poetry.lock",
    "uv.lock",
    "flake.lock",
    "go.sum",
  ]),
  PACKAGE_LOCKFILES: ReadonlySet<string> = new Set([
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lockb",
  ]),
  GENERATED_PATH = new RegExp(
    [
      /\.gen\./u.source,
      /_pb2(?:_grpc)?\.py$/u.source,
      /\.pb\.go$/u.source,
      /_generated\./u.source,
      /\.g\.dart$/u.source,
      /\.freezed\.dart$/u.source,
      /(?:^|\/)(?:generated|__generated__|\.gen)\//u.source,
    ].join("|"),
    "u",
  ),
  PATCH_FILE = /^\*\*\* (?<operation>Add|Update|Delete) File: (?<path>.+)$/u,
  PATCH_MOVE = /^\*\*\* Move to: (?<path>.+)$/u,
  text = (value: unknown): string | undefined => (typeof value === "string" ? value : undefined),
  /**
   * The files an `apply_patch` body touches.
   * @param {string} command The patch text.
   * @returns {EditInput[]} One entry per file header or move.
   */
  patchInputs = (command = ""): EditInput[] => {
    const inputs: EditInput[] = [],
      lines = command.split("\n"),
      start = lines.indexOf("*** Begin Patch"),
      end = lines.indexOf("*** End Patch", start + 1);
    if (start === -1 || end === -1) {
      return inputs;
    }
    for (const line of lines.slice(start + 1, end)) {
      const file = PATCH_FILE.exec(line)?.groups;
      if (file?.["operation"] !== undefined && file["path"] !== undefined) {
        inputs.push({ filePath: file["path"], operation: file["operation"].toLowerCase() });
      }
      const move = PATCH_MOVE.exec(line)?.groups?.["path"];
      if (move !== undefined) {
        inputs.push({ filePath: move, operation: "move" });
      }
    }
    return inputs;
  },
  /**
   * The files a hook payload's edit touches.
   * @param {HookPayload} payload A hook payload.
   * @returns {EditInput[]} The files and operations.
   */
  editInputsFromPayload = (payload: HookPayload): EditInput[] => {
    const input = payload.tool_input ?? {};
    if (payload.tool_name === "apply_patch" || payload.tool_name === "patch") {
      return patchInputs(text(input["command"]) ?? "");
    }
    const filePath =
      text(input["file_path"]) ?? text(input["filePath"]) ?? text(input["path"]) ?? "";
    return filePath === "" ? [] : [{ filePath, operation: "update" }];
  },
  /**
   * Why `filePath` is generated, or null.
   * @param {string} filePath A file path.
   * @returns {string | null} The reason and the tool to use.
   */
  generatedReason = (filePath: string): string | null => {
    if (LOCKFILES.has(basename(filePath))) {
      return "lockfile. Use its package manager";
    }
    return GENERATED_PATH.test(filePath) ? "generated file. Use its code generator" : null;
  },
  // Deleting a JS lockfile together with its package.json removes the package, which no tool does.
  removesPackage = (input: EditInput, inputs: readonly EditInput[], cwd: string): boolean => {
    if (input.operation !== "delete" || !PACKAGE_LOCKFILES.has(basename(input.filePath))) {
      return false;
    }
    const manifest = resolve(dirname(resolve(cwd, input.filePath)), "package.json"),
      changes = inputs.filter((item) => resolve(cwd, item.filePath) === manifest);
    return changes.length === 1 && changes[0]?.operation === "delete";
  },
  /**
   * The block message for the first hand-edit of a generated path, or null.
   * @param {readonly EditInput[]} inputs The files an edit touches.
   * @param {string} cwd The directory relative paths resolve from.
   * @returns {string | null} The block message.
   */
  classifyEdits = (inputs: readonly EditInput[], cwd: string): string | null => {
    for (const input of inputs) {
      const reason = generatedReason(input.filePath);
      if (reason !== null && !removesPackage(input, inputs, cwd)) {
        return `generated-path: blocked hand-edit of ${input.filePath}: ${reason}.`;
      }
    }
    return null;
  };

export { classifyEdits, editInputsFromPayload, generatedReason, patchInputs };
export type { EditInput };
