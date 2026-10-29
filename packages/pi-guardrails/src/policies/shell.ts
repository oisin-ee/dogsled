import { spawnSync } from "node:child_process";
import { appendFileSync, mkdirSync, realpathSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";

import { commandName, splitShell, splitShellGuards, tokens } from "../common.ts";
import type { Command } from "../common.ts";

type Words = readonly string[];
/** A shell policy: the block message for a command, or null. */
type Classifier = (command: Command, cwd: string) => string | null;

const NO_VERIFY = /(^|\s)--no-verify(\s|$)/,
  SHELL_WRAPPERS: ReadonlySet<string> = new Set([
    "command",
    "builtin",
    "exec",
    "time",
    "sudo",
    "doas",
    "nohup",
    "nice",
  ]),
  SHELL_NAMES: ReadonlySet<string> = new Set(["sh", "bash", "zsh", "dash", "ksh", "fish"]),
  IDENTITY_ENV_VARS: ReadonlySet<string> = new Set([
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
  ]),
  IDENTITY_CONFIG_KEYS: ReadonlySet<string> = new Set(["user.name", "user.email"]);

export const SHELL_MSG = {
  add:
    "git-gate: blocked staging the whole working tree (git add -A/./-u/--all or git commit -a) outside an " +
    "agent-owned linked worktree. Per rules/30-safety-and-delivery.md, on the main checkout stage only the files " +
    "YOU changed: git add -- <paths>.",
  agentBrowserIsolation:
    "agent-browser-isolation: blocked direct agent-browser execution. From the target repository, use " +
    "`mise run agent:browser -- <command>`, then run `mise run agent:browser -- close` after browser proof.",
  branch:
    "git-gate: blocked creating or resetting a branch. Per rules/30-safety-and-delivery.md, create new branches with " +
    "git worktree add -b <branch> .worktrees/<name> [<commit-ish>], or with git branch/switch -c/checkout -b " +
    "inside an agent-owned linked worktree. For a delegated child's branch, run " +
    "`dogsled place --branch <branch> --base origin/main` instead (rules/20-delegation.md). " +
    "Forced branch resets (-B, -C, -f) are never allowed.",
  clean:
    'git-gate: blocked "git clean -f" which permanently deletes untracked files. Per rules/30-safety-and-delivery.md, ' +
    'never discard work on your own — preview with "git clean -n" or ask the user.',
  discard:
    "git-gate: blocked a command that discards uncommitted changes. Per rules/30-safety-and-delivery.md, never " +
    "discard the working tree on your own — commit or stash first, or ask the user. git restore is never allowed; " +
    "git checkout may only name explicit files (no ., directories, globs, pathspec magic, -f, or -p).",
  identity:
    "git-gate: blocked setting a git user.name/user.email or author/committer identity on the fly " +
    "(git config user.name/user.email, git -c user.name=/user.email=, git commit --author, or " +
    "GIT_AUTHOR_*/GIT_COMMITTER_* env vars). Use the repository's configured identity instead.",
  noVerify:
    "quality-gate: blocked shell command using --no-verify. " +
    "Run the configured verification hooks instead of bypassing them.",
  stashClear:
    'git-gate: blocked "git stash clear" which destroys every stash at once. Per rules/30-safety-and-delivery.md, drop ' +
    "each finished stash with the verified form: " +
    "git stash show -p --binary -u stash@{N} | git apply --check -R && git stash drop stash@{N}",
  stashDrop:
    'git-gate: blocked "git stash drop" without proof that the stash is already applied. Only drop a stash when its ' +
    "work is already done. From the repository root, run exactly: " +
    "git stash show -p --binary -u stash@{N} | git apply --check -R && git stash drop stash@{N} " +
    "(same N in both). The drop runs only if every stashed change, including untracked files, is already in the " +
    "working tree. If the check fails, apply or commit the remaining work, or ask the user.",
  stashPushPop:
    "git-gate: blocked a shell command that combines git stash push and git stash pop. This is unsafe because " +
    "unrelated working-tree changes can be lost or conflict on restore. Use a worktree or ask the user.",
  worktree:
    "git-gate: blocked creating a worktree outside repo-local .worktrees/. Per rules/30-safety-and-delivery.md, " +
    "worktrees must use git worktree add [-b <branch>] .worktrees/<name> [<commit-ish>]. For a delegated child's " +
    "worktree, run `dogsled place --branch <branch> --base origin/main` instead (rules/20-delegation.md).",
};

export const TYPING_MSG =
  "agent-typing: blocked typing into another agent's terminal. Typed text lands in the user's draft and mixes " +
  "with it. Message the agent through its dogsled inbox instead: the `send` tool (mcp__dogsled__send) or " +
  "`dogsled send <name-or-pane> --message <text>`. To answer a prompt dialog in another pane with keys, add " +
  "`# dogsled: answer-prompt` to `herdr agent send-keys`; each such use is recorded.";

const ANSWER_PROMPT = /#\s*dogsled:\s*answer-prompt\b/u,
  HERDR_VALUE_OPTIONS: ReadonlySet<string> = new Set(["--session", "--machine", "--remote"]),
  PANE_TYPING: ReadonlySet<string> = new Set(["send-text", "send-keys", "send-input"]);

// The herdr subcommand words after global options, through env, timeout, and shell wrappers.
function herdrWords(inputWords: Words): string[] {
  const words = [...inputWords];
  while (words[0] && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
    words.shift();
  }
  if (words.length === 0) {
    return [];
  }
  const base = commandName(words.shift() ?? "");
  if (SHELL_WRAPPERS.has(base) || base === "env") {
    while (words[0]?.startsWith("-") || /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0] ?? "")) {
      words.shift();
    }
    return herdrWords(words);
  }
  if (base === "timeout" || base === "gtimeout") {
    while (words[0]?.startsWith("-")) {
      words.shift();
    }
    return herdrWords(words.slice(1));
  }
  if (base !== "herdr") {
    return [];
  }
  const result: string[] = [];
  for (let index = 0; index < words.length; index += 1) {
    const item = words[index] ?? "";
    if (HERDR_VALUE_OPTIONS.has(item)) {
      index += 1;
    } else {
      result.push(item);
    }
  }
  return result;
}

