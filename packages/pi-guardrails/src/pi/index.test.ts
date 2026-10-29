import assert from "node:assert/strict";
import { test } from "vitest";

import installPiGuardrails from "./index.ts";

interface CodeInput {
  readonly code: string;
  readonly language: string;
}
type Handler = (event: unknown, context: unknown) => unknown;
interface Verdict {
  readonly block?: boolean;
  readonly reason?: string;
}

interface CallContext {
  readonly cwd: string;
  readonly model?: { readonly provider: string; readonly id: string };
}

function fakePi() {
  const handlers = new Map<string, Handler>(),
    followUps: unknown[] = [];
  return {
    followUps,
    on: (event: string, handler: Handler) => {
      handlers.set(event, handler);
      return () => {};
    },
    registeredEvents: () => [...handlers.keys()],
    trigger: async (
      event: string,
      payload: Readonly<Record<string, unknown>>,
      context: CallContext,
    ) => (await handlers.get(event)?.(payload, context)) as Verdict | undefined,
  };
}

const install = (pi: ReturnType<typeof fakePi>): void => {
  installPiGuardrails(pi as unknown as Parameters<typeof installPiGuardrails>[0]);
};

test("pi never registers a terminal handler or creates recursive follow-ups", async () => {
  const pi = fakePi();
  install(pi);
  const event = {
      messages: [{ content: "I will wait for CI before continuing.", role: "assistant" }],
    },
    context = { cwd: "/repo" };

  await pi.trigger("agent_end", event, context);
  await pi.trigger("agent_end", event, context);

  assert.deepEqual(pi.registeredEvents(), ["tool_call"]);
  assert.deepEqual(pi.followUps, []);
});

test("pi permits a terminal response that continues independent work", async () => {
  const pi = fakePi();
  install(pi);

  await pi.trigger(
    "agent_end",
    { messages: [{ content: "CI is pending, so I am continuing child work.", role: "assistant" }] },
    { cwd: "/repo" },
  );

  assert.deepEqual(pi.followUps, []);
});

test("pi blocks a combined stash push and pop shell command", async () => {
  const pi = fakePi();
  install(pi);

  const result = await pi.trigger(
    "tool_call",
    { input: { command: "cd /repo && git stash push -- src && git stash pop" }, toolName: "bash" },
    { cwd: "/repo" },
  );

  assert.equal(result?.block, true);
  assert.match(result?.reason ?? "", /^git-gate:.*stash push and git stash pop/u);
  assert.deepEqual(pi.followUps, []);
});

test("pi allows a non-combined stash call without a follow-up", async () => {
  const pi = fakePi();
  install(pi);

  const result = await pi.trigger(
    "tool_call",
    { input: { command: "git stash push -- src" }, toolName: "hypa_shell" },
    { cwd: "/repo" },
  );

  assert.equal(result, undefined);
  assert.deepEqual(pi.followUps, []);
});

test("pi applies shell gates to context-mode tools behind the MCP name prefix", async () => {
  const pi = fakePi();
  install(pi);
  for (const [input, blocked] of [
    [{ code: "git stash push -- src && git stash pop", language: "shell" }, true],
    [{ code: "ls", language: "shell" }, false],
  ] as [CodeInput, boolean][]) {
    const result = await pi.trigger(
      "tool_call",
      { input, toolName: "mcp__context_mode__ctx_execute" },
      { cwd: "/repo" },
    );
    assert.equal(result?.block ?? false, blocked, input.code);
  }
});

test("pi permits quoted search operators but blocks direct agent-browser commands", async () => {
  const pi = fakePi();
  install(pi);
  for (const [command, blocked] of [
    ["rg 'hooks|agent-browser|bun' .", false],
    ["echo ready | agent-browser snapshot -i", true],
    ["mise run agent:browser -- snapshot -i", false],
  ] as [string, boolean][]) {
    const result = await pi.trigger(
      "tool_call",
      { input: { command }, toolName: "bash" },
      { cwd: "/repo" },
    );
    assert.equal(result?.block ?? false, blocked, command);
  }
});

test("pi protects generated paths and leaves code quality to the repository", async () => {
  const pi = fakePi();
  install(pi);
  for (const [path, blocked] of [
    ["generated/api.ts", true],
    ["src/api.ts", false],
  ] as [string, boolean][]) {
    const result = await pi.trigger(
      "tool_call",
      {
        input: { edits: [{ newText: "const config = {} satisfies Config;" }], path },
        toolName: "edit",
      },
      { cwd: "/repo" },
    );
    assert.equal(result?.block ?? false, blocked, path);
  }
});

