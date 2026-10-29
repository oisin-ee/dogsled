import assert from "node:assert/strict";
import { onTestFinished, test } from "vitest";

import {
  SHELL_MSG,
  classifyAgentBrowserIsolation,
  classifyGit,
  classifyImperativeMutation,
  classifyStashPushPop,
} from "./shell.ts";

const BLOCKED = [
    'git config user.email "foo@bar.com"',
    'git config user.name "Foo Bar"',
    'git config --global user.email "foo@bar.com"',
    "git config --add user.email foo@bar.com",
    'git -c user.email=foo@bar.com commit -m "x"',
    'git -c user.name=Foo -c core.editor=vim commit -m "x"',
    'GIT_AUTHOR_NAME="Foo" GIT_AUTHOR_EMAIL="a@b.com" git commit -m "x"',
    'GIT_COMMITTER_NAME="Foo" git commit -m "x"',
    'git commit --author="Foo <a@b.com>" -m "x"',
    "git commit --author=Foo -m x",
  ],
  ALLOWED = [
    "git config --get user.email",
    "git config --get-all user.name",
    "git config -l",
    "git config --list",
    "git config user.email",
    "git config core.editor vim",
    'git commit -m "normal commit"',
    "git status",
    "git add -- foo.txt",
    'git -c core.editor=vim commit -m "x"',
  ];

test("classifyGit blocks on-the-fly git identity changes", () => {
  for (const command of BLOCKED) {
    assert.match(classifyGit(command, "/repo") ?? "", /^git-gate:.*identity/u, command);
  }
});

test("classifyGit allows identity reads and unrelated git commands", () => {
  for (const command of ALLOWED) {
    assert.equal(classifyGit(command, "/repo"), null, command);
  }
});

test("classifyStashPushPop blocks combined stash commands across shell segments and wrappers", () => {
  const commands = [
    "cd /repo && git stash push --include-untracked && git status && git stash pop",
    "git stash push -- src | tee /tmp/stash.log && git stash pop",
    "command git stash push -- && sudo git stash pop",
    "git -C /repo stash && /usr/bin/git -C /repo stash pop",
    "git stash -q && git stash --quiet pop",
  ];

  for (const command of commands) {
    assert.equal(classifyStashPushPop(command), SHELL_MSG.stashPushPop, command);
  }
});

test("classifyStashPushPop allows separate stash calls", () => {
  for (const command of ["git stash push -- src", "git stash pop", "git stash list"]) {
    assert.equal(classifyStashPushPop(command), null, command);
  }
  assert.equal(classifyStashPushPop("git stash list && git stash pop"), null);
});

test("classifyGit blocks an unverified stash drop and every stash clear", () => {
  for (const command of [
    "git stash drop",
    "git stash drop stash@{1}",
    "git -C /repo stash drop -q",
  ]) {
    assert.equal(classifyGit(command, "/repo"), SHELL_MSG.stashDrop, command);
  }
  assert.equal(classifyGit(["git", "stash", "drop"], "/repo"), SHELL_MSG.stashDrop);
  assert.equal(classifyGit("git stash clear", "/repo"), SHELL_MSG.stashClear);
});

async function stashRepo() {
  const { execFileSync } = await import("node:child_process"),
    { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } = await import("node:fs"),
    { tmpdir } = await import("node:os"),
    repo = realpathSync(mkdtempSync(`${tmpdir()}/stash-gate-`)),
    git = (...args: string[]) =>
      execFileSync("git", ["-C", repo, "-c", "user.name=t", "-c", "user.email=t@t", ...args], {
        encoding: "utf8",
      });
  onTestFinished(() => {
    rmSync(repo, { recursive: true, force: true });
  });
  git("init", "-q");
  mkdirSync(`${repo}/sub`);
  writeFileSync(`${repo}/root.txt`, "a\n");
  writeFileSync(`${repo}/sub/nested.txt`, "b\n");
  writeFileSync(`${repo}/bin`, Buffer.from([0, 1]));
  git("add", "--", "root.txt", "sub/nested.txt", "bin");
  git("commit", "-q", "-m", "init");
  return { git, repo, writeFileSync };
}