// Whether `pane` runs an agent. An unknown pane cannot be typed into, so the lookup failing allows it.
export function paneRunsAgent(pane: string): boolean {
  const result = spawnSync("herdr", ["pane", "get", pane], { encoding: "utf8", timeout: 5000 });
  if (result.status !== 0) {
    return false;
  }
  try {
    const reply = JSON.parse(result.stdout) as { result?: { pane?: { agent?: unknown } } } | null;
    return Boolean(reply?.result?.pane?.agent);
  } catch {
    return false;
  }
}

function recordAnswerPrompt(command: Command, pane: string | null): void {
  const dir = join(
    process.env["XDG_STATE_HOME"] ?? join(homedir(), ".local", "state"),
    "agent-hooks",
  );
  mkdirSync(dir, { recursive: true });
  appendFileSync(
    join(dir, "answer-prompt.jsonl"),
    `${JSON.stringify({ at: new Date().toISOString(), command, from: process.env["HERDR_PANE_ID"] ?? null, pane })}\n`,
  );
}

function shellPieces(words: Words): string | null {
  const base = commandName(words[0] ?? "");
  if (!SHELL_NAMES.has(base)) {
    return null;
  }
  const commandIndex = words.findIndex((word) => word === "-c" || /^-.+c/u.test(word));
  return commandIndex === -1 ? null : (words[commandIndex + 1] ?? "");
}

// Typing into another agent's terminal: `herdr agent prompt`, `herdr agent send-keys`, and
// `herdr pane send-text|send-keys|send-input` aimed at a pane that runs an agent. `agent send-keys` with
// `# dogsled: answer-prompt` on the command line is allowed and recorded. The caller's own pane is not
// Another agent's. `isAgent` is the pane lookup; the second argument is the cwd every classifier receives.
export function classifyAgentTyping(
  command: Command,
  cwd = process.cwd(),
  isAgent: (pane: string) => boolean = paneRunsAgent,
  self: string | undefined = process.env["HERDR_PANE_ID"],
): string | null {
  const marked = typeof command === "string" && ANSWER_PROMPT.test(command);
  for (const piece of splitShell(command)) {
    const nested = shellPieces(piece);
    if (nested !== null) {
      const message = classifyAgentTyping(nested, cwd, isAgent, self);
      if (message) {
        return message;
      }
      continue;
    }
    const [group, verb, target] = herdrWords(piece);
    if (group === "agent" && verb === "prompt") {
      return TYPING_MSG;
    }
    if (group === "agent" && verb === "send-keys") {
      if (!marked) {
        return TYPING_MSG;
      }
      recordAnswerPrompt(command, target ?? null);
      continue;
    }
    if (
      group === "pane" &&
      PANE_TYPING.has(verb ?? "") &&
      target !== undefined &&
      target !== self &&
      isAgent(target)
    ) {
      return TYPING_MSG;
    }
  }
  return null;
}

export const IMPERATIVE_MSG = {
  argocd:
    "block-imperative-mutation: blocked a live Argo CD deploy/control command. GitOps-only: change " +
    "declarative manifests/values in git and let Argo reconcile — never argocd app sync/set/rollback, " +
    "appset create/delete, or mutate clusters/repos/projects by hand. argocd app get/list/diff, " +
    "appset get/list/generate, and cluster list are allowed.",
  fs:
    "block-imperative-mutation: blocked a live filesystem repair/format command (fsck/mkfs). These " +
    "imperatively mutate live volume state; surface the finding and the proper fix instead of running it by hand.",
  helm:
    "block-imperative-mutation: blocked a live helm release mutation. GitOps-only: commit the chart/values " +
    "change and let Argo reconcile — never helm install/upgrade/uninstall against the live cluster. " +
    "helm list/status/template/diff are allowed.",
  k8s:
    "block-imperative-mutation: blocked an imperative mutation of the live cluster. GitOps-only: change " +
    "the declarative manifest/values and let Argo reconcile — never kubectl-mutate the cluster by hand. " +
    "Read-only (get/describe/logs/diff) and --dry-run are allowed.",
  nix:
    "block-imperative-mutation: blocked a Nix build/run/profile/store mutation. Do not use nix as an " +
    "execution/install path to mutate infra by hand; use repo-pinned tooling or declarative config. " +
    "nix eval/flake show/path-info/store ls are allowed.",
};

