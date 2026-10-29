/*
 * Shell text to calls, through `shfmt --to-json`. A command that shfmt cannot
 * parse throws, so a hook fails closed instead of guessing at the calls.
 */
import { spawnSync } from "node:child_process";

/** One argument list of a simple command, after quote removal. */
type Words = readonly string[];
/** The stages of the pipeline that must succeed before a call runs. */
type Guard = readonly Words[] | null;

const isNode = (value: unknown): value is ShNode => typeof value === "object" && value !== null;

/** The subset of shfmt's syntax tree that the parser reads. */
interface ShNode {
  readonly Type?: string;
  readonly Value?: string;
  readonly Dollar?: boolean;
  readonly Parts?: readonly ShNode[];
  readonly Pos?: { readonly Offset: number };
  readonly End?: { readonly Offset: number };
  readonly Assigns?: readonly ShNode[];
  readonly Name?: ShNode;
  readonly Args?: readonly ShNode[];
  readonly Negated?: boolean;
  readonly Background?: boolean;
  readonly Redirs?: readonly unknown[];
  readonly Cmd?: ShNode;
  readonly Op?: string;
  readonly X?: ShNode;
  readonly Y?: ShNode;
}

interface Parsed {
  readonly tree: unknown;
  readonly calls: readonly Words[];
  readonly guards: readonly Guard[];
}