test("pi classifies argv-style spawn calls word by word instead of parsing them as shell text", async () => {
  const pi = fakePi();
  install(pi);
  const body = "Closes #472\\n\\nUses `RondoClient` and `foo(` in Markdown.";
  for (const [input, blocked] of [
    [
      {
        code: `execFileSync('gh', ['pr', 'create', '--body', \`${body}\`], { encoding: 'utf8' })`,
        language: "javascript",
      },
      false,
    ],
    [
      {
        code: `const sha = execFileSync('git', ['rev-parse', 'HEAD']); spawnSync('gh', ['pr', 'edit', '--body', body])`,
        language: "javascript",
      },
      false,
    ],
    [
      {
        code: `subprocess.run(['gh', 'issue', 'comment', '508', '--body', "a \`b(\` c"], check=True)`,
        language: "python",
      },
      false,
    ],
    [{ code: `execFileSync('git', ['add', '-A'])`, language: "javascript" }, true],
    [{ code: `spawnSync('git', ['push', '--no-verify'])`, language: "javascript" }, true],
    [{ code: `subprocess.run(['git', 'stash', 'drop'])`, language: "python" }, true],
    [{ code: `execSync('git add -A')`, language: "javascript" }, true],
    [{ code: `spawnSync('git add -A', { shell: true })`, language: "javascript" }, true],
    [{ code: `subprocess.run("git add -A", shell=True)`, language: "python" }, true],
  ] as [CodeInput, boolean][]) {
    const result = await pi.trigger(
      "tool_call",
      { input, toolName: "ctx_execute" },
      { cwd: "/repo" },
    );
    assert.equal(result?.block ?? false, blocked, `${input.code} -> ${result?.reason ?? ""}`);
  }
});

test("pi fills a missing shell timeout in each tool's own unit", async () => {
  const pi = fakePi();
  install(pi);
  for (const [toolName, input, field, expected] of [
    ["bash", { command: "ls" }, "timeout", 120],
    ["mcp__gateway__keen_bash", { command: "ls" }, "timeout", 120],
    ["hypa_shell", { command: "ls" }, "timeoutMs", 120_000],
    ["ctx_execute", { code: "print(1)", language: "python" }, "timeout", 120_000],
    [
      "ctx_batch_execute",
      { commands: [{ command: "ls", label: "ls" }], queries: ["x"] },
      "timeout",
      120_000,
    ],
  ] as [string, Record<string, unknown>, string, number][]) {
    const result = await pi.trigger("tool_call", { input, toolName }, { cwd: "/repo" });
    assert.equal(result, undefined, toolName);
    assert.equal(input[field], expected, toolName);
  }
});

test("pi blocks shell timeouts above ten minutes and names the unit", async () => {
  const pi = fakePi();
  install(pi);
  for (const [toolName, input, blocked, pattern] of [
    ["bash", { command: "pnpm test", timeout: 600 }, false],
    ["bash", { command: "pnpm test", timeout: 900 }, true, /unit is seconds/u],
    ["bash", { command: "git commit -m x", timeout: 3_600_000 }, true, /looks like milliseconds/u],
    ["bash", { command: "ls", timeout: "abc" }, true, /positive number/u],
    ["hypa_shell", { command: "ls", timeoutMs: 600_000 }, false],
    ["hypa_shell", { command: "ls", timeoutMs: 1_800_000 }, true, /unit is milliseconds/u],
    [
      "mcp__context_mode__ctx_execute",
      { code: "ls", language: "shell", timeout: 3_000_000 },
      true,
      /limit is 600000/u,
    ],
  ] as [string, Record<string, unknown>, boolean, RegExp?][]) {
    const result = await pi.trigger("tool_call", { input, toolName }, { cwd: "/repo" });
    assert.equal(result?.block ?? false, blocked, `${toolName} ${JSON.stringify(input)}`);
    if (pattern) {
      assert.match(result?.reason ?? "", pattern);
    }
  }
});

test("pi directs blocked long jobs and waits to Dogsled run, read, and stop", async () => {
  const pi = fakePi();
  install(pi);
  for (const [toolName, input] of [
    ["bash", { command: "pnpm test", timeout: 601 }],
    ["hypa_shell", { command: "pnpm test", timeoutMs: 600_001 }],
    ["ctx_execute", { code: "print(1)", language: "python", timeout: 600_001 }],
    ["ctx_execute_file", { path: "/tmp/job.log", code: "print(1)", timeout: 600_001 }],
    ["ctx_batch_execute", { commands: [], queries: ["output"], timeout: 600_001 }],
    ["ctx_execute", { code: "import time\ntime.sleep(1)", language: "python" }],
    [
      "ctx_execute_file",
      { code: "await new Promise(r => setTimeout(r, 1))", language: "javascript" },
    ],
    ["bash", { command: "sleep 1" }],
    ["ctx_execute", { code: "sleep 1", language: "shell" }],
  ] as [string, Record<string, unknown>][]) {
    const result = await pi.trigger("tool_call", { input, toolName }, { cwd: "/repo" });
    assert.equal(result?.block, true, toolName);
    assert.match(result?.reason ?? "", /Dogsled `run` \(mcp__dogsled__run\)/u);
    assert.match(result?.reason ?? "", /`read` \(mcp__dogsled__read\)/u);
    assert.match(result?.reason ?? "", /`stop` \(mcp__dogsled__stop\)/u);
    assert.doesNotMatch(result?.reason ?? "", /wrangle|Wrangle|bg_task|bg_status/u);
  }
});