export const WAIT_MSG =
  "wait-gate: blocked a command that waits instead of working (sleep, watch, a follow flag, a --watch or --wait " +
  "flag, or a wait subcommand). Never hold a tool call to wait, and never re-read a log to check progress. " +
  "For long work in pi, use Dogsled `run` (mcp__dogsled__run) and continue other work. " +
  "Use `read` (mcp__dogsled__read) for output and `stop` (mcp__dogsled__stop) to cancel the job. " +
  "Start long work so the host reports its exit: in Claude Code, `run_in_background` " +
  "or Monitor (the exit arrives as a message); in Codex, exec_command, then empty write_stdin calls, which " +
  "return when the process exits. " +
  "For CI, use ci_find and ci_watch.";

const GH_VALUE_OPTIONS: ReadonlySet<string> = new Set(["-R", "--repo", "--hostname"]),
  FOLLOW_FLAGS: ReadonlySet<string> = new Set(["-f", "-F", "--follow", "--retry"]),
  LOG_STREAMERS: ReadonlySet<string> = new Set([
    "kubectl",
    "docker",
    "podman",
    "stern",
    "journalctl",
  ]),
  WAIT_SUBCOMMAND_TOOLS: ReadonlySet<string> = new Set([
    "kubectl",
    "argocd",
    "herdr",
    "docker",
    "podman",
    "aws",
    "gcloud",
  ]);

function positionalWords(words: Words): string[] {
  const result: string[] = [];
  for (let index = 0; index < words.length; index += 1) {
    const word = words[index] ?? "";
    if (GH_VALUE_OPTIONS.has(word)) {
      index += 1;
    } else if (!word.startsWith("-")) {
      result.push(word);
    }
  }
  return result;
}

function hasFlag(words: Words, names: readonly string[]): boolean {
  return words.some((word) => names.some((name) => word === name || word.startsWith(`${name}=`)));
}

function classifyWaitPiece(inputWords: Words): string | null {
  const words = [...inputWords];
  while (words[0] && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
    words.shift();
  }
  if (words.length === 0) {
    return null;
  }
  const base = commandName(words.shift() ?? "");
  if (SHELL_WRAPPERS.has(base) || base === "xargs" || base === "env") {
    while (words[0]?.startsWith("-") || /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0] ?? "")) {
      words.shift();
    }
    return classifyWaitPiece(words);
  }
  if (base === "timeout" || base === "gtimeout") {
    while (words[0]?.startsWith("-")) {
      words.shift();
    }
    return classifyWaitPiece(words.slice(1));
  }
  if (SHELL_NAMES.has(base)) {
    const commandIndex = words.findIndex((word) => word === "-c" || /^-.+c/u.test(word));
    return commandIndex === -1 ? null : classifyCiWait(words[commandIndex + 1] ?? "");
  }
  if (base === "sleep" || base === "watch") {
    return WAIT_MSG;
  }
  if (
    base === "tail" &&
    words.some((word) => FOLLOW_FLAGS.has(word) || /^-[A-Za-z0-9]*[fF]/u.test(word))
  ) {
    return WAIT_MSG;
  }
  const [first, second] = positionalWords(words);
  if (
    LOG_STREAMERS.has(base) &&
    (first === "logs" || base === "stern" || base === "journalctl") &&
    words.some((word) => word === "-f" || word === "--follow" || word === "--follow=true")
  ) {
    return WAIT_MSG;
  }
  if (base === "gh") {
    if (first === "run" && second === "watch") {
      return WAIT_MSG;
    }
    if (first === "pr" && second === "checks" && hasFlag(words, ["--watch"])) {
      return WAIT_MSG;
    }
    return null;
  }
  if (
    WAIT_SUBCOMMAND_TOOLS.has(base) &&
    [first, second].some((word) => /^(wait|wait-output|watch)$/u.test(word ?? ""))
  ) {
    return WAIT_MSG;
  }
  if (base === "kubectl" && first === "rollout" && second === "status") {
    return WAIT_MSG;
  }
  if (base === "kubectl" && hasFlag(words, ["--watch", "-w", "--watch-only"])) {
    return WAIT_MSG;
  }
  return ["herdr", "helm"].includes(base) && hasFlag(words, ["--wait"]) ? WAIT_MSG : null;
}

// A wait holds one tool call while nothing in the session moves. Background work and short checks replace it.
// The name predates the wider scope; running pi sessions resolve this export by name, so it stays.
export function classifyCiWait(command: Command): string | null {
  for (const piece of splitShell(command)) {
    const message = classifyWaitPiece(tokens(piece));
    if (message) {
      return message;
    }
  }
  return null;
}

export function classifyNoVerify(command: Command): string | null {
  const text = typeof command === "string" ? command : command.join(" ");
  return NO_VERIFY.test(text) ? SHELL_MSG.noVerify : null;
}

const AGENT_BROWSER_BINARIES: ReadonlySet<string> = new Set(["agent-browser"]),
  ENV_VALUE_OPTIONS: ReadonlySet<string> = new Set([
    "-C",
    "--chdir",
    "-S",
    "--split-string",
    "-u",
    "--unset",
  ]),
  EMPTY_OPTIONS: ReadonlySet<string> = new Set(),
  WRAPPER_VALUE_OPTIONS: ReadonlyMap<string, ReadonlySet<string>> = new Map<
    string,
    ReadonlySet<string>
  >([
    ["doas", new Set(["-C", "-u"])],
    ["exec", new Set(["-a"])],
    ["nice", new Set(["-n", "--adjustment"])],
    [
      "sudo",
      new Set([
        "-C",
        "--close-from",
        "-D",
        "--chdir",
        "-g",
        "--group",
        "-h",
        "--host",
        "-p",
        "--prompt",
        "-R",
        "--chroot",
        "-T",
        "--command-timeout",
        "-u",
        "--user",
      ]),
    ],
    ["time", new Set(["-f", "--format", "-o", "--output"])],
  ]);