const MAX_BUFFER = 16 * 1024 * 1024,
  // A loaded host (load average ~40) exceeded 5s and blocked valid commands.
  PARSE_TIMEOUT_MS = 15_000,
  cache: { command?: string; parsed?: Parsed } = {},
  literalPart = (part: ShNode, source: Buffer, doubleQuoted = false): string => {
    if (part.Type === "Lit") {
      const value = part.Value ?? "";
      return doubleQuoted
        ? value.replaceAll(/\\([\\$`"\n])/gu, (_match, char: string) => (char === "\n" ? "" : char))
        : value.replaceAll(/\\(.)/gsu, "$1");
    }
    if (part.Type === "SglQuoted" && part.Dollar !== true) {
      return part.Value ?? "";
    }
    if (part.Type === "DblQuoted") {
      return (part.Parts ?? []).map((item) => literalPart(item, source, true)).join("");
    }
    return source.subarray(part.Pos?.Offset ?? 0, part.End?.Offset ?? 0).toString();
  },
  word = (value: ShNode | undefined, source: Buffer): string =>
    (value?.Parts ?? []).map((part) => literalPart(part, source)).join(""),
  /*
   * An assignment's `Value` is a word node, while a literal's `Value` is its
   * text, so the field is read by shape.
   */
  assignValue = (assign: ShNode): ShNode | undefined => {
    const value: unknown = (assign as Record<string, unknown>)["Value"];
    return isNode(value) ? value : undefined;
  },
  callWords = (node: ShNode, source: Buffer): string[] => [
    ...(node.Assigns ?? []).map(
      (assign) => `${assign.Name?.Value ?? ""}=${word(assignValue(assign), source)}`,
    ),
    ...(node.Args ?? []).map((arg) => word(arg, source)),
  ],
  /*
   * The calls of a pipeline statement in stage order. A negated, backgrounded,
   * or redirected statement, or a compound command, has no plain pipeline: its
   * exit status or input no longer comes from its stages alone.
   */
  pipelineStages = (stmt: ShNode | undefined, source: Buffer): string[][] | null => {
    if (
      stmt === undefined ||
      stmt.Negated === true ||
      stmt.Background === true ||
      (stmt.Redirs ?? []).length > 0
    ) {
      return null;
    }
    const cmd = stmt.Cmd;
    if (cmd?.Type === "CallExpr") {
      return [callWords(cmd, source)];
    }
    if (cmd?.Type !== "BinaryCmd") {
      return null;
    }
    const op = cmd.Op;
    // `&&` is left-associative, so its right operand is the last pipeline that ran before a later `&&` operand.
    if (op === "&&") {
      return pipelineStages(cmd.Y, source);
    }
    if (op !== "|") {
      return null;
    }
    const left = pipelineStages(cmd.X, source),
      right = pipelineStages(cmd.Y, source);
    return left && right ? [...left, ...right] : null;
  },
  parse = (input: string): Parsed => {
    const result = spawnSync("shfmt", ["--to-json", "-ln=bash"], {
      encoding: "utf8",
      input,
      maxBuffer: MAX_BUFFER,
      timeout: PARSE_TIMEOUT_MS,
    });
    if (result.error !== undefined || result.status !== 0) {
      const detail = result.error?.message ?? result.stderr.trim();
      throw new Error(
        `shell-parser: cannot classify this command: ${detail}. ` +
          "If shfmt is unavailable, restore managed tools with mise bootstrap --yes.",
      );
    }
    const tree: unknown = JSON.parse(result.stdout),
      source = Buffer.from(input),
      calls: Words[] = [],
      guards: Guard[] = [],
      guardOf = new Map<ShNode, Guard>(),
      visit = (node: unknown): void => {
        if (Array.isArray(node)) {
          for (const item of node) {
            visit(item);
          }
          return;
        }
        if (!isNode(node)) {
          return;
        }
        if (node.Type === "BinaryCmd" && node.Op === "&&" && node.Y?.Cmd?.Type === "CallExpr") {
          guardOf.set(node.Y.Cmd, pipelineStages(node.X, source));
        }
        if (node.Type === "CallExpr") {
          calls.push(callWords(node, source));
          guards.push(guardOf.get(node) ?? null);
        }
        for (const value of Object.values(node)) {
          visit(value);
        }
      };
    visit(tree);
    return { tree, calls, guards };
  },
  parsedOf = (command: string): Parsed => {
    if (cache.command !== command || cache.parsed === undefined) {
      cache.parsed = parse(command);
      cache.command = command;
    }
    return cache.parsed;
  },
  /**
   * The simple commands in `command`, in source order, as fresh copies.
   * @param {string} command Shell text.
   * @returns {string[][]} One word list per call.
   */
  shellCalls = (command: string): string[][] => parsedOf(command).calls.map((words) => [...words]),
  /**
   * For each call, in shellCalls order: the stages of the pipeline that must
   * succeed before the call runs, when the call is the direct right operand of
   * `&&`. Otherwise null.
   * @param {string} command Shell text.
   * @returns {(string[][] | null)[]} One guard per call.
   */
  shellCallGuards = (command: string): (string[][] | null)[] =>
    parsedOf(command).guards.map((stages) => stages?.map((words) => [...words]) ?? null);

/**
 * Whether a literal contains a glob character that the shell would expand. An
 * escaped character is literal, so it does not expand.
 * @param {string} value The raw literal text.
 * @returns {boolean} True for an active glob character.
 */
function hasActiveGlob(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const character = value[index] ?? "";
    if (!"*?[]{}".includes(character)) {
      continue;
    }
    let backslashes = 0;
    for (let before = index - 1; before >= 0 && value[before] === "\\"; before -= 1) {
      backslashes += 1;
    }
    if (backslashes % 2 === 0) {
      return true;
    }
  }
  return false;
}

/**
 * Returns literal calls only when the complete AST has read-only shell syntax.
 * @param {string} command Shell text.
 * @returns {string[][] | null} Literal argv, or null for unsupported syntax.
 */
function literalShellCalls(command: string): string[][] | null {
  const parsed = parsedOf(command),
    allowed = new Set([
      "File",
      "Stmt",
      "CallExpr",
      "Word",
      "Lit",
      "SglQuoted",
      "DblQuoted",
      "BinaryCmd",
    ]),
    safe = (node: unknown, quoted = false): boolean => {
      if (Array.isArray(node)) {
        return node.every((item: unknown) => safe(item, quoted));
      }
      if (!isNode(node)) {
        return true;
      }
      if (node.Type !== undefined && !allowed.has(node.Type)) {
        return false;
      }
      if (
        node.Dollar === true ||
        node.Background === true ||
        (node.Redirs ?? []).length > 0 ||
        (node.Assigns ?? []).length > 0 ||
        (node.Type === "CallExpr" && (node.Args ?? []).length === 0) ||
        (node.Type === "BinaryCmd" && !["&&", "||", "|"].includes(node.Op ?? "")) ||
        (node.Type === "Lit" && !quoted && hasActiveGlob(node.Value ?? "")) ||
        (node.Type === "Lit" && !quoted && (node.Value ?? "").startsWith("~"))
      ) {
        return false;
      }
      return Object.values(node).every((value: unknown) =>
        safe(value, quoted || node.Type === "DblQuoted"),
      );
    };
  return safe(parsed.tree) && parsed.calls.length > 0
    ? parsed.calls.map((words) => [...words])
    : null;
}

export { literalShellCalls, shellCallGuards, shellCalls };
