// Shell-like tools have no default timeout, so an omitted or oversized value can hold a session for hours.
// Each entry names the argument and its unit, because the tools disagree: pi's bash takes seconds, the
// Others take milliseconds.
/** The argument that holds a tool's timeout, and its unit. */
interface TimeoutSpec {
  readonly field: string;
  readonly unit: "s" | "ms";
  readonly perSecond: number;
}

const TIMEOUT_FIELDS: ReadonlyMap<string, TimeoutSpec> = new Map<string, TimeoutSpec>([
    ["bash", { field: "timeout", perSecond: 1, unit: "s" }],
    ["hypa_shell", { field: "timeoutMs", perSecond: 1000, unit: "ms" }],
    ["ctx_execute", { field: "timeout", perSecond: 1000, unit: "ms" }],
    ["ctx_execute_file", { field: "timeout", perSecond: 1000, unit: "ms" }],
    ["ctx_batch_execute", { field: "timeout", perSecond: 1000, unit: "ms" }],
  ]),
  // Sleeps and timer waits written in sandbox code. Shell text goes through the shell wait gate instead.
  CODE_WAITS = [
    /\btime\.sleep\s*\(/u,
    /\basyncio\.sleep\s*\(/u,
    /\bsetTimeout\s*\(/u,
    /\bsetInterval\s*\(/u,
    /\bAtomics\.wait\s*\(/u,
    /\btimers\/promises\b/u,
    /\bBun\.sleep(Sync)?\s*\(/u,
    /\btime\.Sleep\s*\(/u,
    /\bthread::sleep\s*\(/u,
    /\b(?:Kernel\.)?sleep\s*\(?\s*\d/u,
    /\busleep\s*\(/u,
  ],
  CODE_TOOLS: ReadonlySet<string> = new Set(["ctx_execute", "ctx_execute_file"]);

export function codeWaitViolation(
  toolName: string,
  input: Readonly<Record<string, unknown>>,
): string | null {
  const { code } = input;
  if (!CODE_TOOLS.has(toolName) || input["language"] === "shell" || typeof code !== "string") {
    return null;
  }
  return CODE_WAITS.some((pattern) => pattern.test(code))
    ? "wait-gate: blocked a sleep or timer wait in sandbox code. Never hold a tool call to wait. Start long work with " +
        "Dogsled `run` (mcp__dogsled__run) and continue other work. Use `read` (mcp__dogsled__read) for output " +
        "and `stop` (mcp__dogsled__stop) to cancel the job."
    : null;
}

export const MAX_TIMEOUT_SECONDS = 600;
export const DEFAULT_TIMEOUT_SECONDS = 120;

function limitMessage(
  toolName: string,
  { field, unit, perSecond }: TimeoutSpec,
  value: number,
): string {
  const max = MAX_TIMEOUT_SECONDS * perSecond,
    unitHint =
      unit === "s" && value >= 1000
        ? ` ${value} looks like milliseconds; ${toolName} takes seconds.`
        : "";
  return (
    `timeout-gate: blocked ${toolName} ${field}=${value}. The ${field} unit is ${unit === "s" ? "seconds" : "milliseconds"} ` +
    `and the limit is ${max} (${MAX_TIMEOUT_SECONDS / 60} minutes).${unitHint} ` +
    "For a longer job, start it with Dogsled `run` (mcp__dogsled__run) and continue other work. " +
    "Use `read` (mcp__dogsled__read) for output and `stop` (mcp__dogsled__stop) to cancel the job. " +
    "For CI, use the ci_find and ci_watch tools."
  );
}

// Fills a missing timeout in place, as pi's tool_call contract allows, and returns a block message for an
// Invalid or oversized one.
export function enforceTimeout(toolName: string, input: Record<string, unknown>): string | null {
  const spec = TIMEOUT_FIELDS.get(toolName);
  if (spec === undefined) {
    return null;
  }
  const value = input[spec.field];
  if (value === undefined || value === null) {
    input[spec.field] = DEFAULT_TIMEOUT_SECONDS * spec.perSecond;
    return null;
  }
  const number = typeof value === "string" && value.trim() !== "" ? Number(value) : value;
  if (typeof number !== "number" || !Number.isFinite(number) || number <= 0) {
    return (
      `timeout-gate: blocked ${toolName} ${spec.field}=${JSON.stringify(value)}. Give a positive number of ` +
      `${spec.unit === "s" ? "seconds" : "milliseconds"}, or omit it for the ${DEFAULT_TIMEOUT_SECONDS}-second default.`
    );
  }
  return number > MAX_TIMEOUT_SECONDS * spec.perSecond
    ? limitMessage(toolName, spec, number)
    : null;
}