function discardOptions(words: string[], valueOptions: ReadonlySet<string>): void {
  while (words.length > 0) {
    const option = words[0] ?? "";
    if (option === "--") {
      words.shift();
      break;
    }
    if (!option.startsWith("-")) {
      break;
    }
    words.shift();
    if (!option.includes("=") && valueOptions.has(option)) {
      words.shift();
    }
  }
}

function stashOperation(inputWords: Words): string | null {
  const words = [...inputWords];
  while (words[0] && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
    words.shift();
  }
  if (words.length === 0) {
    return null;
  }

  const base = commandName(words.shift() ?? "");
  if (base === "env") {
    discardOptions(words, ENV_VALUE_OPTIONS);
    while (words[0] && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
      words.shift();
    }
    return stashOperation(words);
  }
  if (SHELL_WRAPPERS.has(base)) {
    discardOptions(words, WRAPPER_VALUE_OPTIONS.get(base) ?? EMPTY_OPTIONS);
    return stashOperation(words);
  }
  if (SHELL_NAMES.has(base)) {
    const commandIndex = words.findIndex((word) => word === "-c" || /^-.+c/.test(word));
    return commandIndex === -1 ? null : stashOperation(tokens(words[commandIndex + 1] ?? ""));
  }
  if (base !== "git") {
    return null;
  }

  while (words[0]?.startsWith("-") === true) {
    const option = words.shift();
    if ((option === "-C" || option === "-c") && words.length > 0) {
      words.shift();
    }
  }
  if (words.shift() !== "stash") {
    return null;
  }

  while (words[0]?.startsWith("-") && words[0] !== "--") {
    const option = words.shift();
    if ((option === "-m" || option === "--message") && words.length > 0) {
      words.shift();
    }
  }
  const subcommand = words[0] ?? "";
  if (words[0] === "--") {
    return "push";
  }
  if (subcommand === "pop") {
    return "pop";
  }
  if (
    subcommand === "" ||
    subcommand === "push" ||
    subcommand === "save" ||
    subcommand.startsWith("-")
  ) {
    return "push";
  }
  return "stash";
}

function stashOperations(command: Command): Set<string> {
  const operations = new Set<string>();
  for (const piece of splitShell(command)) {
    const operation = stashOperation(tokens(piece));
    if (operation) {
      operations.add(operation);
    }
  }
  return operations;
}

export function classifyStashPushPop(command: Command): string | null {
  const operations = stashOperations(command);
  return operations.has("push") && operations.has("pop") ? SHELL_MSG.stashPushPop : null;
}

function hasDirectAgentBrowserInvocation(inputWords: Words): boolean {
  const words = [...inputWords];
  while (words[0] && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
    words.shift();
  }
  if (words.length === 0) {
    return false;
  }

  const base = commandName(words.shift() ?? "");
  if (AGENT_BROWSER_BINARIES.has(base)) {
    return true;
  }
  if (base === "env") {
    discardOptions(words, ENV_VALUE_OPTIONS);
    while (words[0] && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
      words.shift();
    }
    return hasDirectAgentBrowserInvocation(words);
  }
  if (SHELL_WRAPPERS.has(base)) {
    discardOptions(words, WRAPPER_VALUE_OPTIONS.get(base) ?? EMPTY_OPTIONS);
    return hasDirectAgentBrowserInvocation(words);
  }
  if (SHELL_NAMES.has(base)) {
    const commandIndex = words.findIndex((word) => word === "-c" || /^-.+c/.test(word));
    return commandIndex === -1
      ? false
      : classifyAgentBrowserIsolation(words[commandIndex + 1] ?? "") !== null;
  }
  if (base === "mise") {
    const separator = words.indexOf("--");
    return separator === -1 ? false : hasDirectAgentBrowserInvocation(words.slice(separator + 1));
  }
  return false;
}

export function classifyAgentBrowserIsolation(command: Command): string | null {
  return splitShell(command).some((piece) => hasDirectAgentBrowserInvocation(tokens(piece)))
    ? SHELL_MSG.agentBrowserIsolation
    : null;
}

const WORKTREE_VALUE_OPTIONS: ReadonlySet<string> = new Set(["-b", "-B", "--reason"]),
  WORKTREE_FLAG_OPTIONS: ReadonlySet<string> = new Set([
    "-f",
    "--force",
    "--no-force",
    "-d",
    "--detach",
    "--no-detach",
    "--checkout",
    "--no-checkout",
    "--lock",
    "--no-lock",
    "--orphan",
    "--no-orphan",
    "--quiet",
    "-q",
    "--track",
    "--no-track",
    "--guess-remote",
    "--no-guess-remote",
    "--relative-paths",
    "--no-relative-paths",
  ]);

function worktreeAddPath(args: Words): string {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index] ?? "";
    if (arg === "--") {
      return args[index + 1] ?? "";
    }
    if (WORKTREE_VALUE_OPTIONS.has(arg)) {
      index += 1;
      continue;
    }
    if (/^-b.+/.test(arg) || /^-B.+/.test(arg) || arg.startsWith("--reason=")) {
      continue;
    }
    if (WORKTREE_FLAG_OPTIONS.has(arg) || arg.startsWith("--")) {
      continue;
    }
    if (arg.startsWith("-")) {
      continue;
    }
    return arg;
  }
  return "";
}

