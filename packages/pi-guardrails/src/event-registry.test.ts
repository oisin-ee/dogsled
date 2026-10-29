import assert from "node:assert/strict";
import { test } from "vitest";

import { runEvent } from "./event-registry.ts";

function shellRuntime() {
  const errors: string[] = [];
  return {
    errors,
    runtime: {
      cwd: "/repo",
      stderr: (message: string) => {
        errors.push(message);
      },
    },
  };
}

test("stop ignores prose and never queries CI or requests continuation", () => {
  const { errors, runtime } = shellRuntime(),
    result = runEvent(runtime, {
      cwd: "/repo",
      hook_event_name: "Stop",
      last_assistant_message: "I will wait for GitHub Actions before continuing.",
    });

  assert.deepEqual(result, { code: 0 });
  assert.deepEqual(errors, []);
});

test("stop permits a CI-pending continuation statement", () => {
  const result = runEvent(shellRuntime().runtime, {
    cwd: "/repo",
    hook_event_name: "Stop",
    last_assistant_message: "CI is pending, so I am continuing child work.",
  });

  assert.deepEqual(result, { code: 0 });
});

test("preToolUse blocks a combined stash push and pop shell command", () => {
  const { errors, runtime } = shellRuntime(),
    result = runEvent(runtime, {
      cwd: "/repo",
      hook_event_name: "PreToolUse",
      tool_input: { command: "cd /repo && git stash push -- src && git status && git stash pop" },
      tool_name: "Bash",
    });

  assert.deepEqual(result, { code: 2 });
  assert.match(errors.join(""), /^git-gate:.*stash push and git stash pop/u);
});

test("preToolUse allows a non-combined stash call without an advisory", () => {
  const { errors, runtime } = shellRuntime(),
    result = runEvent(runtime, {
      cwd: "/repo",
      hook_event_name: "PreToolUse",
      tool_input: { command: "git stash push -- src" },
      tool_name: "Bash",
    });

  assert.deepEqual(result, { code: 0 });
  assert.deepEqual(errors, []);
});

test("preToolUse rejects invalid shell syntax instead of a fail-open result", () => {
  const { errors, runtime } = shellRuntime(),
    result = runEvent(runtime, {
      hook_event_name: "PreToolUse",
      tool_input: { command: "echo 'unterminated" },
      tool_name: "Bash",
    });
  assert.deepEqual(result, { code: 2 });
  assert.match(errors.join(""), /^shell-parser:/u);
});

test("edit checks protect generated paths without code-quality keyword gates", () => {
  const { runtime } = shellRuntime();
  for (const [filePath, code] of [
    ["src/config.ts", 0],
    ["src/generated/api.ts", 2],
    ["pnpm-lock.yaml", 2],
  ] as [string, number][]) {
    const result = runEvent(runtime, {
      hook_event_name: "PreToolUse",
      tool_input: {
        command: `*** Begin Patch\n*** Update File: ${filePath}\n+const value = {} satisfies Config;\n*** End Patch`,
      },
      tool_name: "apply_patch",
    });
    assert.deepEqual(result, { code }, filePath);
  }
});

test("package-manager choice is not an authority gate", () => {
  const { runtime } = shellRuntime();
  for (const command of [
    "npm --version",
    "bun test",
    "pnpm install",
    "npx vitest",
    "yarn install",
  ]) {
    assert.deepEqual(
      runEvent(runtime, {
        hook_event_name: "PreToolUse",
        tool_input: { command },
        tool_name: "Bash",
      }),
      { code: 0 },
      command,
    );
  }
});

test("generated gate permits only an atomic package manifest and JS lock deletion", () => {
  const { runtime } = shellRuntime(),
    cases: [string, number][] = [
      ["*** Delete File: retired/pnpm-lock.yaml\n*** Delete File: retired/package.json", 0],
      ["*** Delete File: retired/package.json\n*** Delete File: retired/package-lock.json", 0],
      ["*** Delete File: retired/pnpm-lock.yaml", 2],
      ["*** Update File: retired/pnpm-lock.yaml\n*** Delete File: retired/package.json", 2],
      ["*** Add File: retired/pnpm-lock.yaml\n*** Delete File: retired/package.json", 2],
      ["*** Delete File: retired/pnpm-lock.yaml\n*** Delete File: other/package.json", 2],
      ["*** Delete File: retired/Cargo.lock\n*** Delete File: retired/package.json", 2],
      [
        "*** Delete File: retired/pnpm-lock.yaml\n*** End Patch\n*** Delete File: retired/package.json",
        2,
      ],
      [
        "*** Delete File: retired/pnpm-lock.yaml\n*** Delete File: retired/package.json\n" +
          "*** Add File: retired/package.json",
        2,
      ],
      [
        "*** Delete File: retired/pnpm-lock.yaml\n*** Delete File: retired/package.json\n" +
          "*** Update File: new/package.json\n*** Move to: retired/package.json",
        2,
      ],
    ];
  for (const [body, code] of cases) {
    assert.deepEqual(
      runEvent(runtime, {
        cwd: "/repo",
        hook_event_name: "PreToolUse",
        tool_input: { command: `*** Begin Patch\n${body}\n*** End Patch` },
        tool_name: "apply_patch",
      }),
      { code },
      body,
    );
  }
});