const VERIFIED_DROP =
  "git stash show -p --binary -u stash@{0} | git apply --check -R && git stash drop stash@{0}";

/** One stash-drop scenario, with the command and working directory it needs. */
interface StashDropCase {
  /** The scenario name, shown as the test name. */
  readonly name: string;
  /** The command and the working directory the classification runs in. */
  readonly build: (repo: string, sub: string) => [command: string, cwd: string];
}

const ALLOWED_STASH_DROP_CASES: readonly StashDropCase[] = [
  {
    name: "the exact verifying pipeline at the repository root",
    build: (repo) => [VERIFIED_DROP, repo],
  },
  {
    name: "the long flag form with a numeric stash index",
    build: (repo) => [
      "git stash show --include-untracked --patch --binary 1 | git apply --reverse --check && git stash drop -q stash@{1}",
      repo,
    ],
  },
  {
    name: "the short flag form without a stash index",
    build: (repo) => [
      "git stash show -u -p --binary | git apply -R --check && git stash drop",
      repo,
    ],
  },
  {
    name: "a cd to the repository root before the pipeline",
    build: (repo, sub) => [`cd ${repo} && ${VERIFIED_DROP}`, sub],
  },
  {
    name: "git -C pointing at the repository root from a subdirectory",
    build: (_repo, sub) => [
      `git -C .. stash show -p --binary -u | git -C .. apply --check -R && git -C .. stash drop`,
      sub,
    ],
  },
  {
    name: "a status check before the pipeline",
    build: (repo) => [`git status && ${VERIFIED_DROP}`, repo],
  },
];