function realPath(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return path;
  }
}

// The main checkout owns `.worktrees/`, even when the command runs inside a linked worktree.
function mainWorktreeRoot(cwd: string): string {
  const result = spawnSync(
      "git",
      ["-C", cwd, "rev-parse", "--path-format=absolute", "--git-common-dir"],
      {
        encoding: "utf8",
        timeout: 5000,
      },
    ),
    commonDir = result.status === 0 ? result.stdout.trim() : "";
  return basename(commonDir) === ".git" ? realPath(dirname(commonDir)) : realPath(cwd);
}

function worktreePathAllowed(commandCwd: string, target: string): boolean {
  const worktreesDir = `${mainWorktreeRoot(commandCwd)}/.worktrees/`;
  return resolve(realPath(commandCwd), target).startsWith(worktreesDir);
}

// A linked worktree has its own git dir under the shared common dir; one agent owns it.
function inLinkedWorktree(cwd: string): boolean {
  const result = spawnSync(
    "git",
    ["-C", cwd, "rev-parse", "--path-format=absolute", "--absolute-git-dir", "--git-common-dir"],
    { encoding: "utf8", timeout: 5000 },
  );
  if (result.status !== 0) {
    return false;
  }
  const [gitDir, commonDir] = result.stdout.trim().split("\n").map(realPath);
  return (
    gitDir !== undefined &&
    commonDir !== undefined &&
    gitDir !== "" &&
    commonDir !== "" &&
    gitDir !== commonDir
  );
}

function isDirectory(cwd: string, path: string): boolean {
  try {
    return statSync(resolve(cwd, path)).isDirectory();
  } catch {
    return false;
  }
}

function pathExists(cwd: string, path: string): boolean {
  try {
    statSync(resolve(cwd, path));
    return true;
  } catch {
    return false;
  }
}

// An explicit file: no pathspec magic, no glob, not `.`, and not a directory.
function explicitFile(cwd: string, path: string): boolean {
  return (
    Boolean(path) && !path.startsWith(":") && !/[*?[\]]/u.test(path) && !isDirectory(cwd, path)
  );
}

const CHECKOUT_VALUE_OPTIONS: ReadonlySet<string> = new Set([
  "-b",
  "-B",
  "--orphan",
  "--conflict",
  "--pathspec-from-file",
]);

// Git checkout may restore files only when each one is named explicitly.
function classifyCheckout(rest: Words, cwd: string): string | null {
  if (
    rest.some(
      (item) =>
        ["-f", "--force", "-p", "--patch", "--pathspec-from-file"].includes(item) ||
        item.startsWith("--pathspec-from-file="),
    )
  ) {
    return SHELL_MSG.discard;
  }
  if (rest.includes("-B")) {
    return SHELL_MSG.branch;
  }
  if (rest.includes("-b") && !inLinkedWorktree(cwd)) {
    return SHELL_MSG.branch;
  }
  const separator = rest.indexOf("--");
  if (separator !== -1) {
    const paths = rest.slice(separator + 1);
    return paths.length > 0 && paths.every((path) => explicitFile(cwd, path))
      ? null
      : SHELL_MSG.discard;
  }
  const paths: string[] = [];
  for (let index = 0; index < rest.length; index += 1) {
    const item = rest[index] ?? "";
    if (CHECKOUT_VALUE_OPTIONS.has(item)) {
      index += 1;
    } else if (!item.startsWith("-")) {
      paths.push(item);
    }
  }
  // Without `--`, git reads existing paths as pathspecs; branch and commit names fall through.
  return paths.some(
    (item) => (pathExists(cwd, item) || /^:|[*?[\]]/u.test(item)) && !explicitFile(cwd, item),
  )
    ? SHELL_MSG.discard
    : null;
}

function classifyBranch(rest: Words, cwd: string): string | null {
  if (rest.some((item) => ["-f", "--force"].includes(item))) {
    return SHELL_MSG.branch;
  }
  return rest[0] && !rest[0].startsWith("-") && !inLinkedWorktree(cwd) ? SHELL_MSG.branch : null;
}

function changedDirectory(words: Words, cwd: string): string {
  if (words[0] !== "cd" || words.length !== 2) {
    return cwd;
  }
  const dir = words[1] ?? "",
    target = dir === "~" || dir.startsWith("~/") ? homedir() + dir.slice(1) : dir;
  return resolve(cwd, target);
}

const GIT_CONFIG_READ_ONLY: ReadonlySet<string> = new Set([
    "--get",
    "--get-all",
    "--get-regexp",
    "--get-urlmatch",
    "-l",
    "--list",
    "--get-color",
    "--get-colorbool",
  ]),
  GIT_CONFIG_MUTATING_FLAGS: ReadonlySet<string> = new Set([
    "--add",
    "--replace-all",
    "--unset",
    "--unset-all",
  ]),
  GIT_CONFIG_SCOPE_FLAGS: ReadonlySet<string> = new Set([
    "--global",
    "--local",
    "--system",
    "--worktree",
    "--includes",
    "--no-includes",
    "-z",
    "--null",
  ]),
  GIT_CONFIG_VALUE_FLAGS: ReadonlySet<string> = new Set([
    "--file",
    "-f",
    "--blob",
    "--type",
    "-t",
    "--default",
  ]);

