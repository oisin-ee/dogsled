import type { Command } from "../common.ts";
import { isRecord } from "../common.ts";

type CallKind = "argv" | "shell" | "python" | "literals";
interface EscapeCall {
  readonly kind: CallKind;
  readonly pattern: RegExp;
}

export const SHELL_TOOL_NAMES: ReadonlySet<string> = new Set([
  "bash",
  "ctx_batch_execute",
  "ctx_execute",
  "ctx_execute_file",
  "hypa_shell",
  "shell",
]);

export const EDIT_TOOL_NAMES: ReadonlySet<string> = new Set(["edit", "write"]);

const DIRECT_SHELL_TOOLS: ReadonlySet<string> = new Set(["bash", "hypa_shell", "shell"]),
  SHELL_CODE_TOOLS: ReadonlySet<string> = new Set(["ctx_execute", "ctx_execute_file"]),
  SHELL_BATCH_TOOLS: ReadonlySet<string> = new Set(["ctx_batch_execute"]),
  // Keep this adapter self-contained: context-mode's equivalent extractor is an unexported package internal, while
  // shell parsers do not recognize process-spawn calls embedded in JavaScript, Python, Ruby, Go, PHP, or Rust.
  // `shell` calls pass one string to a shell. `argv` calls pass an argument list that no shell parses, so the
  // list is classified word by word. Parsing it as shell text fails on ordinary values such as Markdown bodies.
  JS_CALLS: readonly EscapeCall[] = [
    { kind: "argv", pattern: /\b(?:execFileSync|execFile|spawnSync|spawn)\s*\(/gu },
    { kind: "shell", pattern: /\b(?:execSync|exec)\s*\(/gu },
  ],
  SHELL_ESCAPE_CALLS: Readonly<Record<string, readonly EscapeCall[]>> = {
    go: [{ kind: "literals", pattern: /\bexec\.Command\s*\(/gu }],
    javascript: JS_CALLS,
    php: [{ kind: "shell", pattern: /\b(?:exec|passthru|proc_open|shell_exec|system)\s*\(/gu }],
    python: [
      { kind: "shell", pattern: /\bos\.system\s*\(/gu },
      {
        kind: "python",
        pattern: /\bsubprocess\.(?:Popen|call|check_call|check_output|run)\s*\(/gu,
      },
    ],
    ruby: [{ kind: "shell", pattern: /\b(?:exec|spawn|system)\s*\(/gu }],
    rust: [{ kind: "literals", pattern: /\bCommand::new\s*\(/gu }],
    typescript: JS_CALLS,
  };

// The context-mode pi package registers `ctx_execute` under its bare name. pi's native MCP client names tools
// `mcp__<server>__<tool>`, so the same tool from a configured MCP server arrives as `mcp__context_mode__ctx_execute`.
export function canonicalToolName(name: string): string {
  if (!name.startsWith("mcp__")) {
    return name;
  }
  for (const known of [...SHELL_TOOL_NAMES, ...EDIT_TOOL_NAMES]) {
    if (name.endsWith(`_${known}`)) {
      return known;
    }
  }
  return name;
}

const text = (value: unknown): string | undefined =>
  typeof value === "string" ? value : undefined;

function commandFromInput(input: Readonly<Record<string, unknown>>): string {
  return text(input["command"]) ?? text(input["cmd"]) ?? text(input["commandText"]) ?? "";
}

function callArguments(code: string, matchIndex: number): string {
  const opening = code.indexOf("(", matchIndex);
  if (opening === -1) {
    return "";
  }
  let depth = 0,
    escaped = false,
    quote = "";
  for (let index = opening; index < code.length; index += 1) {
    const character = code[index] ?? "";
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = "";
      }
      continue;
    }
    if (['"', "'", "`"].includes(character)) {
      quote = character;
      continue;
    }
    if (character === "(") {
      depth += 1;
    }
    if (character === ")") {
      depth -= 1;
      if (depth === 0) {
        return code.slice(opening + 1, index);
      }
    }
  }
  return "";
}

function stringLiterals(source: string): string[] {
  const values: string[] = [];
  let current = "",
    escaped = false,
    quote = "";
  for (const character of source) {
    if (!quote) {
      if (['"', "'", "`"].includes(character)) {
        quote = character;
      }
      continue;
    }
    if (escaped) {
      current += character;
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (character === quote) {
      values.push(current);
      current = "";
      quote = "";
    } else {
      current += character;
    }
  }
  return values;
}

// Split source text on commas outside quotes and brackets.
function topLevelParts(source: string): string[] {
  const parts: string[] = [];
  let current = "",
    depth = 0,
    escaped = false,
    quote = "";
  for (const character of source) {
    if (quote) {
      current += character;
      if (escaped) {
        escaped = false;
      } else if (character === "\\") {
        escaped = true;
      } else if (character === quote) {
        quote = "";
      }
      continue;
    }
    if (['"', "'", "`"].includes(character)) {
      quote = character;
    } else if ("([{".includes(character)) {
      depth += 1;
    } else if (")]}".includes(character)) {
      depth -= 1;
    } else if (character === "," && depth === 0) {
      parts.push(current.trim());
      current = "";
      continue;
    }
    current += character;
  }
  if (current.trim()) {
    parts.push(current.trim());
  }
  return parts;
}

// One argument word: a plain string literal yields its value; anything else keeps its source text.
function argumentWord(part: string): string {
  const literal = /^(["'`])([\s\S]*)\1$/u.exec(part);
  if (literal && !(literal[1] === "`" && (literal[2] ?? "").includes("${"))) {
    return stringLiterals(part)[0] ?? "";
  }
  return part;
}

function listWords(part: string | undefined): string[] | null {
  if (part === undefined || !/^\[[\s\S]*\]$/u.test(part)) {
    return null;
  }
  return topLevelParts(part.slice(1, -1)).map(argumentWord);
}

function argvCommand(args: string): Command {
  const parts = topLevelParts(args),
    shell = parts.some((part) => /\bshell\s*:\s*(?!false\b)[^,}\s]/u.test(part)),
    argv = [argumentWord(parts[0] ?? ""), ...(listWords(parts[1]) ?? [])];
  return shell ? argv.join(" ") : argv;
}

function pythonCommand(args: string): Command {
  const parts = topLevelParts(args),
    shell = parts.some((part) => /^shell\s*=\s*True\b/u.test(part)),
    argv = listWords(parts[0]);
  if (!argv) {
    return stringLiterals(parts[0] ?? "").join(" ");
  }
  return shell ? argv.join(" ") : argv;
}

function embeddedCommand(kind: CallKind, args: string): Command {
  if (kind === "argv") {
    return argvCommand(args);
  }
  if (kind === "python") {
    return pythonCommand(args);
  }
  if (kind === "literals") {
    return stringLiterals(args);
  }
  return stringLiterals(args).join(" ");
}

function embeddedShellCommands(code: unknown, language: unknown): Command[] {
  const calls = typeof language === "string" ? SHELL_ESCAPE_CALLS[language] : undefined;
  if (typeof code !== "string" || calls === undefined) {
    return [];
  }
  const commands: Command[] = [];
  for (const { kind, pattern } of calls) {
    pattern.lastIndex = 0;
    for (const match of code.matchAll(pattern)) {
      const command = embeddedCommand(kind, callArguments(code, match.index));
      if (command.length > 0 && command[0] !== "") {
        commands.push(command);
      }
    }
  }
  return commands;
}

export function commandsFromToolInput(
  toolName: string,
  input: Readonly<Record<string, unknown>>,
): Command[] {
  if (DIRECT_SHELL_TOOLS.has(toolName)) {
    return [commandFromInput(input)];
  }
  if (SHELL_CODE_TOOLS.has(toolName)) {
    if (input["language"] === "shell") {
      return [text(input["code"]) ?? ""];
    }
    return embeddedShellCommands(input["code"], input["language"]);
  }
  const batch = input["commands"];
  if (SHELL_BATCH_TOOLS.has(toolName) && Array.isArray(batch)) {
    return batch.map((entry: unknown) => (isRecord(entry) ? commandFromInput(entry) : ""));
  }
  return [];
}