const BLOCKED_STASH_DROP_CASES: readonly StashDropCase[] = [
  {
    name: "the exact verifying pipeline outside the repository root",
    build: (_repo, sub) => [VERIFIED_DROP, sub],
  },
  {
    name: "a drop of a different stash than the one that was shown",
    build: (repo) => [
      "git stash show -p --binary -u stash@{0} | git apply --check -R && git stash drop stash@{1}",
      repo,
    ],
  },
  {
    name: "a show without the untracked flag",
    build: (repo) => [
      "git stash show -p --binary stash@{0} | git apply --check -R && git stash drop stash@{0}",
      repo,
    ],
  },
  {
    name: "a show without the patch flag",
    build: (repo) => [
      "git stash show -p -u stash@{0} | git apply --check -R && git stash drop stash@{0}",
      repo,
    ],
  },
  {
    name: "a show without the binary flag",
    build: (repo) => [
      "git stash show --binary -u stash@{0} | git apply --check -R && git stash drop stash@{0}",
      repo,
    ],
  },
  {
    name: "a show restricted to a path",
    build: (repo) => [
      "git stash show -p --binary -u -- root.txt | git apply --check -R && git stash drop",
      repo,
    ],
  },
  {
    name: "a show with a stray -R flag",
    build: (repo) => [
      "git stash show -p --binary -u -R | git apply --check -R && git stash drop",
      repo,
    ],
  },
  {
    name: "an apply without the check flag",
    build: (repo) => ["git stash show -p --binary -u | git apply --check && git stash drop", repo],
  },
  {
    name: "an apply without the reverse flag",
    build: (repo) => ["git stash show -p --binary -u | git apply -R && git stash drop", repo],
  },
  {
    name: "an apply with an extra exclude option",
    build: (repo) => [
      "git stash show -p --binary -u | git apply --check -R --exclude=root.txt && git stash drop",
      repo,
    ],
  },
  {
    name: "an apply with a patch file argument",
    build: (repo) => [
      "git stash show -p --binary -u | git apply --check -R other.patch && git stash drop",
      repo,
    ],
  },
  {
    name: "an apply that reads from a redirect",
    build: (repo) => [
      "git stash show -p --binary -u | git apply --check -R < other.patch && git stash drop",
      repo,
    ],
  },
  {
    name: "a pipeline that also writes to standard error",
    build: (repo) => [
      "git stash show -p --binary -u |& git apply --check -R && git stash drop",
      repo,
    ],
  },
  {
    name: "a pipeline joined with or",
    build: (repo) => [
      "git stash show -p --binary -u | git apply --check -R || git stash drop",
      repo,
    ],
  },
  {
    name: "a pipeline joined with a semicolon",
    build: (repo) => ["git stash show -p --binary -u | git apply --check -R; git stash drop", repo],
  },
  {
    name: "a negated pipeline",
    build: (repo) => [
      "! git stash show -p --binary -u | git apply --check -R && git stash drop",
      repo,
    ],
  },
  {
    name: "a pipeline behind a short circuit",
    build: (repo) => [
      "true || git stash show -p --binary -u | git apply --check -R && git stash drop",
      repo,
    ],
  },
  {
    name: "a drop inside a subshell",
    build: (repo) => [
      "git stash show -p --binary -u | git apply --check -R && (git stash drop)",
      repo,
    ],
  },
  {
    name: "a drop inside a command substitution",
    build: (repo) => [
      "git stash show -p --binary -u | git apply --check -R && echo $(git stash drop)",
      repo,
    ],
  },
  {
    name: "an extra stage in the pipeline",
    build: (repo) => [
      "git stash show -p --binary -u | cat | git apply --check -R && git stash drop",
      repo,
    ],
  },
  {
    name: "a refs/stash reference instead of a stash index",
    build: (repo) => [
      "git stash show -p --binary -u refs/stash | git apply --check -R && git stash drop refs/stash",
      repo,
    ],
  },
  {
    name: "a show that changes directory to a subdirectory",
    build: (repo) => [
      "git -C sub stash show -p --binary -u | git apply --check -R && git stash drop",
      repo,
    ],
  },
  {
    name: "a show with a GIT_DIR assignment",
    build: (repo) => [
      "GIT_DIR=x git stash show -p --binary -u | git apply --check -R && git stash drop",
      repo,
    ],
  },
];

test.each(ALLOWED_STASH_DROP_CASES)(
  "classifyGit allows a stash drop after $name",
  async ({ build }) => {
    const { repo } = await stashRepo(),
      sub = `${repo}/sub`,
      [command, cwd] = build(repo, sub);
    assert.equal(classifyGit(command, cwd), null, command);
  },
);

test.each(BLOCKED_STASH_DROP_CASES)(
  "classifyGit blocks a stash drop after $name",
  async ({ build }) => {
    const { repo } = await stashRepo(),
      sub = `${repo}/sub`,
      [command, cwd] = build(repo, sub);
    assert.equal(classifyGit(command, cwd), SHELL_MSG.stashDrop, command);
  },
);

test("the verified stash drop runs only when every stashed change is already in the working tree", async () => {
  const { execFileSync } = await import("node:child_process"),
    { unlinkSync } = await import("node:fs"),
    { git, repo, writeFileSync } = await stashRepo(),
    exitCode = () => {
      try {
        execFileSync("bash", ["-c", VERIFIED_DROP], { cwd: repo, stdio: "ignore" });
        return 0;
      } catch (error) {
        return (error as { status: number }).status;
      }
    };
  writeFileSync(`${repo}/root.txt`, "a\nchanged\n");
  writeFileSync(`${repo}/sub/nested.txt`, "b\nchanged\n");
  writeFileSync(`${repo}/bin`, Buffer.from([0, 1, 2]));
  writeFileSync(`${repo}/untracked.txt`, "new\n");
  git("stash", "push", "-q", "-u");

  assert.notEqual(exitCode(), 0);
  assert.equal(git("stash", "list").trim().split("\n").length, 1);

  git("stash", "apply", "-q");
  unlinkSync(`${repo}/untracked.txt`);
  assert.notEqual(exitCode(), 0, "a missing untracked file keeps the stash");
  assert.equal(git("stash", "list").trim().split("\n").length, 1);

  writeFileSync(`${repo}/untracked.txt`, "new\n");
  assert.equal(exitCode(), 0);
  assert.equal(git("stash", "list").trim(), "");
});