function classifyGitConfigIdentity(rest: Words): boolean {
  const words = [...rest];
  let readOnly = false,
    mutating = false;
  while (words.length > 0) {
    const word = words[0] ?? "";
    if (GIT_CONFIG_READ_ONLY.has(word)) {
      readOnly = true;
      words.shift();
      continue;
    }
    if (GIT_CONFIG_MUTATING_FLAGS.has(word)) {
      mutating = true;
      words.shift();
      continue;
    }
    if (GIT_CONFIG_SCOPE_FLAGS.has(word)) {
      words.shift();
      continue;
    }
    if (GIT_CONFIG_VALUE_FLAGS.has(word)) {
      words.shift();
      if (!word.includes("=")) {
        words.shift();
      }
      continue;
    }
    if (word.startsWith("-")) {
      words.shift();
      continue;
    }
    break;
  }
  const key = (words[0] ?? "").toLowerCase();
  if (!IDENTITY_CONFIG_KEYS.has(key)) {
    return false;
  }
  return readOnly ? false : mutating || words.length > 1;
}

const STASH_SHOW_FLAGS: ReadonlyMap<string, string> = new Map([
  ["-p", "patch"],
  ["--patch", "patch"],
  ["--binary", "binary"],
  ["-u", "untracked"],
  ["--include-untracked", "untracked"],
]);

// `git [-C <dir>] <args>` with no assignments, wrappers, or other global options.
function plainGit(words: Words, cwd: string): { args: string[]; cwd: string } | null {
  if (words[0] !== "git" && !words[0]?.endsWith("/git")) {
    return null;
  }
  if (words[1] !== "-C") {
    return { args: words.slice(1), cwd };
  }
  return words[2] ? { args: words.slice(3), cwd: resolve(cwd, words[2]) } : null;
}

// `stash@{<n>}` or `<n>` names stash <n>; no stash names stash 0. Other stash forms are not accepted.
function stashIndex(refs: Words): number | null {
  if (refs.length > 1) {
    return null;
  }
  const match = /^(?:stash@\{(\d+)\}|(\d+))$/u.exec(refs[0] ?? "0");
  return match ? Number(match[1] ?? match[2]) : null;
}

function stashShowIndex(args: Words): number | null {
  if (args[0] !== "stash" || args[1] !== "show") {
    return null;
  }
  const flags = new Set<string>(),
    refs: string[] = [];
  for (const arg of args.slice(2)) {
    const flag = STASH_SHOW_FLAGS.get(arg);
    if (flag !== undefined) {
      flags.add(flag);
    } else if (arg.startsWith("-")) {
      return null;
    } else {
      refs.push(arg);
    }
  }
  return flags.size === 3 ? stashIndex(refs) : null;
}

function isReverseApplyCheck(args: Words): boolean {
  const options = new Set(args.slice(1));
  return (
    args[0] === "apply" &&
    args.length === 3 &&
    options.has("--check") &&
    (options.has("-R") || options.has("--reverse"))
  );
}

function stashDropIndex(args: Words): number | null {
  if (args[0] !== "stash" || args[1] !== "drop") {
    return null;
  }
  return stashIndex(args.slice(2).filter((arg) => arg !== "-q" && arg !== "--quiet"));
}

function isRepositoryRoot(cwd: string): boolean {
  const result = spawnSync("git", ["-C", cwd, "rev-parse", "--show-prefix"], {
    encoding: "utf8",
    timeout: 5000,
  });
  return result.status === 0 && result.stdout.trim() === "";
}

// A drop is allowed only as the right operand of `&&` after the exact check that every change of the same stash,
// Including untracked and binary files, is already in the working tree. `git apply` ignores patched paths outside
// The current subdirectory, so the check must run at the repository root.
function verifiedStashDrop(
  piece: Words,
  guard: readonly Words[] | null | undefined,
  cwd: string,
): boolean {
  const drop = plainGit(tokens(piece), cwd);
  if (!drop || guard?.length !== 2) {
    return false;
  }
  const show = plainGit(guard[0] ?? [], cwd),
    apply = plainGit(guard[1] ?? [], cwd);
  if (!show || !apply || show.cwd !== drop.cwd || apply.cwd !== drop.cwd) {
    return false;
  }
  const index = stashDropIndex(drop.args);
  return (
    index !== null &&
    stashShowIndex(show.args) === index &&
    isReverseApplyCheck(apply.args) &&
    isRepositoryRoot(drop.cwd)
  );
}