test("pi allows the Dogsled job tools", async () => {
  const pi = fakePi();
  install(pi);
  for (const [toolName, input] of [
    ["mcp__dogsled__run", { cmd: ["pnpm", "test"], timeout: "20m" }],
    ["mcp__dogsled__read", { target: "job-1" }],
    ["mcp__dogsled__stop", { id: "job-1" }],
  ] as [string, Record<string, unknown>][]) {
    const result = await pi.trigger("tool_call", { input, toolName }, { cwd: "/repo" });
    assert.equal(result, undefined, toolName);
  }
});

test("pi blocks blocking CI watches and points to ci_watch", async () => {
  const pi = fakePi();
  install(pi);
  const result = await pi.trigger(
    "tool_call",
    {
      input: { command: "gh pr checks 663 --repo oisin-ee/rondo --watch", timeout: 300 },
      toolName: "bash",
    },
    { cwd: "/repo" },
  );
  assert.equal(result?.block, true);
  assert.match(result?.reason ?? "", /^wait-gate:.*ci_watch/u);
});

test("pi blocks sleeps and timer waits written in sandbox code", async () => {
  const pi = fakePi();
  install(pi);
  for (const [input, blocked] of [
    [{ code: "import time\nwhile True:\n  time.sleep(10)", language: "python" }, true],
    [{ code: "await new Promise((r) => setTimeout(r, 60000));", language: "javascript" }, true],
    [{ code: "import asyncio\nawait asyncio.sleep(5)", language: "python" }, true],
    [{ code: "sleep 30", language: "shell" }, true],
    [{ code: "print(open('/tmp/cza.log').read()[-200:])", language: "python" }, false],
  ] as [CodeInput, boolean][]) {
    const result = await pi.trigger(
      "tool_call",
      { input, toolName: "ctx_execute" },
      { cwd: "/repo" },
    );
    assert.equal(result?.block ?? false, blocked, input.code);
    if (blocked) {
      assert.match(result?.reason ?? "", /^wait-gate:/u);
    }
  }
});

test("pi refuses herdr agent prompt to another agent and allows dogsled send", async () => {
  const pi = fakePi();
  install(pi);
  const refused = await pi.trigger(
    "tool_call",
    { input: { command: "herdr agent prompt w1:p1 hi" }, toolName: "bash" },
    { cwd: "/repo" },
  );
  assert.equal(refused?.block, true);
  assert.match(refused?.reason ?? "", /^agent-typing:.*dogsled send/u);
  const allowed = await pi.trigger(
    "tool_call",
    { input: { command: "dogsled send w1:p1 --message hi" }, toolName: "bash" },
    { cwd: "/repo" },
  );
  assert.equal(allowed, undefined);
});

test("pi does not ban a planning identity from Git, markdown editing, or workflow tools", async () => {
  const pi = fakePi();
  install(pi);
  for (const model of [
    { provider: "tier", id: "plan-claude" },
    { provider: "tier", id: "plan-gpt" },
  ]) {
    const planner = { cwd: "/repo", model };
    for (const [toolName, input] of [
      ["bash", { command: "git status" }],
      ["bash", { command: "git diff" }],
      ["bash", { command: "git log --oneline" }],
      ["write", { path: "/repo/plan.md", content: "- [ ] step" }],
      ["edit", { path: "/repo/plan.md", newText: "- [ ] step" }],
      ["todo", { items: [] }],
      ["mcp__dogsled__spawn", { unit: "u-1" }],
    ] as [string, Record<string, unknown>][]) {
      const result = await pi.trigger("tool_call", { input, toolName }, planner);
      assert.equal(result, undefined, `${model.id} ${toolName}`);
    }
  }
});

test("pi applies the same safeguards to a planning identity and another model", async () => {
  const pi = fakePi();
  install(pi);
  for (const model of [
    { provider: "tier", id: "plan-claude" },
    { provider: "tier", id: "worker-gpt" },
  ]) {
    const context = { cwd: "/repo", model };
    const blocked = await pi.trigger(
      "tool_call",
      { input: { command: "git stash push -- src && git stash pop" }, toolName: "bash" },
      context,
    );
    assert.equal(blocked?.block, true, model.id);
    assert.match(blocked?.reason ?? "", /^git-gate:.*stash push and git stash pop/u);
  }
});