test("quoted search expressions never create commands", () => {
  for (const command of [
    "rg 'hooks|agent-browser|bun' .",
    'echo "agent-browser open https://example.com"',
    String.raw`printf '%s\n' agent-browser`,
    "mise run agent:browser -- open https://example.com",
  ]) {
    assert.equal(classifyAgentBrowserIsolation(command), null, command);
  }
});

test("real agent-browser command boundaries and substitutions retain restrictions", () => {
  for (const command of [
    "echo ready | agent-browser snapshot -i",
    "echo ready\nagent-browser snapshot -i",
    "echo ready; agent-browser snapshot -i",
    "echo $(agent-browser snapshot -i)",
    'echo "$(agent-browser snapshot -i)"',
    "if true; then agent-browser snapshot -i; fi",
    "sh -c 'agent-browser snapshot -i' ignored 'literal argument'",
  ]) {
    assert.equal(classifyAgentBrowserIsolation(command), SHELL_MSG.agentBrowserIsolation, command);
  }
  assert.ok(classifyImperativeMutation("echo ready\nkubectl delete pod x"));
  assert.equal(classifyImperativeMutation("rg 'ready|kubectl delete pod x' ."), null);
});

test("argo observation does not become a live mutation", () => {
  assert.equal(classifyImperativeMutation("argocd app wait app --health --timeout 30"), null);
  assert.ok(classifyImperativeMutation("argocd app sync app"));
  assert.ok(classifyImperativeMutation("argocd app rollback app 2"));
});

test("classifyGit resolves .worktrees/ from the main checkout, including inside a linked worktree", async () => {
  const { execFileSync } = await import("node:child_process"),
    { mkdtempSync, realpathSync } = await import("node:fs"),
    { tmpdir } = await import("node:os"),
    repo = realpathSync(mkdtempSync(`${tmpdir()}/worktree-gate-`)),
    git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { stdio: "ignore" });
  git("init", "-q");
  git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init");
  git("worktree", "add", "-q", "--detach", `${repo}/.worktrees/final`);
  const linked = `${repo}/.worktrees/final`;

  for (const [command, cwd] of [
    ["git worktree add -b diag .worktrees/diag", repo],
    [`git worktree add --detach ${repo}/.worktrees/pinch HEAD`, linked],
    ["git worktree add --detach ../mainchk HEAD", linked],
    [`cd ${linked} && git worktree add --detach ../check HEAD`, repo],
  ] as [string, string][]) {
    assert.equal(classifyGit(command, cwd), null, command);
  }
  for (const [command, cwd] of [
    ["git worktree add --detach /tmp/main-probe HEAD", linked],
    ["git worktree add ../outside", repo],
    [`cd ${linked} && git worktree add --detach ../../../outside HEAD`, repo],
  ] as [string, string][]) {
    assert.equal(classifyGit(command, cwd), SHELL_MSG.worktree, command);
  }
});