function classifyGitPiece(
  piece: Words,
  repoRoot: string,
  guard: readonly Words[] | null | undefined,
): string | null {
  const words = tokens(piece);
  let identityEnv = false;
  while (words[0] !== undefined && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
    const name = words[0].slice(0, words[0].indexOf("="));
    if (IDENTITY_ENV_VARS.has(name)) {
      identityEnv = true;
    }
    words.shift();
  }
  const base = words.shift() ?? "";
  if (base !== "git" && !base.endsWith("/git")) {
    return null;
  }
  if (identityEnv) {
    return SHELL_MSG.identity;
  }
  let commandCwd = repoRoot;
  while (words.length > 0) {
    const word = words[0] ?? "";
    if (word === "-C") {
      const next = words[1];
      if (next) {
        commandCwd = new URL(next, `file://${commandCwd.replace(/\/$/, "")}/`).pathname;
      }
      words.splice(0, 2);
      continue;
    }
    if (word === "-c") {
      if (/^user\.(name|email)=/i.test(words[1] ?? "")) {
        return SHELL_MSG.identity;
      }
      words.splice(0, 2);
      continue;
    }
    if (/^--(git-dir|work-tree|namespace)=/.test(word) || word.startsWith("--")) {
      words.shift();
      continue;
    }
    if (word.startsWith("-")) {
      words.shift();
      continue;
    }
    break;
  }
  const sub = words.shift() ?? "",
    rest = words,
    has = (...items: string[]): boolean => rest.some((item) => items.includes(item));
  if (sub === "add") {
    return has("-A", "--all", "-u", "--update", ".", ":/") && !inLinkedWorktree(commandCwd)
      ? SHELL_MSG.add
      : null;
  }
  if (sub === "commit") {
    if (rest.some((item) => item === "--author" || item.startsWith("--author="))) {
      return SHELL_MSG.identity;
    }
    const all = rest.some((item) => item === "--all" || /^-[^-]*a/.test(item));
    return all && !inLinkedWorktree(commandCwd) ? SHELL_MSG.add : null;
  }
  if (sub === "config") {
    return classifyGitConfigIdentity(rest) ? SHELL_MSG.identity : null;
  }
  if (sub === "branch") {
    return classifyBranch(rest, commandCwd);
  }
  if (sub === "worktree") {
    if (rest[0] !== "add") {
      return null;
    }
    const args = rest.slice(1),
      options = args.includes("--") ? args.slice(0, args.indexOf("--")) : args;
    if (options.some((item) => item.startsWith("-B"))) {
      return SHELL_MSG.branch;
    }
    const target = worktreeAddPath(rest.slice(1));
    return target && !worktreePathAllowed(commandCwd, target) ? SHELL_MSG.worktree : null;
  }
  if (sub === "switch") {
    if (has("-C", "--force-create")) {
      return SHELL_MSG.branch;
    }
    return has("-c", "--create") && !inLinkedWorktree(commandCwd) ? SHELL_MSG.branch : null;
  }
  if (sub === "checkout") {
    return classifyCheckout(rest, commandCwd);
  }
  if (sub === "reset") {
    return has("--hard") ? SHELL_MSG.discard : null;
  }
  if (sub === "restore") {
    let staged = false,
      worktree = false;
    for (const item of rest) {
      if (item === "--staged" || item === "-S") {
        staged = true;
      }
      if (item === "--worktree" || item === "-W") {
        worktree = true;
      }
      if (item === "-SW" || item === "-WS") {
        staged = true;
        worktree = true;
      }
    }
    return worktree || !staged ? SHELL_MSG.discard : null;
  }
  if (sub === "clean") {
    let force = false,
      dry = false;
    for (const item of rest) {
      if (item === "--dry-run") {
        dry = true;
      } else if (item === "--force" || item === "--interactive") {
        force = true;
      } else if (!item.startsWith("--") && item.startsWith("-")) {
        if (item.includes("n")) {
          dry = true;
        }
        if (item.includes("f") || item.includes("i")) {
          force = true;
        }
      }
    }
    return !dry && force ? SHELL_MSG.clean : null;
  }
  if (sub !== "stash") {
    return null;
  }
  if (rest[0] === "clear") {
    return SHELL_MSG.stashClear;
  }
  return rest[0] === "drop" && !verifiedStashDrop(piece, guard, repoRoot)
    ? SHELL_MSG.stashDrop
    : null;
}

export function classifyGit(command: Command, cwd = process.cwd()): string | null {
  let commandCwd = cwd;
  const guards = splitShellGuards(command);
  for (const [index, piece] of splitShell(command).entries()) {
    commandCwd = changedDirectory(tokens(piece), commandCwd);
    const message = classifyGitPiece(piece, commandCwd, guards[index]);
    if (message) {
      return message;
    }
  }
  return null;
}

const K8S_MUTATING: ReadonlySet<string> = new Set([
    "apply",
    "create",
    "delete",
    "replace",
    "patch",
    "edit",
    "scale",
    "autoscale",
    "cordon",
    "uncordon",
    "drain",
    "taint",
    "evict",
    "expose",
    "set",
    "label",
    "annotate",
    "run",
  ]),
  HELM_MUTATING: ReadonlySet<string> = new Set([
    "install",
    "upgrade",
    "uninstall",
    "delete",
    "rollback",
  ]),
  ROLLOUT_MUTATING: ReadonlySet<string> = new Set(["restart", "undo", "pause", "resume"]),
  ARGOCD_APP_MUTATING: ReadonlySet<string> = new Set([
    "add-source",
    "confirm-deletion",
    "create",
    "delete",
    "delete-resource",
    "edit",
    "patch",
    "patch-resource",
    "remove-source",
    "rollback",
    "set",
    "sync",
    "terminate-op",
    "unset",
  ]),
  ARGOCD_APPSET_MUTATING: ReadonlySet<string> = new Set(["create", "delete"]),
  ARGOCD_CLUSTER_MUTATING: ReadonlySet<string> = new Set(["add", "rm", "rotate-auth", "set"]),
  ARGOCD_SUBRESOURCE_READ_ONLY: ReadonlySet<string> = new Set(["", "get", "list"]),
  NIX_MUTATING: ReadonlySet<string> = new Set([
    "build",
    "bundle",
    "copy",
    "develop",
    "run",
    "shell",
    "upgrade-nix",
  ]),
  NIX_PROFILE_MUTATING: ReadonlySet<string> = new Set([
    "add",
    "install",
    "remove",
    "upgrade",
    "rollback",
    "wipe-history",
  ]),
  NIX_FLAKE_MUTATING: ReadonlySet<string> = new Set([
    "archive",
    "clone",
    "init",
    "lock",
    "new",
    "update",
  ]),
  NIX_STORE_MUTATING: ReadonlySet<string> = new Set([
    "add-file",
    "add-path",
    "delete",
    "gc",
    "optimise",
    "repair",
  ]),
  NIX_REGISTRY_MUTATING: ReadonlySet<string> = new Set(["add", "pin", "remove"]);

function classifyImperativePiece(piece: Words): string | null {
  const words = tokens(piece);
  while (words[0] !== undefined && /^[A-Za-z_][A-Za-z0-9_]*=/.test(words[0])) {
    words.shift();
  }
  if (words.length === 0) {
    return null;
  }
  const base = (words.shift() ?? "").replaceAll(/^["']|["']$/gu, "");
  if (SHELL_NAMES.has(commandName(base))) {
    const commandIndex = words.findIndex((word) => word === "-c" || /^-.+c/u.test(word));
    return commandIndex === -1 ? null : classifyImperativeMutation(words[commandIndex + 1] ?? "");
  }
  if (/(^|\/)(fsck|mkfs)(\..+)?$/.test(base) || base.endsWith("fsck")) {
    return IMPERATIVE_MSG.fs;
  }
  let tool: "k8s" | "helm" | "argocd" | "nix";
  if (base === "kubectl" || base.endsWith("/kubectl")) {
    tool = "k8s";
  } else if (base === "helm" || base.endsWith("/helm")) {
    tool = "helm";
  } else if (base === "argocd" || base.endsWith("/argocd")) {
    tool = "argocd";
  } else if (base === "nix" || base.endsWith("/nix")) {
    tool = "nix";
  } else {
    return null;
  }
  if (
    words.some(
      (item) =>
        item === "--dry-run" || (item.startsWith("--dry-run=") && item !== "--dry-run=none"),
    )
  ) {
    return null;
  }
  const verbAt = (start: number): { verb: string; index: number } => {
      let index = start;
      while (index < words.length) {
        const item = words[index] ?? "";
        if (!item.startsWith("-")) {
          return { verb: item, index };
        }
        if (item.includes("=")) {
          index += 1;
          continue;
        }
        const next = words[index + 1];
        index += next !== undefined && !next.startsWith("-") ? 2 : 1;
      }
      return { index: -1, verb: "" };
    },
    { verb, index } = verbAt(0);
  if (!verb) {
    return null;
  }
  if (tool === "k8s") {
    if (verb === "rollout") {
      return ROLLOUT_MUTATING.has(verbAt(index + 1).verb) ? IMPERATIVE_MSG.k8s : null;
    }
    return K8S_MUTATING.has(verb) ? IMPERATIVE_MSG.k8s : null;
  }
  if (tool === "helm") {
    return HELM_MUTATING.has(verb) ? IMPERATIVE_MSG.helm : null;
  }
  if (tool === "argocd") {
    const nested = () => verbAt(index + 1).verb;
    if (verb === "app") {
      const appVerb = nested();
      if (appVerb === "actions") {
        return verbAt(index + 2).verb === "run" ? IMPERATIVE_MSG.argocd : null;
      }
      return ARGOCD_APP_MUTATING.has(appVerb) ? IMPERATIVE_MSG.argocd : null;
    }
    if (verb === "appset") {
      return ARGOCD_APPSET_MUTATING.has(nested()) ? IMPERATIVE_MSG.argocd : null;
    }
    if (verb === "cluster") {
      return ARGOCD_CLUSTER_MUTATING.has(nested()) ? IMPERATIVE_MSG.argocd : null;
    }
    if (verb === "account") {
      return nested() === "update-password" ? IMPERATIVE_MSG.argocd : null;
    }
    if (verb === "admin") {
      return nested() === "import" ? IMPERATIVE_MSG.argocd : null;
    }
    if (["cert", "gpg", "proj", "repo", "repocreds"].includes(verb)) {
      return ARGOCD_SUBRESOURCE_READ_ONLY.has(nested()) ? null : IMPERATIVE_MSG.argocd;
    }
    return null;
  }
  if (NIX_MUTATING.has(verb)) {
    return IMPERATIVE_MSG.nix;
  }
  const nested = () => verbAt(index + 1).verb;
  if (verb === "profile") {
    return NIX_PROFILE_MUTATING.has(nested()) ? IMPERATIVE_MSG.nix : null;
  }
  if (verb === "flake") {
    return NIX_FLAKE_MUTATING.has(nested()) ? IMPERATIVE_MSG.nix : null;
  }
  if (verb === "store") {
    return NIX_STORE_MUTATING.has(nested()) ? IMPERATIVE_MSG.nix : null;
  }
  return verb === "registry" && NIX_REGISTRY_MUTATING.has(nested()) ? IMPERATIVE_MSG.nix : null;
}

export function classifyImperativeMutation(command: Command): string | null {
  for (const piece of splitShell(command)) {
    const message = classifyImperativePiece(piece);
    if (message) {
      return message;
    }
  }
  return null;
}

/** Every shell policy, in the order a hook applies them. */
export const SHELL_CLASSIFIERS: readonly Classifier[] = [
  classifyNoVerify,
  classifyStashPushPop,
  classifyGit,
  classifyAgentBrowserIsolation,
  classifyImperativeMutation,
  classifyCiWait,
  classifyAgentTyping,
];