test("classifyGit relaxes staging and branch creation only inside a linked worktree", async () => {
  const { execFileSync } = await import("node:child_process"),
    { mkdirSync, mkdtempSync, realpathSync, writeFileSync } = await import("node:fs"),
    { tmpdir } = await import("node:os"),
    repo = realpathSync(mkdtempSync(`${tmpdir()}/linked-gate-`)),
    git = (...args: string[]) => execFileSync("git", ["-C", repo, ...args], { stdio: "ignore" });
  git("init", "-q");
  git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init");
  git("worktree", "add", "-q", "--detach", `${repo}/.worktrees/agent`);
  const linked = `${repo}/.worktrees/agent`;
  for (const dir of [repo, linked]) {
    mkdirSync(`${dir}/src`, { recursive: true });
    writeFileSync(`${dir}/a.txt`, "a");
    writeFileSync(`${dir}/src/b.ts`, "b");
  }

  const cases: [string, string | null, string | null][] = [
    // [command, main-checkout result, linked-worktree result]
    ["git add -A", SHELL_MSG.add, null],
    ["git add .", SHELL_MSG.add, null],
    ['git commit -am "x"', SHELL_MSG.add, null],
    ["git branch backup/pre-rebase HEAD", SHELL_MSG.branch, null],
    ["git switch -c feat/x", SHELL_MSG.branch, null],
    ["git checkout -b feat/x", SHELL_MSG.branch, null],
    ["git branch -f main HEAD", SHELL_MSG.branch, SHELL_MSG.branch],
    ["git switch -C feat/x", SHELL_MSG.branch, SHELL_MSG.branch],
    ["git checkout -B feat/x", SHELL_MSG.branch, SHELL_MSG.branch],
    [`git worktree add -B feat/x ${repo}/.worktrees/x`, SHELL_MSG.branch, SHELL_MSG.branch],
    [`git worktree add -Bfeat/x ${repo}/.worktrees/x`, SHELL_MSG.branch, SHELL_MSG.branch],
    ["git restore a.txt", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git restore --staged --worktree a.txt", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout -- a.txt src/b.ts", null, null],
    ["git checkout origin/main -- a.txt", null, null],
    ["git checkout a.txt", null, null],
    ["git checkout main", null, null],
    ["git checkout -- .", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout .", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout -- src", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout src", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout -- '*.ts'", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout -- :/", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout --", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout -f main", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git checkout -p -- a.txt", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git reset --hard", SHELL_MSG.discard, SHELL_MSG.discard],
    ["git clean -fd", SHELL_MSG.clean, SHELL_MSG.clean],
    ["git stash drop", SHELL_MSG.stashDrop, SHELL_MSG.stashDrop],
  ];
  for (const [command, main, inLinked] of cases) {
    assert.equal(classifyGit(command, repo), main, `main: ${command}`);
    assert.equal(classifyGit(command, linked), inLinked, `linked: ${command}`);
  }
  assert.equal(classifyGit(`cd ${linked} && git add -A`, repo), null);
  assert.equal(classifyGit(`git -C ${linked} commit -am x`, repo), null);
});

test("classifyCiWait blocks every command that waits instead of working", async () => {
  const { classifyCiWait, WAIT_MSG } = await import("./shell.ts");
  assert.match(WAIT_MSG, /in Claude Code, `run_in_background` or Monitor/u);
  assert.match(WAIT_MSG, /in Codex, exec_command, then empty write_stdin calls/u);
  assert.match(WAIT_MSG, /For CI, use ci_find and ci_watch/u);
  for (const [command, blocked] of [
    ["sleep 5", true],
    ["sleep 0.5 && ls", true],
    ["for i in $(seq 1 55); do grep -q done /tmp/x.log && break; sleep 10; done", true],
    ["until curl -s localhost:3000; do sleep 1; done", true],
    ["timeout 30 sleep 10", true],
    ["bash -c 'sleep 60; echo hi'", true],
    ["watch -n 5 git status", true],
    ["tail -f /tmp/cza.log", true],
    ["tail -n 20 -F /tmp/cza.log", true],
    ["kubectl logs -f deploy/api", true],
    ["docker logs --follow api", true],
    ["kubectl wait --for=condition=ready pod/x --timeout=60s", true],
    ["kubectl get pods -w", true],
    ["kubectl rollout status deploy/api", true],
    ["argocd app wait app --health --timeout 30", true],
    ["herdr agent wait reviewer --until blocked --timeout 120000", true],
    ["herdr agent prompt reviewer 'Review it.' --wait --timeout 120000", true],
    ["herdr pane wait-output p1 --match done --timeout 120000", true],
    ["gh pr checks 667 --repo oisin-ee/rondo --watch", true],
    ["for id in 1 2; do gh run watch $id; done", true],
    ["gh -R oisin-ee/rondo run watch 123 --exit-status", true],
    ["tail -n 20 /tmp/cza.log", false],
    ["kubectl logs deploy/api --tail 50", false],
    ["kubectl get pods", false],
    ["gh pr checks 667 --repo oisin-ee/rondo", false],
    ["gh run view 123 --log-failed", false],
    ["rg 'sleep|gh run watch' docs", false],
    ["nohup sh -c 'mise bootstrap --yes > /tmp/cza.log 2>&1' >/dev/null 2>&1 &", false],
    ["pnpm test", false],
  ] as [string, boolean][]) {
    assert.equal(classifyCiWait(command), blocked ? WAIT_MSG : null, command);
  }
});

test("agent typing: refuses typing into another agent's terminal and allows the inbox", async () => {
  const { classifyAgentTyping, TYPING_MSG } = await import("./shell.ts"),
    agents = new Set(["w1:p1", "w1:p2"]),
    isAgent = (pane: string) => agents.has(pane),
    check = (command: string | string[]) => classifyAgentTyping(command, "/repo", isAgent, "w1:p2");
  for (const command of [
    "herdr agent prompt w1:p1 hi",
    "herdr --session main agent prompt w1:p1 hi",
    "HERDR_ENV=1 timeout 5 herdr agent prompt w1:p1 'hi there'",
    "herdr agent send-keys w1:p1 Enter",
    "herdr agent send-keys w1:p1 Enter # wrangle: answer-prompt",
    "herdr pane send-text w1:p1 hello",
    "herdr pane send-keys w1:p1 Enter",
    "herdr pane send-input w1:p1 hello",
    "echo ok && herdr pane send-text w1:p1 hello",
    "bash -c 'herdr agent prompt w1:p1 hi'",
    ["herdr", "agent", "prompt", "w1:p1", "hi"],
  ]) {
    assert.equal(check(command), TYPING_MSG, String(command));
  }
  for (const command of [
    "dogsled send w1:p1 --message hi",
    "dogsled send worker --message 'herdr agent prompt is gone'",
    "herdr pane send-text w1:p9 'ls -la'",
    "herdr pane send-keys w1:p2 Enter",
    "herdr pane read w1:p1 --source visible",
    "herdr agent list",
  ]) {
    assert.equal(check(command), null, String(command));
  }
});

test("agent typing: the dogsled answer-prompt marker allows agent send-keys and records it", async () => {
  const { mkdtempSync, readFileSync } = await import("node:fs"),
    { tmpdir } = await import("node:os"),
    { join } = await import("node:path"),
    { classifyAgentTyping } = await import("./shell.ts"),
    state = mkdtempSync(join(tmpdir(), "typing-")),
    previous = process.env["XDG_STATE_HOME"];
  process.env["XDG_STATE_HOME"] = state;
  try {
    const command = "herdr agent send-keys w1:p1 Enter # dogsled: answer-prompt";
    assert.equal(
      classifyAgentTyping(command, "/repo", () => true, "w1:p2"),
      null,
    );
    const [line = ""] = readFileSync(join(state, "agent-hooks", "answer-prompt.jsonl"), "utf8")
        .trim()
        .split("\n"),
      record = JSON.parse(line) as { pane: string; command: string };
    assert.equal(record.pane, "w1:p1");
    assert.equal(record.command, command);
    // The marker never lets text through.
    assert.notEqual(
      classifyAgentTyping(
        "herdr agent prompt w1:p1 hi # dogsled: answer-prompt",
        "/repo",
        () => true,
        "w1:p2",
      ),
      null,
    );
  } finally {
    if (previous === undefined) {
      delete process.env["XDG_STATE_HOME"];
    } else {
      process.env["XDG_STATE_HOME"] = previous;
    }
  }
});
